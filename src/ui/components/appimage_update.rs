use adw::prelude::*;
use gtk::glib;
use reqwest::blocking::Client;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_LATEST_URL: &str = "https://api.github.com/repos/enz1m/enzim-coder/releases/latest";
const CHECK_INTERVAL: Duration = Duration::from_secs(300);
const ARCH_ASSET_TOKEN: &str = "x86_64";

#[derive(Clone, Debug)]
struct ReleaseAsset {
    browser_download_url: String,
}

#[derive(Clone, Debug)]
struct ReleaseInfo {
    version: String,
    title: String,
    body: String,
    published_at: Option<String>,
    appimage_asset: ReleaseAsset,
}

#[derive(Clone, Debug)]
enum UpdateState {
    Unsupported,
    Idle,
    Checking,
    Available(ReleaseInfo),
    Updating(ReleaseInfo),
    ReadyToRestart(ReleaseInfo),
    Error {
        release: Option<ReleaseInfo>,
        message: String,
    },
}

impl UpdateState {
    fn release(&self) -> Option<&ReleaseInfo> {
        match self {
            UpdateState::Available(release)
            | UpdateState::Updating(release)
            | UpdateState::ReadyToRestart(release) => Some(release),
            UpdateState::Error {
                release: Some(release),
                ..
            } => Some(release),
            UpdateState::Unsupported
            | UpdateState::Idle
            | UpdateState::Checking
            | UpdateState::Error { release: None, .. } => None,
        }
    }
}

enum WorkerMessage {
    CheckFinished(Result<Option<ReleaseInfo>, String>),
    UpdateFinished(Result<ReleaseInfo, String>),
}

struct UpdateCoordinator {
    state: RefCell<UpdateState>,
    tx: mpsc::Sender<WorkerMessage>,
    check_in_flight: Cell<bool>,
    update_in_flight: Cell<bool>,
    appimage_path: Option<PathBuf>,
    listeners: RefCell<Vec<Box<dyn Fn(UpdateState)>>>,
}

thread_local! {
    static UPDATE_COORDINATOR: RefCell<Option<Rc<UpdateCoordinator>>> = const { RefCell::new(None) };
}

pub fn build_update_button() -> gtk::Box {
    let coordinator = coordinator();

    let button = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    button.add_css_class("actions-toggle-button");
    button.add_css_class("topbar-update-button");
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    button.set_can_focus(false);
    button.set_visible(false);
    button.set_tooltip_text(Some("AppImage updates"));

    let icon = gtk::Image::from_icon_name("folder-download-symbolic");
    icon.set_pixel_size(14);
    button.append(&icon);

    let label = gtk::Label::new(Some("Update"));
    label.add_css_class("topbar-update-label");
    button.append(&label);

    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("topbar-update-dot");
    dot.set_size_request(6, 6);
    dot.set_halign(gtk::Align::Center);
    dot.set_valign(gtk::Align::Center);
    dot.set_hexpand(false);
    dot.set_vexpand(false);
    dot.set_visible(false);
    button.append(&dot);

    let popover = gtk::Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_parent(&button);
    popover.add_css_class("actions-popover");
    popover.add_css_class("update-popover");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(10);
    root.set_margin_end(10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_size_request(440, -1);
    root.add_css_class("actions-popover-root");

    let title = gtk::Label::new(Some("AppImage updates"));
    title.add_css_class("actions-popover-title");
    title.set_xalign(0.0);
    root.append(&title);

    let summary = gtk::Label::new(Some("No updates available."));
    summary.add_css_class("actions-popover-workspace");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&summary);

    let status = gtk::Label::new(None);
    status.add_css_class("actions-popover-status");
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&status);

    let changelog_heading = gtk::Label::new(Some("Changelog"));
    changelog_heading.add_css_class("actions-section-heading");
    changelog_heading.set_xalign(0.0);
    root.append(&changelog_heading);

    let changelog_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(220)
        .build();
    changelog_scroll.set_has_frame(false);

    let changelog_label = gtk::Label::new(None);
    changelog_label.set_xalign(0.0);
    changelog_label.set_wrap(true);
    changelog_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    changelog_label.set_selectable(true);
    changelog_label.add_css_class("update-changelog-label");
    changelog_scroll.set_child(Some(&changelog_label));
    root.append(&changelog_scroll);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let action_button = gtk::Button::with_label("Update");
    action_button.add_css_class("app-flat-button");
    action_button.add_css_class("actions-add-button");
    footer.append(&action_button);
    root.append(&footer);

    popover.set_child(Some(&root));

    let sync_ui: Rc<dyn Fn(UpdateState)> = {
        let button = button.clone();
        let icon = icon.clone();
        let label = label.clone();
        let dot = dot.clone();
        let title = title.clone();
        let summary = summary.clone();
        let status = status.clone();
        let changelog_heading = changelog_heading.clone();
        let changelog_scroll = changelog_scroll.clone();
        let changelog_label = changelog_label.clone();
        let action_button = action_button.clone();
        Rc::new(move |state: UpdateState| match &state {
            UpdateState::Unsupported | UpdateState::Idle | UpdateState::Checking => {
                button.set_visible(false);
                label.set_text("Update");
                dot.set_visible(false);
                icon.set_icon_name(Some("folder-download-symbolic"));
                title.set_text("AppImage updates");
                summary.set_text("No updates available.");
                status.set_text("");
                changelog_heading.set_visible(false);
                changelog_scroll.set_visible(false);
                changelog_label.set_text("");
                action_button.set_visible(false);
                action_button.set_sensitive(false);
            }
            UpdateState::Available(release) => {
                button.set_visible(true);
                label.set_text("Update");
                dot.set_visible(true);
                icon.set_icon_name(Some("folder-download-symbolic"));
                title.set_text("Update available");
                summary.set_text(&format!(
                    "{} is available. You are on {}.",
                    release.title, APP_VERSION
                ));
                status.set_text(
                    release
                        .published_at
                        .as_deref()
                        .map(|value| format!("Published {}", value))
                        .unwrap_or_default()
                        .as_str(),
                );
                changelog_heading.set_visible(true);
                changelog_scroll.set_visible(true);
                changelog_label.set_text(if release.body.trim().is_empty() {
                    "No changelog was included in the GitHub release."
                } else {
                    &release.body
                });
                action_button.set_visible(true);
                action_button.set_label("Update");
                action_button.set_sensitive(true);
            }
            UpdateState::Updating(release) => {
                button.set_visible(true);
                label.set_text("Updating");
                dot.set_visible(false);
                icon.set_icon_name(Some("view-refresh-symbolic"));
                title.set_text("Updating AppImage");
                summary.set_text(&format!(
                    "Downloading and applying Enzim Coder {}.",
                    release.version
                ));
                status.set_text("The updated AppImage is being written in place.");
                changelog_heading.set_visible(true);
                changelog_scroll.set_visible(true);
                changelog_label.set_text(if release.body.trim().is_empty() {
                    "No changelog was included in the GitHub release."
                } else {
                    &release.body
                });
                action_button.set_visible(true);
                action_button.set_label("Updating...");
                action_button.set_sensitive(false);
            }
            UpdateState::ReadyToRestart(release) => {
                button.set_visible(true);
                label.set_text("Restart");
                dot.set_visible(false);
                icon.set_icon_name(Some("view-refresh-symbolic"));
                title.set_text("Restart to apply update");
                summary.set_text(&format!(
                    "Enzim Coder {} has been installed.",
                    release.version
                ));
                status.set_text("Restart the app to launch the new AppImage.");
                changelog_heading.set_visible(true);
                changelog_scroll.set_visible(true);
                changelog_label.set_text(if release.body.trim().is_empty() {
                    "No changelog was included in the GitHub release."
                } else {
                    &release.body
                });
                action_button.set_visible(true);
                action_button.set_label("Restart to Apply");
                action_button.set_sensitive(true);
            }
            UpdateState::Error { release, message } => {
                button.set_visible(true);
                label.set_text("Update");
                dot.set_visible(true);
                icon.set_icon_name(Some("dialog-warning-symbolic"));
                title.set_text("Update failed");
                summary.set_text(
                    release
                        .as_ref()
                        .map(|info| {
                            format!(
                                "Enzim Coder {} is available, but the update did not complete.",
                                info.version
                            )
                        })
                        .unwrap_or_else(|| {
                            "Automatic AppImage updates are unavailable.".to_string()
                        })
                        .as_str(),
                );
                status.set_text(message);
                let body = release
                    .as_ref()
                    .map(|info| info.body.as_str())
                    .unwrap_or("No changelog was included in the GitHub release.");
                let body = if body.trim().is_empty() {
                    "No changelog was included in the GitHub release."
                } else {
                    body
                };
                changelog_heading.set_visible(release.is_some());
                changelog_scroll.set_visible(release.is_some());
                changelog_label.set_text(body);
                action_button.set_visible(release.is_some());
                action_button.set_label("Retry Update");
                action_button.set_sensitive(release.is_some());
            }
        })
    };

    coordinator.subscribe({
        let sync_ui = sync_ui.clone();
        move |state| sync_ui(state)
    });
    sync_ui(coordinator.snapshot());

    {
        let coordinator = coordinator.clone();
        action_button.connect_clicked(move |_| match coordinator.snapshot() {
            UpdateState::Available(_) | UpdateState::Error { .. } => coordinator.start_update(),
            UpdateState::ReadyToRestart(_) => coordinator.restart_to_apply(),
            UpdateState::Unsupported
            | UpdateState::Idle
            | UpdateState::Checking
            | UpdateState::Updating(_) => {}
        });
    }

    {
        let button = button.clone();
        let popover = popover.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if popover.is_visible() {
                popover.popdown();
            } else {
                popover.popup();
            }
        });
        button.add_controller(click);
    }

    {
        let button = button.clone();
        popover.connect_visible_notify(move |popover| {
            if popover.is_visible() {
                button.add_css_class("is-active");
            } else {
                button.remove_css_class("is-active");
            }
        });
    }

    button
}

impl UpdateCoordinator {
    fn new() -> Rc<Self> {
        let (tx, rx) = mpsc::channel::<WorkerMessage>();
        let appimage_path = current_appimage_path();
        let initial_state = if appimage_path.is_some() {
            UpdateState::Idle
        } else {
            UpdateState::Unsupported
        };
        let coordinator = Rc::new(Self {
            state: RefCell::new(initial_state),
            tx,
            check_in_flight: Cell::new(false),
            update_in_flight: Cell::new(false),
            appimage_path,
            listeners: RefCell::new(Vec::new()),
        });

        {
            let weak = Rc::downgrade(&coordinator);
            glib::timeout_add_local(Duration::from_millis(120), move || {
                let Some(coordinator) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                while let Ok(message) = rx.try_recv() {
                    coordinator.handle_worker_message(message);
                }
                glib::ControlFlow::Continue
            });
        }

        if coordinator.appimage_path.is_some() {
            coordinator.request_check();
            let weak = Rc::downgrade(&coordinator);
            glib::timeout_add_local(CHECK_INTERVAL, move || {
                let Some(coordinator) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                coordinator.request_check();
                glib::ControlFlow::Continue
            });
        }

        coordinator
    }

    fn snapshot(&self) -> UpdateState {
        self.state.borrow().clone()
    }

    fn subscribe<F>(&self, listener: F)
    where
        F: Fn(UpdateState) + 'static,
    {
        self.listeners.borrow_mut().push(Box::new(listener));
    }

    fn set_state(&self, new_state: UpdateState) {
        self.state.replace(new_state.clone());
        for listener in self.listeners.borrow().iter() {
            listener(new_state.clone());
        }
    }

    fn request_check(&self) {
        if self.appimage_path.is_none() || self.check_in_flight.get() || self.update_in_flight.get()
        {
            return;
        }
        if matches!(self.snapshot(), UpdateState::ReadyToRestart(_)) {
            return;
        }

        self.check_in_flight.set(true);
        if !matches!(
            self.snapshot(),
            UpdateState::Available(_) | UpdateState::Updating(_) | UpdateState::Error { .. }
        ) {
            self.set_state(UpdateState::Checking);
        }
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(WorkerMessage::CheckFinished(fetch_latest_release()));
        });
    }

    fn start_update(&self) {
        if self.update_in_flight.get() {
            return;
        }
        let Some(appimage_path) = self.appimage_path.clone() else {
            self.set_state(UpdateState::Error {
                release: None,
                message: "This build is not running from an AppImage.".to_string(),
            });
            return;
        };

        let release = match self.snapshot() {
            UpdateState::Available(release) => release,
            UpdateState::Error {
                release: Some(release),
                ..
            } => release,
            UpdateState::ReadyToRestart(_) => {
                self.restart_to_apply();
                return;
            }
            UpdateState::Unsupported
            | UpdateState::Idle
            | UpdateState::Checking
            | UpdateState::Updating(_)
            | UpdateState::Error { release: None, .. } => return,
        };

        self.update_in_flight.set(true);
        self.set_state(UpdateState::Updating(release.clone()));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = perform_update(&appimage_path, &release).map(|_| release);
            let _ = tx.send(WorkerMessage::UpdateFinished(result));
        });
    }

    fn restart_to_apply(&self) {
        let Some(appimage_path) = self.appimage_path.clone() else {
            return;
        };
        let mut command = Command::new(&appimage_path);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(_) => {
                if let Some(app) = gtk::gio::Application::default() {
                    app.quit();
                }
            }
            Err(err) => {
                let release = self.snapshot().release().cloned();
                self.set_state(UpdateState::Error {
                    release,
                    message: format!("Updated AppImage is ready, but restarting failed: {err}"),
                });
            }
        }
    }

    fn handle_worker_message(&self, message: WorkerMessage) {
        match message {
            WorkerMessage::CheckFinished(result) => {
                self.check_in_flight.set(false);
                match result {
                    Ok(Some(release)) => {
                        if self.update_in_flight.get() {
                            return;
                        }
                        self.set_state(UpdateState::Available(release));
                    }
                    Ok(None) => {
                        if !self.update_in_flight.get() {
                            self.set_state(UpdateState::Idle);
                        }
                    }
                    Err(err) => {
                        if !self.update_in_flight.get() {
                            let release = self.snapshot().release().cloned();
                            if let Some(release) = release {
                                self.set_state(UpdateState::Error {
                                    release: Some(release),
                                    message: format!("Failed to check GitHub releases: {err}"),
                                });
                            } else {
                                self.set_state(UpdateState::Idle);
                            }
                        }
                    }
                }
            }
            WorkerMessage::UpdateFinished(result) => {
                self.update_in_flight.set(false);
                match result {
                    Ok(release) => self.set_state(UpdateState::ReadyToRestart(release)),
                    Err(err) => {
                        let release = self.snapshot().release().cloned();
                        self.set_state(UpdateState::Error {
                            release,
                            message: err,
                        });
                    }
                }
            }
        }
    }
}

fn coordinator() -> Rc<UpdateCoordinator> {
    UPDATE_COORDINATOR.with(|slot| {
        if slot.borrow().is_none() {
            slot.replace(Some(UpdateCoordinator::new()));
        }
        slot.borrow().as_ref().cloned().expect("update coordinator")
    })
}

fn current_appimage_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn fetch_latest_release() -> Result<Option<ReleaseInfo>, String> {
    let client = github_client()?;
    let response = client
        .get(RELEASES_LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .map_err(|err| err.to_string())?
        .error_for_status()
        .map_err(|err| err.to_string())?;
    let json = response.json::<Value>().map_err(|err| err.to_string())?;

    let release = parse_release(&json)?;
    let ordering = compare_versions(&release.version, APP_VERSION);
    if ordering != Ordering::Greater {
        return Ok(None);
    }

    Ok(Some(release))
}

fn github_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(format!("EnzimCoder/{APP_VERSION}"))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())
}

fn parse_release(json: &Value) -> Result<ReleaseInfo, String> {
    let tag_name = json
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "GitHub release payload is missing tag_name".to_string())?;
    let version = normalize_version(tag_name);
    if version.is_empty() {
        return Err(format!("Unsupported release tag format: {tag_name}"));
    }

    let assets = json
        .get("assets")
        .and_then(Value::as_array)
        .ok_or_else(|| "GitHub release payload is missing assets".to_string())?;

    let mut appimage_asset = None;
    for asset in assets {
        let Some(name) = asset.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(download_url) = asset.get("browser_download_url").and_then(Value::as_str) else {
            continue;
        };
        let parsed = ReleaseAsset {
            browser_download_url: download_url.to_string(),
        };
        if name.ends_with(".AppImage") && name.contains(ARCH_ASSET_TOKEN) {
            appimage_asset = Some(parsed.clone());
        }
    }

    let appimage_asset = appimage_asset
        .ok_or_else(|| "Latest release does not contain an x86_64 AppImage asset".to_string())?;

    Ok(ReleaseInfo {
        version,
        title: json
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(tag_name)
            .to_string(),
        body: json
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        published_at: json
            .get("published_at")
            .and_then(Value::as_str)
            .map(|value| value.to_string()),
        appimage_asset,
    })
}

fn normalize_version(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed.trim_start_matches(|ch: char| !ch.is_ascii_digit());
    let mut normalized = String::new();
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+') {
            normalized.push(ch);
        } else {
            break;
        }
    }
    normalized
}

fn parse_version_components(raw: &str) -> Vec<u64> {
    let core = raw.split(['-', '+']).next().unwrap_or(raw).trim();
    core.split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(|ch| ch.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

fn compare_versions(lhs: &str, rhs: &str) -> Ordering {
    let left = parse_version_components(lhs);
    let right = parse_version_components(rhs);
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        let left_value = *left.get(index).unwrap_or(&0);
        let right_value = *right.get(index).unwrap_or(&0);
        match left_value.cmp(&right_value) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

fn perform_update(appimage_path: &Path, release: &ReleaseInfo) -> Result<(), String> {
    if let Some(tool) = find_update_tool() {
        let status = Command::new(&tool)
            .arg(appimage_path)
            .status()
            .map_err(|err| format!("Failed to start {}: {err}", tool.display()))?;
        if status.success() {
            return Ok(());
        }
    }

    replace_with_download(appimage_path, &release.appimage_asset.browser_download_url)
}

fn find_update_tool() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for candidate in ["appimageupdatetool", "AppImageUpdate"] {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn replace_with_download(appimage_path: &Path, download_url: &str) -> Result<(), String> {
    let parent = appimage_path
        .parent()
        .ok_or_else(|| "AppImage path has no parent directory".to_string())?;
    let file_name = appimage_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Invalid AppImage filename".to_string())?;
    let temp_path = parent.join(format!(".{file_name}.download"));

    let client = Client::builder()
        .user_agent(format!("EnzimCoder/{APP_VERSION}"))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|err| err.to_string())?;
    let mut response = client
        .get(download_url)
        .send()
        .map_err(|err| err.to_string())?
        .error_for_status()
        .map_err(|err| err.to_string())?;

    let mut output = File::create(&temp_path)
        .map_err(|err| format!("Failed to create {}: {err}", temp_path.display()))?;
    io::copy(&mut response, &mut output)
        .map_err(|err| format!("Failed to write {}: {err}", temp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("Failed to chmod {}: {err}", temp_path.display()))?;
    }

    fs::rename(&temp_path, appimage_path).map_err(|err| {
        format!(
            "Failed to replace {} with downloaded update: {err}",
            appimage_path.display()
        )
    })?;

    Ok(())
}
