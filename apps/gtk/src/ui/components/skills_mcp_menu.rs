use crate::services::app::CodexProfileManager;
use crate::services::app::chat::AppDb;
use crate::services::app::runtime::McpServerInfo;
use crate::services::app::runtime::RuntimeClient;
use crate::services::app::skills::{
    PolicyKind, ProfileAssignments, SkillMcpCatalog, load_catalog, load_profile_assignments,
    set_profile_assigned, write_skill_assignment_for_profile,
};
use crate::ui::components::settings_dialog::{self, SettingsPage};
use crate::ui::components::skills_mcp_reload_guard::run_with_opencode_reload_guard;
use gtk::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Skill,
    Mcp,
}

#[derive(Clone, Debug)]
struct PopupEntry {
    kind: EntryKind,
    key: String,
    name: String,
    description: String,
    assigned: bool,
    auth_label: Option<String>,
    authenticated: Option<bool>,
}

#[derive(Clone, Debug)]
struct PopupData {
    profile_id: i64,
    profile_backend_kind: String,
    profile_name: String,
    profile_running: bool,
    entries: Vec<PopupEntry>,
    warning: Option<String>,
}

fn normalize_mcp_server_name(input: &str) -> String {
    let mut out = String::new();
    for ch in input.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '-' || lower == '_' {
            out.push(lower);
        } else if (lower.is_whitespace() || lower == '.') && !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "mcp_server".to_string()
    } else {
        trimmed
    }
}

fn set_skill_for_profile(
    profile_home: &str,
    slug: &str,
    content: &str,
    enabled: bool,
    client: Arc<RuntimeClient>,
) -> Result<(), String> {
    let profile = crate::services::app::chat::CodexProfileRecord {
        id: 0,
        backend_kind: client.backend_kind().to_string(),
        name: String::new(),
        icon_name: String::new(),
        home_dir: profile_home.to_string(),
        last_account_type: None,
        last_email: None,
        status: String::new(),
        created_at: 0,
        updated_at: 0,
    };
    write_skill_assignment_for_profile(&profile, slug, content, enabled)?;

    let _ = client.skills_list(&[], true);

    Ok(())
}

fn set_mcp_for_profile(
    name: &str,
    config: Value,
    enabled: bool,
    client: Arc<RuntimeClient>,
) -> Result<(), String> {
    let key_path = format!("mcp_servers.{}", normalize_mcp_server_name(name));
    if enabled {
        client.config_batch_write(vec![(key_path, config, "upsert".to_string())])?;
    } else {
        client.config_value_write(&key_path, Value::Null, "replace")?;
    }
    client.config_mcp_server_reload()?;
    Ok(())
}

fn mcp_status_map(items: &[McpServerInfo]) -> HashMap<String, (String, bool)> {
    items
        .iter()
        .map(|item| {
            (
                normalize_mcp_server_name(&item.name),
                (item.auth_label.clone(), item.authenticated),
            )
        })
        .collect::<HashMap<_, _>>()
}

fn build_entries(
    catalog: &SkillMcpCatalog,
    assignments: &ProfileAssignments,
    mcp_status: &[McpServerInfo],
) -> Vec<PopupEntry> {
    let mut out = Vec::<PopupEntry>::new();

    for skill in &catalog.skills {
        out.push(PopupEntry {
            kind: EntryKind::Skill,
            key: skill.key.clone(),
            name: skill.name.clone(),
            description: skill.description.clone(),
            assigned: assignments.skills.contains(&skill.key),
            auth_label: None,
            authenticated: None,
        });
    }

    let auth_map = mcp_status_map(mcp_status);
    for mcp in &catalog.mcps {
        let auth = auth_map
            .get(&normalize_mcp_server_name(&mcp.name))
            .cloned()
            .unwrap_or(("Unknown".to_string(), false));
        out.push(PopupEntry {
            kind: EntryKind::Mcp,
            key: mcp.key.clone(),
            name: mcp.name.clone(),
            description: mcp.description.clone(),
            assigned: assignments.mcps.contains(&mcp.key),
            auth_label: Some(auth.0),
            authenticated: Some(auth.1),
        });
    }

    out.sort_by(|a, b| {
        let kind_a = if a.kind == EntryKind::Skill { 0 } else { 1 };
        let kind_b = if b.kind == EntryKind::Skill { 0 } else { 1 };
        kind_a.cmp(&kind_b).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    out
}

fn fetch_popup_data(profile_id: i64, client: Option<Arc<RuntimeClient>>) -> PopupData {
    let background_db = AppDb::open_default();
    let profile = background_db.get_codex_profile(profile_id).ok().flatten();
    let profile_name = profile
        .as_ref()
        .map(|profile| profile.name.clone())
        .unwrap_or_else(|| format!("Profile #{profile_id}"));
    let profile_backend_kind = profile
        .as_ref()
        .map(|profile| profile.backend_kind.clone())
        .unwrap_or_else(|| "codex".to_string());
    let profile_running = profile
        .as_ref()
        .map(|profile| profile.status.eq_ignore_ascii_case("running"))
        .unwrap_or(false);

    let catalog = load_catalog(background_db.as_ref());
    let assignments = load_profile_assignments(background_db.as_ref(), profile_id);

    let mut warning = None;
    let mut mcp_status = Vec::<McpServerInfo>::new();
    if let Some(client) = client {
        match client.mcp_server_status_list(100) {
            Ok(items) => mcp_status = items,
            Err(err) => warning = Some(format!("mcpServerStatus/list failed: {err}")),
        }
    }

    PopupData {
        profile_id,
        profile_backend_kind,
        profile_name,
        profile_running,
        entries: build_entries(&catalog, &assignments, &mcp_status),
        warning,
    }
}

pub fn build_skills_mcp_button(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    _active_workspace_path: Rc<RefCell<Option<String>>>,
    compact: bool,
) -> gtk::Box {
    let button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    button.add_css_class("actions-toggle-button");
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    button.set_can_focus(false);
    if compact {
        button.add_css_class("multi-chat-pane-skills-mcp-button");
    } else {
        button.add_css_class("topbar-skills-mcp-button");
    }

    let icon = gtk::Image::from_icon_name("3d-box-symbolic");
    icon.set_pixel_size(14);
    icon.set_hexpand(true);
    icon.set_halign(gtk::Align::Center);
    button.append(&icon);
    button.set_tooltip_text(Some("Skills & MCP"));

    let popover = gtk::Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_parent(&button);
    popover.add_css_class("actions-popover");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_margin_start(8);
    root.set_margin_end(8);
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_size_request(430, -1);
    root.add_css_class("actions-popover-root");

    let title = gtk::Label::new(Some("Skills & MCP"));
    title.add_css_class("actions-popover-title");
    title.set_xalign(0.0);
    root.append(&title);

    let profile_label = gtk::Label::new(Some("Profile: -"));
    profile_label.add_css_class("actions-popover-workspace");
    profile_label.set_xalign(0.0);
    root.append(&profile_label);

    let status_label = gtk::Label::new(Some(""));
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_label.add_css_class("actions-popover-status");
    root.append(&status_label);

    let info_label = gtk::Label::new(Some(
        "Toggle assignment for the active profile. Stopped profiles are read-only in Settings.",
    ));
    info_label.set_xalign(0.0);
    info_label.set_wrap(true);
    info_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    info_label.add_css_class("actions-popover-status");
    root.append(&info_label);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(220)
        .build();
    scroll.set_has_frame(false);

    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    scroll.set_child(Some(&list_box));
    root.append(&scroll);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let refresh_button = gtk::Button::with_label("Refresh");
    refresh_button.add_css_class("app-flat-button");
    refresh_button.add_css_class("actions-add-button");
    let add_button = gtk::Button::with_label("Add Skill / MCP");
    add_button.add_css_class("app-flat-button");
    add_button.add_css_class("actions-add-button");
    footer.append(&refresh_button);
    footer.append(&add_button);
    root.append(&footer);

    popover.set_child(Some(&root));

    let refresh_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let refresh_fn: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let popover = popover.clone();
        let profile_label = profile_label.clone();
        let status_label = status_label.clone();
        let list_box = list_box.clone();
        let refresh_handle = refresh_handle.clone();
        Rc::new(move || {
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }

            let profile_id = db.runtime_profile_id().ok().flatten().unwrap_or(1);
            let client = manager.running_client_for_profile(profile_id);
            status_label.set_text("Loading Skills/MCP...");

            let (tx, rx) = mpsc::channel::<PopupData>();
            thread::spawn(move || {
                let data = fetch_popup_data(profile_id, client);
                let _ = tx.send(data);
            });

            let list_box = list_box.clone();
            let status_label = status_label.clone();
            let profile_label = profile_label.clone();
            let refresh_handle_for_rows = refresh_handle.clone();
            let manager_for_rows = manager.clone();
            let popover_for_rows = popover.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(data) => {
                    while let Some(child) = list_box.first_child() {
                        list_box.remove(&child);
                    }

                    profile_label.set_text(&format!("Profile: {}", data.profile_name));

                    if data.entries.is_empty() {
                        let empty = gtk::Label::new(Some("No Skills/MCP entries in catalog yet."));
                        empty.set_xalign(0.0);
                        empty.add_css_class("dim-label");
                        list_box.append(&empty);
                    }

                    for entry in data.entries {
                        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                        row.add_css_class("actions-command-card");

                        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        let check = gtk::CheckButton::new();
                        check.set_active(entry.assigned);
                        let skill_supported = entry.kind != EntryKind::Skill
                            || crate::services::app::runtime::capabilities_for_backend_kind(
                                &data.profile_backend_kind,
                            )
                            .supports_skill_assignment;
                        check.set_sensitive(data.profile_running && skill_supported);
                        if !data.profile_running {
                            check.set_tooltip_text(Some(
                                "Profile is stopped. Start it to change assignments.",
                            ));
                        } else if !skill_supported {
                            check.set_tooltip_text(Some(
                                "Skill assignment is not supported for this runtime profile.",
                            ));
                        }
                        top.append(&check);

                        let name = gtk::Label::new(Some(&entry.name));
                        name.set_xalign(0.0);
                        name.set_hexpand(true);
                        name.add_css_class("actions-command-title");
                        top.append(&name);

                        let kind_badge = gtk::Label::new(Some(match entry.kind {
                            EntryKind::Skill => "Skill",
                            EntryKind::Mcp => "MCP",
                        }));
                        kind_badge.add_css_class("actions-run-status");
                        top.append(&kind_badge);

                        if entry.kind == EntryKind::Mcp {
                            if let Some(auth_label) = entry.auth_label.clone() {
                                let auth_chip = gtk::Label::new(Some(&auth_label));
                                auth_chip.add_css_class("actions-run-status");
                                top.append(&auth_chip);
                            }

                            if entry.assigned && entry.authenticated == Some(false) {
                                let auth_button = gtk::Button::with_label("Auth");
                                auth_button.add_css_class("app-flat-button");
                                auth_button.add_css_class("actions-run-button");
                                let status_label = status_label.clone();
                                let server_name = entry.name.clone();
                                let client =
                                    manager_for_rows.running_client_for_profile(data.profile_id);
                                let refresh_handle_for_auth = refresh_handle_for_rows.clone();
                                auth_button.connect_clicked(move |_| {
                                    let Some(client) = client.clone() else {
                                        status_label
                                            .set_text("Runtime profile is not running.");
                                        return;
                                    };
                                    status_label.set_text("Starting MCP OAuth login...");
                                    let (tx, rx) = mpsc::channel::<Result<String, String>>();
                                    let server_name_for_thread = server_name.clone();
                                    thread::spawn(move || {
                                        let _ = tx.send(
                                            client.mcp_server_oauth_login(&server_name_for_thread),
                                        );
                                    });
                                    let status_label = status_label.clone();
                                    let refresh_handle_for_auth = refresh_handle_for_auth.clone();
                                    gtk::glib::timeout_add_local(
                                        Duration::from_millis(60),
                                        move || match rx.try_recv() {
                                            Ok(Ok(url)) => {
                                                if let Some(display) = gtk::gdk::Display::default() {
                                                    display.clipboard().set_text(&url);
                                                }
                                                status_label.set_text(
                                                    "OAuth URL copied to clipboard. Complete login, then refresh.",
                                                );
                                                if let Some(refresh) =
                                                    refresh_handle_for_auth.borrow().as_ref()
                                                {
                                                    refresh();
                                                }
                                                gtk::glib::ControlFlow::Break
                                            }
                                            Ok(Err(err)) => {
                                                status_label
                                                    .set_text(&format!("MCP OAuth failed: {err}"));
                                                gtk::glib::ControlFlow::Break
                                            }
                                            Err(mpsc::TryRecvError::Empty) => {
                                                gtk::glib::ControlFlow::Continue
                                            }
                                            Err(mpsc::TryRecvError::Disconnected) => {
                                                gtk::glib::ControlFlow::Break
                                            }
                                        },
                                    );
                                });
                                top.append(&auth_button);
                            }
                        }

                        let detail = entry.description.clone();
                        if !detail.trim().is_empty() {
                            let reveal_button = gtk::Button::new();
                            reveal_button.set_has_frame(false);
                            reveal_button.add_css_class("app-flat-button");
                            reveal_button.set_tooltip_text(Some("Show details"));
                            let reveal_icon = gtk::Image::from_icon_name("pan-end-symbolic");
                            reveal_icon.set_pixel_size(12);
                            reveal_button.set_child(Some(&reveal_icon));
                            top.append(&reveal_button);

                            let detail_revealer = gtk::Revealer::new();
                            detail_revealer.set_reveal_child(false);
                            detail_revealer
                                .set_transition_type(gtk::RevealerTransitionType::SlideDown);
                            detail_revealer.set_transition_duration(140);

                            let detail_label = gtk::Label::new(Some(&detail));
                            detail_label.set_xalign(0.0);
                            detail_label.set_wrap(true);
                            detail_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                            detail_label.add_css_class("actions-command-text");
                            detail_revealer.set_child(Some(&detail_label));

                            {
                                let detail_revealer = detail_revealer.clone();
                                let reveal_icon = reveal_icon.clone();
                                reveal_button.connect_clicked(move |_| {
                                    let reveal = !detail_revealer.reveals_child();
                                    detail_revealer.set_reveal_child(reveal);
                                    reveal_icon.set_icon_name(Some(if reveal {
                                        "pan-down-symbolic"
                                    } else {
                                        "pan-end-symbolic"
                                    }));
                                });
                            }

                            row.append(&top);
                            row.append(&detail_revealer);
                        } else {
                            row.append(&top);
                        }

                        {
                            let entry_kind = entry.kind;
                            let entry_key = entry.key.clone();
                            let entry_name = entry.name.clone();
                            let profile_id = data.profile_id;
                            let profile_backend_kind = data.profile_backend_kind.clone();
                            let parent_for_toggle = popover_for_rows
                                .root()
                                .and_then(|root| root.downcast::<gtk::Window>().ok());
                            let manager_for_toggle = manager_for_rows.clone();
                            let status_label_for_toggle = status_label.clone();
                            let refresh_handle_for_toggle = refresh_handle_for_rows.clone();
                            check.connect_toggled(move |toggle| {
                                let enabled = toggle.is_active();
                                let start_toggle: Rc<dyn Fn()> = {
                                    let manager_for_toggle = manager_for_toggle.clone();
                                    let status_label_for_toggle = status_label_for_toggle.clone();
                                    let refresh_handle_for_toggle =
                                        refresh_handle_for_toggle.clone();
                                    let entry_key_for_thread = entry_key.clone();
                                    let entry_name_for_thread = entry_name.clone();
                                    let profile_backend_kind_for_thread =
                                        profile_backend_kind.clone();
                                    let toggle = toggle.clone();
                                    Rc::new(move || {
                                        status_label_for_toggle.set_text("Applying assignment...");
                                        let client =
                                            manager_for_toggle.running_client_for_profile(profile_id);
                                        let (tx, rx) = mpsc::channel::<Result<(), String>>();
                                        let entry_key_for_thread =
                                            entry_key_for_thread.clone();
                                        let entry_name_for_thread =
                                            entry_name_for_thread.clone();
                                        let profile_backend_kind_for_thread =
                                            profile_backend_kind_for_thread.clone();
                                        thread::spawn(move || {
                                            let result = (|| -> Result<(), String> {
                                                let background_db = AppDb::open_default();
                                                let kind = match entry_kind {
                                                    EntryKind::Skill => PolicyKind::Skill,
                                                    EntryKind::Mcp => PolicyKind::Mcp,
                                                };
                                                if entry_kind == EntryKind::Skill
                                                    && !crate::services::app::runtime::capabilities_for_backend_kind(
                                                        &profile_backend_kind_for_thread,
                                                    )
                                                    .supports_skill_assignment
                                                {
                                                    return Err(
                                                        "Skill assignment is not supported for this runtime profile."
                                                            .to_string(),
                                                    );
                                                }
                                                set_profile_assigned(
                                                    background_db.as_ref(),
                                                    profile_id,
                                                    kind,
                                                    &entry_key_for_thread,
                                                    enabled,
                                                )?;

                                                let profile = background_db
                                                    .get_codex_profile(profile_id)
                                                    .map_err(|err| err.to_string())?
                                                    .ok_or_else(|| {
                                                        format!(
                                                            "profile {} not found",
                                                            profile_id
                                                        )
                                                    })?;

                                                let catalog = load_catalog(background_db.as_ref());
                                                match entry_kind {
                                                    EntryKind::Skill => {
                                                        let skill = catalog
                                                            .skills
                                                            .into_iter()
                                                            .find(|item| {
                                                                item.key == entry_key_for_thread
                                                            })
                                                            .ok_or_else(|| {
                                                                format!(
                                                                    "skill not found in catalog: {}",
                                                                    entry_key_for_thread
                                                                )
                                                            })?;
                                                        let Some(client) = client else {
                                                            return Err(
                                                                "Runtime profile is not running."
                                                                    .to_string(),
                                                            );
                                                        };
                                                        set_skill_for_profile(
                                                            &profile.home_dir,
                                                            &skill.slug,
                                                            &skill.content,
                                                            enabled,
                                                            client,
                                                        )
                                                    }
                                                    EntryKind::Mcp => {
                                                        let mcp = catalog
                                                            .mcps
                                                            .into_iter()
                                                            .find(|item| {
                                                                item.key == entry_key_for_thread
                                                            })
                                                            .ok_or_else(|| {
                                                                format!(
                                                                    "mcp not found in catalog: {}",
                                                                    entry_key_for_thread
                                                                )
                                                            })?;
                                                        let Some(client) = client else {
                                                            return Err(
                                                                "Runtime profile is not running."
                                                                    .to_string(),
                                                            );
                                                        };
                                                        set_mcp_for_profile(
                                                            &entry_name_for_thread,
                                                            mcp.config,
                                                            enabled,
                                                            client,
                                                        )
                                                    }
                                                }
                                            })();
                                            let _ = tx.send(result);
                                        });
                                        let status_label_for_toggle =
                                            status_label_for_toggle.clone();
                                        let refresh_handle_for_toggle =
                                            refresh_handle_for_toggle.clone();
                                        let toggle = toggle.clone();
                                        gtk::glib::timeout_add_local(
                                            Duration::from_millis(60),
                                            move || match rx.try_recv() {
                                                Ok(Ok(())) => {
                                                    status_label_for_toggle
                                                        .set_text("Assignment updated.");
                                                    if let Some(refresh) =
                                                        refresh_handle_for_toggle.borrow().as_ref()
                                                    {
                                                        refresh();
                                                    }
                                                    gtk::glib::ControlFlow::Break
                                                }
                                                Ok(Err(err)) => {
                                                    status_label_for_toggle
                                                        .set_text(&format!(
                                                            "Toggle failed: {err}"
                                                        ));
                                                    toggle.set_active(!enabled);
                                                    gtk::glib::ControlFlow::Break
                                                }
                                                Err(mpsc::TryRecvError::Empty) => {
                                                    gtk::glib::ControlFlow::Continue
                                                }
                                                Err(mpsc::TryRecvError::Disconnected) => {
                                                    gtk::glib::ControlFlow::Break
                                                }
                                            },
                                        );
                                    })
                                };
                                let status_label_for_cancel = status_label_for_toggle.clone();
                                let toggle_for_cancel = toggle.clone();
                                run_with_opencode_reload_guard(
                                    parent_for_toggle.as_ref(),
                                    manager_for_toggle.clone(),
                                    profile_id,
                                    status_label_for_toggle.clone(),
                                    start_toggle,
                                    Rc::new(move || {
                                        status_label_for_cancel
                                            .set_text("Assignment canceled.");
                                        toggle_for_cancel.set_active(!enabled);
                                    }),
                                );
                            });
                        }

                        list_box.append(&row);
                    }

                    if !data.profile_running {
                        status_label.set_text(
                            "Profile is stopped. Open Settings to assign when profile is running.",
                        );
                    } else if let Some(warning) = data.warning {
                        status_label.set_text(&warning);
                    } else {
                        status_label.set_text("");
                    }

                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    status_label.set_text("Failed to load Skills/MCP data.");
                    gtk::glib::ControlFlow::Break
                }
            });
        })
    };
    refresh_handle.replace(Some(refresh_fn.clone()));

    {
        let popover = popover.clone();
        let refresh_fn = refresh_fn.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            let is_open = popover.is_visible();
            if is_open {
                popover.popdown();
            } else {
                refresh_fn();
                popover.popup();
            }
        });
        button.add_controller(click);
    }

    {
        let refresh_fn = refresh_fn.clone();
        refresh_button.connect_clicked(move |_| refresh_fn());
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let popover = popover.clone();
        add_button.connect_clicked(move |_| {
            popover.popdown();
            let parent = popover
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            settings_dialog::show(
                parent.as_ref(),
                db.clone(),
                manager.clone(),
                SettingsPage::SkillsMcp,
            );
        });
    }

    {
        let button_for_controller = button.clone();
        let button_for_enter = button.clone();
        let button_for_leave = button.clone();
        let popover_for_leave = popover.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            button_for_enter.add_css_class("is-active");
        });
        motion.connect_leave(move |_| {
            if !popover_for_leave.is_visible() {
                button_for_leave.remove_css_class("is-active");
            }
        });
        button_for_controller.add_controller(motion);
    }

    {
        let button = button.clone();
        popover.connect_visible_notify(move |p| {
            if p.is_visible() {
                button.add_css_class("is-active");
            } else {
                button.remove_css_class("is-active");
            }
        });
    }

    button
}
