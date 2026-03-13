use adw::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use crate::ui::components::settings_dialog;

const PROFILE_ICON_CHOICES: [(&str, &str); 15] = [
    ("person-symbolic", "Person"),
    ("briefcase-symbolic", "Briefcase"),
    ("laptop-symbolic", "Laptop"),
    ("computer-symbolic", "Computer"),
    ("star-symbolic", "Star"),
    ("go-home-symbolic", "Home"),
    ("rocket-symbolic", "Rocket"),
    ("brain-symbolic", "Brain"),
    ("chat-bubble-symbolic", "Chat"),
    ("bookmark-symbolic", "Bookmark"),
    ("folder-symbolic", "Folder"),
    ("target-symbolic", "Target"),
    ("shield-symbolic", "Shield"),
    ("globe-symbolic", "Globe"),
    ("car-side-symbolic", "Car"),
];

fn normalize_profile_icon_name(icon_name: Option<&str>) -> String {
    let value = icon_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("person-symbolic");
    if PROFILE_ICON_CHOICES
        .iter()
        .any(|(candidate, _)| *candidate == value)
    {
        value.to_string()
    } else {
        "person-symbolic".to_string()
    }
}

fn profile_icon_label(icon_name: &str) -> &'static str {
    PROFILE_ICON_CHOICES
        .iter()
        .find(|(candidate, _)| *candidate == icon_name)
        .map(|(_, label)| *label)
        .unwrap_or("Profile Icon")
}

fn reload_profile_dropdown(
    profile_model: &gtk::StringList,
    dropdown: &gtk::DropDown,
    db: &AppDb,
    manager: &CodexProfileManager,
    preferred_profile_id: Option<i64>,
) -> Vec<i64> {
    while profile_model.n_items() > 0 {
        profile_model.remove(0);
    }
    let profiles = db.list_codex_profiles().unwrap_or_default();
    let running_ids: HashSet<i64> = manager
        .running_clients()
        .into_iter()
        .map(|(profile_id, _)| profile_id)
        .collect();
    let selected_profile_id =
        preferred_profile_id.or_else(|| db.active_profile_id().ok().flatten());
    let mut ids = Vec::new();
    let mut selected_index = 0usize;
    for (index, profile) in profiles.iter().enumerate() {
        let is_running =
            profile.status.eq_ignore_ascii_case("running") || running_ids.contains(&profile.id);
        let state = if is_running { "Running" } else { "Stopped" };
        let account = profile
            .last_email
            .clone()
            .or(profile.last_account_type.clone())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let label = if let Some(account) = account {
            format!("{} ({} | {})", profile.name, state, account)
        } else {
            format!("{} ({})", profile.name, state)
        };
        profile_model.append(&label);
        ids.push(profile.id);
        if Some(profile.id) == selected_profile_id {
            selected_index = index;
        }
    }
    if !ids.is_empty() {
        dropdown.set_selected(selected_index as u32);
    } else {
        dropdown.set_selected(gtk::INVALID_LIST_POSITION);
    }
    ids
}

fn selected_profile_id(dropdown: &gtk::DropDown, profile_ids: &[i64]) -> Option<i64> {
    let selected = dropdown.selected();
    if selected == gtk::INVALID_LIST_POSITION {
        return None;
    }
    let index = selected as usize;
    profile_ids.get(index).copied()
}

fn is_system_home_profile(db: &AppDb, profile_id: i64) -> bool {
    let system_home =
        crate::data::configured_profile_home_dir(&crate::data::default_app_data_dir());
    let system_home = system_home.to_string_lossy().to_string();
    db.get_codex_profile(profile_id)
        .ok()
        .flatten()
        .map(|profile| profile.home_dir.trim() == system_home.trim())
        .unwrap_or(false)
}

pub(crate) fn build_settings_page(
    dialog: &gtk::Window,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
) -> (gtk::Box, gtk::Button) {
    let dialog = dialog.clone();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro_label = gtk::Label::new(Some(
        "Manage isolated Codex profiles. Create additional profiles for separate accounts and runtime sessions.",
    ));
    intro_label.set_xalign(0.0);
    intro_label.set_wrap(true);
    intro_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    intro_label.add_css_class("dim-label");
    root.append(&intro_label);

    let profile_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    profile_section.add_css_class("profile-settings-section");
    let profile_section_title = gtk::Label::new(Some("Profiles"));
    profile_section_title.add_css_class("profile-section-title");
    profile_section_title.set_xalign(0.0);
    profile_section.append(&profile_section_title);
    let create_button = gtk::Button::with_label("Create New");
    create_button.add_css_class("profile-create-button");

    let profile_model = gtk::StringList::new(&[]);
    let profile_combo = gtk::DropDown::builder()
        .model(&profile_model)
        .enable_search(false)
        .build();
    let sync_profile_dropdown: Rc<dyn Fn(Option<i64>) -> Vec<i64>> = {
        let db = db.clone();
        let manager = manager.clone();
        let profile_model = profile_model.clone();
        let profile_combo = profile_combo.clone();
        Rc::new(move |preferred_profile_id: Option<i64>| {
            reload_profile_dropdown(
                &profile_model,
                &profile_combo,
                &db,
                &manager,
                preferred_profile_id,
            )
        })
    };
    let profile_ids: Rc<std::cell::RefCell<Vec<i64>>> = Rc::new(std::cell::RefCell::new(
        reload_profile_dropdown(&profile_model, &profile_combo, &db, &manager, None),
    ));
    profile_combo.set_hexpand(true);
    let profile_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    profile_row.add_css_class("profile-selector-row");

    let selected_profile_icon_name = Rc::new(RefCell::new("person-symbolic".to_string()));
    let selected_profile_icon_button = gtk::Button::new();
    selected_profile_icon_button.add_css_class("profile-icon-inline-button");
    selected_profile_icon_button.set_halign(gtk::Align::Start);
    let selected_profile_icon_image = gtk::Image::from_icon_name("person-symbolic");
    selected_profile_icon_image.set_pixel_size(15);
    selected_profile_icon_button.set_child(Some(&selected_profile_icon_image));
    profile_row.append(&selected_profile_icon_button);
    profile_row.append(&profile_combo);
    profile_section.append(&profile_row);

    let status_label = gtk::Label::new(Some("Profile status: unknown"));
    status_label.set_xalign(0.0);
    profile_section.append(&status_label);

    let account_label = gtk::Label::new(Some("Account: loading..."));
    account_label.set_xalign(0.0);
    profile_section.append(&account_label);

    let system_profile_note = gtk::Label::new(Some(
        "System profile auth controls are hidden to avoid changing your global Codex login.",
    ));
    system_profile_note.set_xalign(0.0);
    system_profile_note.set_wrap(true);
    system_profile_note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    system_profile_note.add_css_class("dim-label");
    system_profile_note.set_visible(false);
    profile_section.append(&system_profile_note);
    root.append(&profile_section);

    let runtime_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    runtime_section.add_css_class("profile-settings-section");
    let runtime_section_title = gtk::Label::new(Some("Runtime"));
    runtime_section_title.add_css_class("profile-section-title");
    runtime_section_title.set_xalign(0.0);
    runtime_section.append(&runtime_section_title);
    let process_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    process_actions.add_css_class("profile-runtime-actions");
    let start_button = gtk::Button::with_label("Start");
    start_button.add_css_class("profile-runtime-start");
    let stop_button = gtk::Button::with_label("Stop");
    stop_button.add_css_class("profile-runtime-stop");
    let restart_button = gtk::Button::with_label("Restart");
    restart_button.add_css_class("profile-runtime-restart");
    let remove_profile_button = gtk::Button::with_label("Remove Profile");
    remove_profile_button.add_css_class("destructive-action");
    process_actions.append(&start_button);
    process_actions.append(&stop_button);
    process_actions.append(&restart_button);
    process_actions.append(&remove_profile_button);
    runtime_section.append(&process_actions);
    root.append(&runtime_section);

    let auth_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    auth_section.add_css_class("profile-settings-section");
    let auth_section_title = gtk::Label::new(Some("Authentication"));
    auth_section_title.add_css_class("profile-section-title");
    auth_section_title.set_xalign(0.0);
    auth_section.append(&auth_section_title);
    let auth_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    auth_actions.add_css_class("profile-auth-actions");
    let login_button = gtk::Button::with_label("Login ChatGPT");
    let logout_button = gtk::Button::with_label("Logout");
    auth_actions.append(&login_button);
    auth_actions.append(&logout_button);
    auth_section.append(&auth_actions);
    root.append(&auth_section);

    let operation_label = gtk::Label::new(None);
    operation_label.set_xalign(0.0);
    operation_label.set_wrap(true);
    operation_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    operation_label.add_css_class("dim-label");
    operation_label.set_visible(false);
    root.append(&operation_label);

    let login_link_section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    login_link_section.add_css_class("profile-settings-section");
    login_link_section.set_visible(false);
    let login_link_title = gtk::Label::new(Some("Login Link"));
    login_link_title.add_css_class("profile-section-title");
    login_link_title.set_xalign(0.0);
    login_link_section.append(&login_link_title);
    let login_link_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let login_link_entry = gtk::Entry::new();
    login_link_entry.set_editable(false);
    login_link_entry.set_hexpand(true);
    login_link_entry.set_placeholder_text(Some("Login URL will appear here"));
    let copy_link_button = gtk::Button::with_label("Copy");
    copy_link_button.set_sensitive(false);
    login_link_row.append(&login_link_entry);
    login_link_row.append(&copy_link_button);
    login_link_section.append(&login_link_row);
    root.append(&login_link_section);

    let refresh_ui: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let status_label = status_label.clone();
        let account_label = account_label.clone();
        let login_button = login_button.clone();
        let logout_button = logout_button.clone();
        let start_button = start_button.clone();
        let stop_button = stop_button.clone();
        let restart_button = restart_button.clone();
        let remove_profile_button = remove_profile_button.clone();
        let runtime_section = runtime_section.clone();
        let auth_section = auth_section.clone();
        let process_actions = process_actions.clone();
        let auth_actions = auth_actions.clone();
        let system_profile_note = system_profile_note.clone();
        let selected_profile_icon_button = selected_profile_icon_button.clone();
        let selected_profile_icon_name = selected_profile_icon_name.clone();
        let selected_profile_icon_image = selected_profile_icon_image.clone();
        Rc::new(move || {
            let ids = profile_ids.borrow();
            let Some(profile_id) = selected_profile_id(&profile_combo, &ids) else {
                return;
            };
            if let Ok(Some(profile)) = db.get_codex_profile(profile_id) {
                let profile_icon_name = normalize_profile_icon_name(Some(&profile.icon_name));
                selected_profile_icon_name.replace(profile_icon_name.clone());
                selected_profile_icon_image.set_icon_name(Some(&profile_icon_name));
                selected_profile_icon_button
                    .set_tooltip_text(Some(profile_icon_label(&profile_icon_name)));
                let is_running = profile.status.eq_ignore_ascii_case("running")
                    || manager.running_client_for_profile(profile_id).is_some();
                let runtime_state = if is_running { "running" } else { "stopped" };
                status_label.set_text(&format!("Profile status: {runtime_state}"));
                let account = profile
                    .last_email
                    .clone()
                    .or(profile.last_account_type.clone())
                    .unwrap_or_else(|| "not logged in".to_string());
                account_label.set_text(&format!("Account: {}", account));

                let is_system = is_system_home_profile(&db, profile_id);
                let can_remove = !is_system
                    && db
                        .list_codex_profiles()
                        .map(|v| v.len() > 1)
                        .unwrap_or(false);
                let has_account = profile
                    .last_email
                    .as_deref()
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
                    || profile
                        .last_account_type
                        .as_deref()
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false);

                start_button.set_sensitive(!is_running);
                stop_button.set_sensitive(is_running);
                restart_button.set_sensitive(is_running);
                remove_profile_button.set_sensitive(can_remove);
                remove_profile_button.set_visible(!is_system);
                runtime_section.set_visible(!is_system);
                auth_section.set_visible(!is_system);
                process_actions.set_visible(!is_system);
                auth_actions.set_visible(!is_system);
                system_profile_note.set_visible(is_system);
                logout_button.set_sensitive(has_account);
                if has_account {
                    login_button.set_label("Reauthenticate");
                } else {
                    login_button.set_label("Login ChatGPT");
                }
            }
        })
    };
    refresh_ui();
    {
        let refresh_ui = refresh_ui.clone();
        profile_combo.connect_selected_notify(move |_| {
            refresh_ui();
        });
    }
    {
        let dialog = dialog.clone();
        let db = db.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let selected_profile_icon_name = selected_profile_icon_name.clone();
        let operation_label = operation_label.clone();
        let refresh_ui = refresh_ui.clone();
        selected_profile_icon_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                return;
            };
            let picker = gtk::Window::builder()
                .title("Select Profile Icon")
                .default_width(420)
                .modal(true)
                .transient_for(&dialog)
                .build();
            picker.set_resizable(false);
            let picker_root = gtk::Box::new(gtk::Orientation::Vertical, 10);
            picker_root.add_css_class("profile-icon-picker-surface");
            picker_root.set_margin_start(12);
            picker_root.set_margin_end(12);
            picker_root.set_margin_top(12);
            picker_root.set_margin_bottom(12);
            let picker_flow = gtk::FlowBox::new();
            picker_flow.add_css_class("profile-icon-picker-grid");
            picker_flow.set_selection_mode(gtk::SelectionMode::None);
            picker_flow.set_min_children_per_line(5);
            picker_flow.set_max_children_per_line(5);
            picker_flow.set_column_spacing(8);
            picker_flow.set_row_spacing(8);
            picker_root.append(&picker_flow);
            let selected_icon = selected_profile_icon_name.borrow().clone();
            for (icon_name, icon_label) in PROFILE_ICON_CHOICES {
                let button = gtk::Button::new();
                button.set_has_frame(false);
                button.add_css_class("profile-icon-picker-option");
                button.add_css_class("app-flat-button");
                button.set_tooltip_text(Some(icon_label));
                if icon_name == selected_icon {
                    button.add_css_class("is-selected");
                }
                let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
                content.set_halign(gtk::Align::Center);
                let icon = gtk::Image::from_icon_name(icon_name);
                icon.set_pixel_size(20);
                content.append(&icon);
                button.set_child(Some(&content));
                let item = gtk::FlowBoxChild::new();
                item.set_child(Some(&button));
                picker_flow.insert(&item, -1);

                let db = db.clone();
                let operation_label = operation_label.clone();
                let refresh_ui = refresh_ui.clone();
                let picker = picker.clone();
                let icon_name = icon_name.to_string();
                button.connect_clicked(move |_| {
                    match db.update_codex_profile_icon(profile_id, &icon_name) {
                        Ok(()) => {
                            operation_label.set_visible(false);
                            refresh_ui();
                            picker.close();
                        }
                        Err(err) => {
                            operation_label.set_visible(true);
                            operation_label
                                .set_text(&format!("Failed to update profile icon: {err}"));
                        }
                    }
                });
            }
            picker.set_child(Some(&picker_root));
            picker.present();
        });
    }
    {
        let refresh_ui = refresh_ui.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(300), move || {
            refresh_ui();
            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        start_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            if let Some(profile_id) = profile_id {
                let _ = manager.ensure_started(profile_id);
                let next_ids = sync_profile_dropdown(Some(profile_id));
                profile_ids.replace(next_ids);
                refresh_ui();
            }
        });
    }
    {
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        stop_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            if let Some(profile_id) = profile_id {
                manager.stop_profile(profile_id);
                let next_ids = sync_profile_dropdown(Some(profile_id));
                profile_ids.replace(next_ids);
                refresh_ui();
            }
        });
    }
    {
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        restart_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            if let Some(profile_id) = profile_id {
                let _ = manager.restart_profile(profile_id);
                let next_ids = sync_profile_dropdown(Some(profile_id));
                profile_ids.replace(next_ids);
                refresh_ui();
            }
        });
    }

    {
        let manager = manager.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let profile_ids = profile_ids.clone();
        let refresh_ui = refresh_ui.clone();
        let dialog = dialog.clone();
        create_button.connect_clicked(move |_| {
            let prompt = gtk::Window::builder()
                .title("Create Codex Profile")
                .default_width(360)
                .modal(true)
                .transient_for(&dialog)
                .build();
            let box_root = gtk::Box::new(gtk::Orientation::Vertical, 8);
            box_root.set_margin_start(12);
            box_root.set_margin_end(12);
            box_root.set_margin_top(12);
            box_root.set_margin_bottom(12);
            let entry = gtk::Entry::new();
            entry.set_placeholder_text(Some("Profile name"));
            box_root.append(&entry);
            let ok = gtk::Button::with_label("Create");
            ok.add_css_class("suggested-action");
            box_root.append(&ok);
            prompt.set_child(Some(&box_root));
            {
                let prompt = prompt.clone();
                let manager = manager.clone();
                let sync_profile_dropdown = sync_profile_dropdown.clone();
                let profile_ids = profile_ids.clone();
                let refresh_ui = refresh_ui.clone();
                ok.connect_clicked(move |_| {
                    let name = entry.text().trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    if let Ok(profile) = manager.create_profile(&name) {
                        let next_ids = sync_profile_dropdown(Some(profile.id));
                        profile_ids.replace(next_ids);
                        refresh_ui();
                    }
                    prompt.close();
                });
            }
            prompt.present();
        });
    }

    {
        let login_link_entry = login_link_entry.clone();
        copy_link_button.connect_clicked(move |_| {
            let text = login_link_entry.text();
            if text.is_empty() {
                return;
            }
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(text.as_str());
            }
        });
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let operation_label = operation_label.clone();
        let login_link_section = login_link_section.clone();
        let login_link_entry = login_link_entry.clone();
        let copy_link_button = copy_link_button.clone();
        let status_label = status_label.clone();
        let account_label = account_label.clone();
        login_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                return;
            };
            if is_system_home_profile(&db, profile_id) {
                eprintln!("refusing profile login flow on system HOME profile");
                return;
            }
            let Ok(client) = manager.ensure_started(profile_id) else {
                return;
            };
            let next_ids = sync_profile_dropdown(Some(profile_id));
            profile_ids.replace(next_ids);
            refresh_ui();
            operation_label.set_visible(true);
            operation_label.set_text("Generating ChatGPT login link...");
            login_link_section.set_visible(false);
            login_link_entry.set_text("");
            copy_link_button.set_sensitive(false);
            let (tx, rx) = mpsc::channel::<Result<(String, String), String>>();
            thread::spawn(move || {
                let _ = tx.send(client.account_login_start_chatgpt());
            });
            let db_after_start = db.clone();
            let manager_after_start = manager.clone();
            let sync_profile_dropdown_after_start = sync_profile_dropdown.clone();
            let refresh_ui_after_start = refresh_ui.clone();
            let operation_label_after_start = operation_label.clone();
            let status_label_after_start = status_label.clone();
            let account_label_after_start = account_label.clone();
            let login_link_section_after_start = login_link_section.clone();
            let login_link_entry_after_start = login_link_entry.clone();
            let copy_link_button_after_start = copy_link_button.clone();
            let profile_ids_after_start = profile_ids.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
                Ok(Ok((_login_id, url))) => {
                    login_link_section_after_start.set_visible(true);
                    login_link_entry_after_start.set_text(&url);
                    copy_link_button_after_start.set_sensitive(true);
                    operation_label_after_start.set_text(
                        "Open the link above in your browser, complete login, then return here.",
                    );
                    let Ok(client) = manager_after_start.ensure_started(profile_id) else {
                        operation_label_after_start
                            .set_text("Failed to keep profile runtime active.");
                        return gtk::glib::ControlFlow::Break;
                    };
                    let (poll_tx, poll_rx) =
                        mpsc::channel::<Result<Option<(String, Option<String>)>, String>>();
                    thread::spawn(move || {
                        for _ in 0..90 {
                            match client.account_read(true) {
                                Ok(Some(account)) => {
                                    let _ = poll_tx
                                        .send(Ok(Some((account.account_type, account.email))));
                                    return;
                                }
                                Ok(None) => {
                                    thread::sleep(Duration::from_secs(1));
                                }
                                Err(_) => {
                                    thread::sleep(Duration::from_secs(1));
                                }
                            }
                        }
                        let _ =
                            poll_tx
                                .send(Err("Timed out waiting for login completion. Try again."
                                    .to_string()));
                    });
                    let operation_label_poll = operation_label_after_start.clone();
                    let status_label_poll = status_label_after_start.clone();
                    let account_label_poll = account_label_after_start.clone();
                    let db_poll = db_after_start.clone();
                    let sync_profile_dropdown_poll = sync_profile_dropdown_after_start.clone();
                    let refresh_ui_poll = refresh_ui_after_start.clone();
                    let profile_ids_poll = profile_ids_after_start.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(120), move || match poll_rx
                        .try_recv()
                    {
                        Ok(Ok(Some((account_type, email)))) => {
                            let account_text =
                                email.clone().unwrap_or_else(|| account_type.clone());
                            status_label_poll.set_text("Profile status: running");
                            account_label_poll.set_text(&format!("Account: {}", account_text));
                            operation_label_poll.set_text("Login completed.");
                            let _ = db_poll.update_codex_profile_status(profile_id, "running");
                            let _ = db_poll.update_codex_profile_account(
                                profile_id,
                                Some(account_type.as_str()),
                                email.as_deref(),
                            );
                            if db_poll.active_profile_id().ok().flatten() == Some(profile_id) {
                                let _ = db_poll.set_current_codex_account(
                                    Some(account_type.as_str()),
                                    email.as_deref(),
                                );
                            }
                            let next_ids = sync_profile_dropdown_poll(Some(profile_id));
                            profile_ids_poll.replace(next_ids);
                            refresh_ui_poll();
                            gtk::glib::ControlFlow::Break
                        }
                        Ok(Ok(None)) => {
                            operation_label_poll
                                .set_text("Login completed but no account details returned.");
                            gtk::glib::ControlFlow::Break
                        }
                        Ok(Err(err)) => {
                            operation_label_poll.set_text(&err);
                            gtk::glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
                    });
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    operation_label_after_start.set_visible(true);
                    operation_label_after_start.set_text(&format!("Login failed: {err}"));
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
            });
        });
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let operation_label = operation_label.clone();
        logout_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                return;
            };
            if is_system_home_profile(&db, profile_id) {
                eprintln!("refusing logout on system HOME profile");
                return;
            }
            let Ok(client) = manager.ensure_started(profile_id) else {
                return;
            };
            let _ = client.account_logout();
            let _ = db.update_codex_profile_account(profile_id, None, None);
            if db.active_profile_id().ok().flatten() == Some(profile_id) {
                let _ = db.set_current_codex_account(None, None);
            }
            operation_label.set_visible(true);
            operation_label.set_text("Logged out from this profile.");
            let next_ids = sync_profile_dropdown(Some(profile_id));
            profile_ids.replace(next_ids);
            refresh_ui();
        });
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let dialog = dialog.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let operation_label = operation_label.clone();
        let refresh_ui = refresh_ui.clone();
        remove_profile_button.connect_clicked(move |_| {
            let ids = profile_ids.borrow();
            let Some(profile_id) = selected_profile_id(&profile_combo, &ids) else {
                return;
            };
            if is_system_home_profile(&db, profile_id) {
                return;
            }
            let profile_name = db
                .get_codex_profile(profile_id)
                .ok()
                .flatten()
                .map(|p| p.name)
                .unwrap_or_else(|| "this profile".to_string());
            let confirm = gtk::Window::builder()
                .title("Remove Codex Profile")
                .default_width(420)
                .modal(true)
                .transient_for(&dialog)
                .build();
            let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
            root.set_margin_start(12);
            root.set_margin_end(12);
            root.set_margin_top(12);
            root.set_margin_bottom(12);
            let msg = gtk::Label::new(Some(&format!(
                "Remove profile \"{profile_name}\"?\nIts runtime folder will be deleted, and its threads will be reassigned to another profile."
            )));
            msg.set_xalign(0.0);
            msg.set_wrap(true);
            msg.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            root.append(&msg);
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            actions.set_halign(gtk::Align::End);
            let cancel = gtk::Button::with_label("Cancel");
            let remove = gtk::Button::with_label("Remove");
            remove.add_css_class("destructive-action");
            actions.append(&cancel);
            actions.append(&remove);
            root.append(&actions);
            confirm.set_child(Some(&root));

            {
                let confirm = confirm.clone();
                cancel.connect_clicked(move |_| confirm.close());
            }
            {
                let confirm = confirm.clone();
                let db = db.clone();
                let manager = manager.clone();
                let sync_profile_dropdown = sync_profile_dropdown.clone();
                let profile_ids = profile_ids.clone();
                let operation_label = operation_label.clone();
                let refresh_ui = refresh_ui.clone();
                remove.connect_clicked(move |_| {
                    match manager.remove_profile(profile_id) {
                        Ok(()) => {
                            let preferred_id = db.active_profile_id().ok().flatten();
                            let next_ids = sync_profile_dropdown(preferred_id);
                            profile_ids.replace(next_ids);
                            refresh_ui();
                            operation_label.set_visible(true);
                            operation_label.set_text("Profile removed.");
                        }
                        Err(err) => {
                            operation_label.set_visible(true);
                            operation_label.set_text(&format!("Failed to remove profile: {err}"));
                        }
                    }
                    confirm.close();
                });
            }
            confirm.present();
        });
    }
    (root, create_button)
}

pub fn show(parent: Option<&gtk::Window>, db: Rc<AppDb>, manager: Rc<CodexProfileManager>) {
    settings_dialog::show(parent, db, manager, settings_dialog::SettingsPage::Profiles);
}
