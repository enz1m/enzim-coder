use crate::services::app::CodexProfileManager;
use crate::services::app::chat::AppDb;
use adw::prelude::*;
use gtk::glib::object::ObjectExt;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod components;
mod content;
pub(crate) mod scheduler;
pub(crate) mod settings;
mod sidebar;
mod styles;
pub(crate) mod widget_tree;

pub fn install_css() {
    styles::install_css();
}

fn gtk_supports_backdrop_filter() -> bool {
    let major = gtk::major_version();
    let minor = gtk::minor_version();
    major > 4 || (major == 4 && minor >= 21)
}

fn pane_layout_has_saved_thread(db: &AppDb) -> bool {
    let Ok(Some(raw)) = db.get_setting(settings::SETTING_PANE_LAYOUT_V1) else {
        return false;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let has_saved_thread = parsed
        .get("panes")
        .and_then(serde_json::Value::as_array)
        .map(|panes| {
            panes.iter().any(|pane| {
                pane.get("threadId")
                    .or_else(|| pane.get("codexThreadId"))
                    .and_then(serde_json::Value::as_str)
                    .map(|id| !id.trim().is_empty())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if has_saved_thread {
        return true;
    }
    let has_pending_profile_thread = db
        .get_setting("pending_profile_thread_id")
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|thread_id| db.get_thread_record(thread_id).ok().flatten())
        .map(|thread| {
            thread
                .remote_thread_id()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
        })
        .unwrap_or(false);
    has_pending_profile_thread
        && parsed
            .get("panes")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|panes| !panes.is_empty())
}

fn start_account_sync_loop(db: Rc<AppDb>, manager: Rc<CodexProfileManager>) {
    let (tx, rx) = mpsc::channel::<(i64, Option<crate::services::app::runtime::AccountInfo>)>();
    let refresh_in_flight = Rc::new(RefCell::new(false));
    let pending_count = Rc::new(RefCell::new(0usize));

    {
        let db = db.clone();
        let refresh_in_flight = refresh_in_flight.clone();
        let pending_count = pending_count.clone();
        crate::ui::scheduler::every(Duration::from_millis(80), move || {
            while let Ok((profile_id, account)) = rx.try_recv() {
                let _ = crate::ui::components::runtime_auth_dialog::sync_runtime_account_to_db(
                    &db, profile_id, account,
                );
                let next = pending_count.borrow().saturating_sub(1);
                pending_count.replace(next);
                if next == 0 {
                    refresh_in_flight.replace(false);
                }
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    let refresh_in_flight_for_periodic = refresh_in_flight.clone();
    let pending_count_for_periodic = pending_count.clone();
    let tx_periodic = tx.clone();
    let manager_periodic = manager.clone();
    let db_periodic = db.clone();
    crate::ui::scheduler::every(Duration::from_millis(1800), move || {
        if *refresh_in_flight_for_periodic.borrow() {
            return gtk::glib::ControlFlow::Continue;
        }
        let Some(runtime_profile_id) = db_periodic.runtime_profile_id().ok().flatten() else {
            return gtk::glib::ControlFlow::Continue;
        };
        let Some(client) = manager_periodic.running_client_for_profile(runtime_profile_id) else {
            return gtk::glib::ControlFlow::Continue;
        };
        refresh_in_flight_for_periodic.replace(true);
        pending_count_for_periodic.replace(1);
        let tx = tx_periodic.clone();
        thread::spawn(move || {
            let _ = tx.send((
                runtime_profile_id,
                client.account_read(false).ok().flatten(),
            ));
        });
        gtk::glib::ControlFlow::Continue
    });
}

fn start_remote_thread_activation_loop(
    db: Rc<AppDb>,
    sidebar: adw::ToolbarView,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) {
    crate::ui::scheduler::every(Duration::from_millis(120), move || {
        let Some(raw_thread_id) = db
            .get_setting(crate::services::app::remote::SETTING_REMOTE_TELEGRAM_ACTIVATE_LOCAL_THREAD_ID)
            .ok()
            .flatten()
            .filter(|value| !value.trim().is_empty())
        else {
            return gtk::glib::ControlFlow::Continue;
        };
        let _ = db.set_setting(
            crate::services::app::remote::SETTING_REMOTE_TELEGRAM_ACTIVATE_LOCAL_THREAD_ID,
            "",
        );

        let Ok(local_thread_id) = raw_thread_id.parse::<i64>() else {
            return gtk::glib::ControlFlow::Continue;
        };
        let Some(thread) = db.get_thread_record(local_thread_id).ok().flatten() else {
            return gtk::glib::ControlFlow::Continue;
        };

        let _ = db.set_runtime_profile_id(thread.profile_id);
        let _ = db.set_active_profile_id(thread.profile_id);
        let _ = db.set_current_profile_account_identity(
            thread.remote_account_type(),
            thread.remote_account_email(),
        );

        let workspace_path = thread
            .worktree_path
            .as_deref()
            .map(str::trim)
            .filter(|path| thread.worktree_active && !path.is_empty())
            .map(|path| path.to_string())
            .or_else(|| {
                db.workspace_path_for_local_thread(local_thread_id)
                    .ok()
                    .flatten()
            });
        if let Some(path) = workspace_path {
            active_workspace_path.replace(Some(path.clone()));
            let _ = db.set_setting("last_active_workspace_path", &path);
        }
        let _ = db.set_setting("last_active_thread_id", &local_thread_id.to_string());

        let linked_thread_id = thread
            .remote_thread_id()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        if let Some(thread_id) = linked_thread_id {
            active_thread_id.replace(Some(thread_id));
            let _ = db.set_setting("pending_profile_thread_id", "");
        } else {
            active_thread_id.replace(None);
            let _ = db.set_setting("pending_profile_thread_id", &local_thread_id.to_string());
        }

        if let Some(sidebar_content) = sidebar.content() {
            let _ = widget_tree::select_thread_row(&sidebar_content, local_thread_id);
            let root_widget: gtk::Widget = sidebar_content
                .root()
                .map(|root| root.upcast())
                .unwrap_or(sidebar_content);
            if let Some(stack_widget) =
                widget_tree::find_widget_by_name(&root_widget, "main-content-view-stack")
            {
                if let Ok(stack) = stack_widget.downcast::<adw::ViewStack>() {
                    stack.set_visible_child_name("chat");
                }
            }
        }

        gtk::glib::ControlFlow::Continue
    });
}

fn schedule_startup_memory_trim(window: &adw::ApplicationWindow) {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let window_weak = window.downgrade();
        for delay_ms in [9500u64] {
            let window_weak = window_weak.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                if !window.is_visible() {
                    return;
                }
                unsafe {
                    libc::malloc_trim(0);
                }
            });
        }
    }
}

fn schedule_periodic_idle_memory_trim(window: &adw::ApplicationWindow) {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let window_weak = window.downgrade();
        gtk::glib::timeout_add_local(Duration::from_secs(300), move || {
            let Some(window) = window_weak.upgrade() else {
                return gtk::glib::ControlFlow::Break;
            };
            if !window.is_visible() {
                return gtk::glib::ControlFlow::Continue;
            }
            if crate::ui::components::chat::has_any_active_turn() {
                return gtk::glib::ControlFlow::Continue;
            }
            unsafe {
                libc::malloc_trim(0);
            }
            gtk::glib::ControlFlow::Continue
        });
    }
}

pub fn build_ui(app: &adw::Application) {
    if let Some(existing_window) = app
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<adw::ApplicationWindow>().ok())
    {
        existing_window.present();
        return;
    }

    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::Default);
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_overlay_scrolling(false);
    }

    let app_data_dir = crate::services::app::chat::default_app_data_dir();
    let fresh_start = !app_data_dir.join("enzimcoder.db").exists();
    let db = AppDb::open_default();
    let _ = db.delete_open_threads_without_turns();
    if db.remote_telegram_active_account().ok().flatten().is_some() {
        crate::services::app::remote::start_background_worker();
    }
    crate::services::app::restore::init(&db);

    components::style_picker::initialize_theme(&db);
    crate::ui::components::chat::runtime_controls::preload_opencode_hidden_model_cache(&db);

    let _ = db.ensure_default_codex_profile(&app_data_dir);
    let profile_manager = Rc::new(CodexProfileManager::new(db.clone()));
    {
        let profile_manager = profile_manager.clone();
        app.connect_shutdown(move |_| {
            profile_manager.shutdown_all();
        });
    }
    let runtime_profile_id = db.active_profile_id().ok().flatten().unwrap_or(1);
    let _ = db.set_runtime_profile_id(runtime_profile_id);
    let codex = profile_manager.ensure_started(runtime_profile_id).ok();
    if let Some(client) = codex.as_ref() {
        crate::ui::components::chat::runtime_controls::refresh_model_options_cache_async(Some(
            client.clone(),
        ));
    }
    let launch_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let _ = db.set_setting("last_launch_at", &launch_ts);
    {
        let to_start: VecDeque<i64> = db
            .list_codex_profiles()
            .unwrap_or_default()
            .into_iter()
            .filter(|profile| {
                profile.status.eq_ignore_ascii_case("running") && profile.id != runtime_profile_id
            })
            .map(|profile| profile.id)
            .collect();
        let startup_queue = Rc::new(RefCell::new(to_start));
        let manager_startup = profile_manager.clone();
        let db_startup = db.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(120), move || {
            let Some(profile_id) = startup_queue.borrow_mut().pop_front() else {
                return gtk::glib::ControlFlow::Break;
            };
            if manager_startup.ensure_started(profile_id).is_err() {
                let _ = db_startup.update_codex_profile_status(profile_id, "stopped");
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    start_account_sync_loop(db.clone(), profile_manager.clone());
    let last_workspace_path = db
        .get_setting("last_active_workspace_path")
        .ok()
        .flatten()
        .or_else(|| db.get_setting("last_workspace_path").ok().flatten())
        .filter(|path| {
            let trimmed = path.trim();
            !trimmed.is_empty() && Path::new(trimmed).is_dir()
        });

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Enzim Coder")
        .default_width(1200)
        .default_height(780)
        .build();
    window.set_size_request(980, 620);
    if !gtk_supports_backdrop_filter() {
        window.add_css_class("no-backdrop-blur");
    }

    let active_thread_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let active_workspace_path: Rc<RefCell<Option<String>>> =
        Rc::new(RefCell::new(last_workspace_path));

    let sidebar = sidebar::build_sidebar(
        &window,
        db.clone(),
        profile_manager.clone(),
        active_thread_id.clone(),
        active_workspace_path.clone(),
    );
    sidebar.set_width_request(sidebar::SIDEBAR_WIDTH);
    sidebar.set_size_request(sidebar::SIDEBAR_WIDTH, -1);

    let content = content::build_content(
        db.clone(),
        profile_manager.clone(),
        codex.clone(),
        active_thread_id.clone(),
        active_workspace_path.clone(),
    );

    let main_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    main_container.add_css_class("main-container");

    let sidebar_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_container.add_css_class("sidebar-host");
    sidebar_container.set_hexpand(false);
    sidebar_container.set_halign(gtk::Align::Start);
    sidebar_container.set_width_request(sidebar::SIDEBAR_WIDTH);
    sidebar_container.set_size_request(sidebar::SIDEBAR_WIDTH, -1);
    sidebar_container.append(&sidebar);

    content.set_hexpand(true);

    main_container.append(&sidebar_container);
    main_container.append(&content);

    let root_overlay = gtk::Overlay::new();
    if !gtk_supports_backdrop_filter() {
        root_overlay.add_css_class("no-backdrop-blur");
    }
    root_overlay.set_child(Some(&main_container));

    let remote_overlay = gtk::Box::new(gtk::Orientation::Vertical, 0);
    remote_overlay.add_css_class("remote-mode-overlay");
    remote_overlay.set_hexpand(true);
    remote_overlay.set_vexpand(true);
    remote_overlay.set_halign(gtk::Align::Fill);
    remote_overlay.set_valign(gtk::Align::Fill);

    let remote_center = gtk::Box::new(gtk::Orientation::Vertical, 16);
    remote_center.add_css_class("remote-mode-overlay-content");
    remote_center.set_halign(gtk::Align::Center);
    remote_center.set_valign(gtk::Align::Center);
    remote_center.set_hexpand(true);
    remote_center.set_vexpand(true);

    let remote_title = gtk::Label::new(Some("Remote mode is On"));
    remote_title.add_css_class("remote-mode-overlay-title");
    remote_center.append(&remote_title);

    let remote_hint = gtk::Label::new(Some("Forwarding assistant updates to Telegram."));
    remote_hint.add_css_class("remote-mode-overlay-hint");
    remote_hint.set_xalign(0.5);
    remote_center.append(&remote_hint);

    let remote_close = gtk::Button::new();
    let remote_close_glyph = gtk::Label::new(Some("X"));
    remote_close_glyph.add_css_class("remote-mode-overlay-close-glyph");
    remote_close_glyph.set_xalign(0.5);
    remote_close_glyph.set_yalign(0.5);
    remote_close_glyph.set_halign(gtk::Align::Center);
    remote_close_glyph.set_valign(gtk::Align::Center);
    remote_close.set_child(Some(&remote_close_glyph));
    remote_close.set_has_frame(true);
    remote_close.set_halign(gtk::Align::Center);
    remote_close.set_valign(gtk::Align::Center);
    remote_close.set_hexpand(false);
    remote_close.set_vexpand(false);
    remote_close.set_size_request(62, 62);
    remote_close.set_tooltip_text(Some("Turn off remote mode"));
    remote_close.add_css_class("circular");
    remote_close.add_css_class("remote-mode-overlay-close");
    {
        let db = db.clone();
        remote_close.connect_clicked(move |_| {
            let _ = db.set_remote_mode_enabled(false);
        });
    }
    remote_center.append(&remote_close);
    remote_overlay.append(&remote_center);
    remote_overlay.set_visible(db.remote_mode_enabled());
    root_overlay.add_overlay(&remote_overlay);

    if fresh_start {
        components::welcome_overlay::attach(
            &root_overlay,
            db.clone(),
            profile_manager.clone(),
            runtime_profile_id,
        );
    }

    {
        let db = db.clone();
        let remote_overlay = remote_overlay.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(130), move || {
            remote_overlay.set_visible(db.remote_mode_enabled());
            gtk::glib::ControlFlow::Continue
        });
    }

    start_remote_thread_activation_loop(
        db.clone(),
        sidebar.clone(),
        active_thread_id.clone(),
        active_workspace_path.clone(),
    );

    window.set_content(Some(&root_overlay));

    if !(settings::is_multiview_enabled(&db) && pane_layout_has_saved_thread(&db)) {
        if let Ok(Some(last_thread_id_str)) = db.get_setting("last_active_thread_id") {
            if let Ok(last_thread_id) = last_thread_id_str.parse::<i64>() {
                let mut restored = false;
                if let Ok(workspaces) = db.list_workspaces_with_threads() {
                    for workspace in workspaces {
                        if let Some(thread) =
                            workspace.threads.iter().find(|t| t.id == last_thread_id)
                        {
                            let sidebar_clone = sidebar.clone();
                            let db_for_restore = db.clone();
                            let active_workspace_path_clone = active_workspace_path.clone();
                            let active_thread_id_clone = active_thread_id.clone();
                            let thread_profile_id = thread.profile_id;
                            let thread_account_type = thread.remote_account_type_owned();
                            let thread_account_email = thread.remote_account_email_owned();
                            let workspace_path = thread
                                .worktree_path
                                .as_deref()
                                .filter(|path| thread.worktree_active && !path.trim().is_empty())
                                .map(|path| path.to_string())
                                .unwrap_or_else(|| workspace.workspace.path.clone());
                            let remote_thread_id = thread.remote_thread_id_owned();

                            gtk::glib::idle_add_local_once(move || {
                                let _ = db_for_restore.set_runtime_profile_id(thread_profile_id);
                                let _ = db_for_restore.set_active_profile_id(thread_profile_id);
                                let _ = db_for_restore.set_current_profile_account_identity(
                                    thread_account_type.as_deref(),
                                    thread_account_email.as_deref(),
                                );
                                active_workspace_path_clone.replace(Some(workspace_path));
                                active_thread_id_clone.replace(remote_thread_id);
                                if let Some(content) = sidebar_clone.content() {
                                    widget_tree::select_thread_row(&content, last_thread_id);
                                }
                            });
                            restored = true;
                            break;
                        }
                    }
                }
                if !restored {
                    let _ = db.set_setting("last_active_thread_id", "");
                    active_thread_id.replace(None);
                }
            }
        }
    }

    window.present();
    schedule_startup_memory_trim(&window);
    schedule_periodic_idle_memory_trim(&window);
}
