use super::{composer, runtime_controls};
use crate::services::app::CodexProfileManager;
use crate::services::app::chat::{AppDb, CodexProfileRecord};
use crate::services::app::runtime::RuntimeClient;
use crate::ui::widget_tree;
use adw::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub(super) struct AttachArgs {
    pub db: Rc<AppDb>,
    pub manager: Rc<CodexProfileManager>,
    pub active_thread_id: Rc<RefCell<Option<String>>>,
    pub selected_thread_id: Rc<RefCell<Option<String>>>,
    pub active_workspace_path: Rc<RefCell<Option<String>>>,
    pub composer_revealer: gtk::Revealer,
    pub live_turn_status_revealer: gtk::Revealer,
    pub heading: gtk::Label,
    pub install_box: gtk::Box,
    pub empty_state: gtk::Box,
    pub messages_box: gtk::Box,
    pub conversation_stack: gtk::Stack,
}

fn selector_backend_icon_name(backend_kind: &str) -> &'static str {
    if backend_kind.eq_ignore_ascii_case("opencode") {
        "provider-opencode"
    } else {
        "provider-codex"
    }
}

fn selector_profile_identity(profile: &CodexProfileRecord) -> Option<String> {
    profile
        .last_email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            profile
                .last_account_type
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn selector_is_system_profile(profile: &CodexProfileRecord) -> bool {
    let system_home = crate::services::app::chat::configured_profile_home_dir(
        &crate::services::app::chat::default_app_data_dir(),
    );
    profile.home_dir.trim() == system_home.to_string_lossy().trim()
}

fn selector_default_model_id(client: &RuntimeClient, backend_kind: &str) -> Option<String> {
    let client = Arc::new(client.clone());
    let models = runtime_controls::model_options(Some(&client));
    if let Some(model) = models.iter().find(|model| model.is_default) {
        return Some(model.id.clone());
    }
    if let Some(model) = models.first() {
        if !model.id.trim().is_empty() {
            return Some(model.id.clone());
        }
    }
    if backend_kind.eq_ignore_ascii_case("codex") {
        Some("gpt-5.3-codex".to_string())
    } else {
        None
    }
}

pub(super) fn attach(args: AttachArgs) {
    let profile_selector = gtk::Box::new(gtk::Orientation::Vertical, 12);
    profile_selector.add_css_class("chat-profile-selector");
    profile_selector.set_halign(gtk::Align::Center);
    profile_selector.set_valign(gtk::Align::Center);
    profile_selector.set_visible(false);

    let selector_header = gtk::Box::new(gtk::Orientation::Vertical, 3);
    selector_header.add_css_class("chat-profile-selector-header");

    let selector_title = gtk::Label::new(Some("Choose Runtime"));
    selector_title.add_css_class("chat-profile-selector-title");
    selector_title.set_xalign(0.0);
    selector_header.append(&selector_title);

    let selector_subtitle = gtk::Label::new(Some("Pick the runtime for this thread."));
    selector_subtitle.add_css_class("chat-profile-selector-subtitle");
    selector_subtitle.set_wrap(true);
    selector_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    selector_subtitle.set_xalign(0.0);
    selector_header.append(&selector_subtitle);
    profile_selector.append(&selector_header);

    let profile_cards = gtk::Box::new(gtk::Orientation::Vertical, 8);
    profile_cards.add_css_class("chat-profile-selector-grid");
    profile_selector.append(&profile_cards);

    let selector_status = gtk::Label::new(None);
    selector_status.add_css_class("chat-profile-selector-status");
    selector_status.set_xalign(0.0);
    selector_status.set_wrap(true);
    selector_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    selector_status.set_visible(false);
    profile_selector.append(&selector_status);
    args.empty_state.append(&profile_selector);

    let db = args.db;
    let manager = args.manager;
    let active_thread_id = args.active_thread_id;
    let selected_thread_id = args.selected_thread_id;
    let active_workspace_path = args.active_workspace_path;
    let composer_revealer = args.composer_revealer;
    let live_turn_status_revealer = args.live_turn_status_revealer;
    let heading = args.heading;
    let install_box = args.install_box;
    let messages_box_for_selector = args.messages_box;
    let conversation_stack_for_selector = args.conversation_stack;

    let selector_mode_active = Rc::new(RefCell::new(false));
    let selector_mode_active_flag = selector_mode_active.clone();
    let selector_render_key: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let selector_render_key_flag = selector_render_key.clone();
    let codex_install_state: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    let codex_install_check_in_flight = Rc::new(RefCell::new(false));
    let codex_install_last_check_micros = Rc::new(RefCell::new(0i64));
    let (codex_install_tx, codex_install_rx) = mpsc::channel::<bool>();

    gtk::glib::timeout_add_local(Duration::from_millis(250), move || {
        if profile_selector.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        while let Ok(installed) = codex_install_rx.try_recv() {
            codex_install_check_in_flight.replace(false);
            codex_install_state.replace(Some(installed));
        }
        let pending_thread_id = db
            .get_setting("pending_profile_thread_id")
            .ok()
            .flatten()
            .and_then(|value| value.parse::<i64>().ok());
        let pending_thread_record =
            pending_thread_id.and_then(|thread_id| db.get_thread_record(thread_id).ok().flatten());
        let current_local_thread_id_raw = db
            .get_setting("last_active_thread_id")
            .ok()
            .flatten()
            .and_then(|value| value.parse::<i64>().ok());
        let current_local_thread_id = current_local_thread_id_raw.and_then(|thread_id| {
            db.get_thread_record(thread_id)
                .ok()
                .flatten()
                .map(|_| thread_id)
        });
        if current_local_thread_id_raw.is_some() && current_local_thread_id.is_none() {
            let _ = db.set_setting("last_active_thread_id", "");
        }
        let has_workspaces = db
            .list_workspaces_with_threads()
            .map(|items| !items.is_empty())
            .unwrap_or(false);
        if current_local_thread_id.is_some() {
            heading.set_text("Start Coding");
        } else if has_workspaces {
            heading.set_text("Select a Thread");
        } else {
            heading.set_text("Add a Workspace");
        }
        let pending_unresolved = pending_thread_record
            .as_ref()
            .map(|thread| {
                thread
                    .remote_thread_id()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
            })
            .unwrap_or(false);
        let should_probe_codex = active_thread_id.borrow().is_none() && has_workspaces;
        if should_probe_codex && *codex_install_state.borrow() != Some(true) {
            let now = gtk::glib::monotonic_time();
            let last_check = *codex_install_last_check_micros.borrow();
            let retry_interval = if codex_install_state.borrow().is_some() {
                3_000_000
            } else {
                0
            };
            if !*codex_install_check_in_flight.borrow() && now - last_check >= retry_interval {
                codex_install_check_in_flight.replace(true);
                codex_install_last_check_micros.replace(now);
                let tx = codex_install_tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(crate::services::app::runtime::any_runtime_cli_available());
                });
            }
        }
        let codex_missing =
            should_probe_codex && matches!(*codex_install_state.borrow(), Some(false));
        let show_selector =
            active_thread_id.borrow().is_none() && pending_unresolved && !codex_missing;
        let has_active_thread = active_thread_id.borrow().is_some();
        let show_composer = !show_selector
            && !codex_missing
            && current_local_thread_id.is_some()
            && has_active_thread;
        profile_selector.set_visible(show_selector);
        install_box.set_visible(codex_missing);
        heading.set_visible(!show_selector);
        if codex_missing {
            heading.set_text("Install Runtime CLI");
        }
        composer_revealer.set_visible(show_composer);
        composer_revealer.set_reveal_child(show_composer);
        if !show_composer {
            live_turn_status_revealer.set_reveal_child(false);
            live_turn_status_revealer.set_visible(false);
        }
        if (show_selector || codex_missing) && !*selector_mode_active_flag.borrow() {
            widget_tree::clear_box_children(&messages_box_for_selector);
            conversation_stack_for_selector.set_visible_child_name("empty");
            selector_mode_active_flag.replace(true);
        } else if !show_selector && !codex_missing && *selector_mode_active_flag.borrow() {
            selector_mode_active_flag.replace(false);
        }
        if !show_selector {
            selector_status.set_visible(false);
            selector_render_key_flag.borrow_mut().clear();
            return gtk::glib::ControlFlow::Continue;
        }

        let Some(pending_thread_id) = pending_thread_record.as_ref().map(|thread| thread.id) else {
            let _ = db.set_setting("pending_profile_thread_id", "");
            return gtk::glib::ControlFlow::Continue;
        };

        let running_ids: HashSet<i64> = manager
            .running_clients()
            .into_iter()
            .map(|(profile_id, _)| profile_id)
            .collect();
        let profiles = db.list_codex_profiles().unwrap_or_default();
        let selected_opencode_access_mode =
            composer::default_composer_setting_value(db.as_ref(), "opencode_access_mode")
                .unwrap_or_else(|| "workspaceWrite".to_string());
        let selected_opencode_command_mode =
            composer::default_composer_setting_value(db.as_ref(), "opencode_command_mode")
                .unwrap_or_else(|| "allowAll".to_string());
        let mut key = format!("pending:{pending_thread_id};");
        key.push_str(&format!(
            "opencode_access:{};opencode_command:{};",
            selected_opencode_access_mode, selected_opencode_command_mode
        ));
        for profile in &profiles {
            let running_flag = running_ids.contains(&profile.id);
            let email = profile.last_email.as_deref().unwrap_or("");
            let account_type = profile.last_account_type.as_deref().unwrap_or("");
            let icon_name = profile.icon_name.as_str();
            key.push_str(&format!(
                "{}:{}:{}:{}:{}:{}:{}|",
                profile.id,
                profile.name,
                profile.status,
                email,
                account_type,
                icon_name,
                running_flag
            ));
        }
        let previous_key = selector_render_key_flag.borrow().clone();
        if previous_key == key {
            return gtk::glib::ControlFlow::Continue;
        }
        selector_render_key_flag.replace(key);

        widget_tree::clear_box_children(&profile_cards);

        let mut has_selectable = false;
        let mut has_startable = false;
        let mut render_backend_section =
            |backend_kind: &str, backend_profiles: Vec<CodexProfileRecord>| {
                let profile_count = backend_profiles.len();
                let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
                section.add_css_class("chat-profile-section");
                section.set_hexpand(true);

                let section_header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                section_header.add_css_class("chat-profile-section-header");
                section_header.set_hexpand(true);

                let section_icon_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                section_icon_wrap.add_css_class("chat-profile-section-icon-wrap");
                let section_icon =
                    gtk::Image::from_icon_name(selector_backend_icon_name(backend_kind));
                section_icon.set_pixel_size(16);
                section_icon_wrap.append(&section_icon);
                section_header.append(&section_icon_wrap);

                let section_heading = gtk::Box::new(gtk::Orientation::Vertical, 1);
                section_heading.set_hexpand(true);
                let section_title = gtk::Label::new(Some(
                    crate::services::app::runtime::backend_display_name(backend_kind),
                ));
                section_title.add_css_class("chat-profile-section-title");
                section_title.set_xalign(0.0);
                section_heading.append(&section_title);
                section_header.append(&section_heading);

                let section_badge = gtk::Label::new(Some(&match profile_count {
                    0 => "Setup".to_string(),
                    1 => "1 profile".to_string(),
                    count => format!("{count} profiles"),
                }));
                section_badge.add_css_class("chat-profile-section-badge");
                section_badge.set_halign(gtk::Align::End);
                section_header.append(&section_badge);
                section.append(&section_header);

                let render_profiles = backend_profiles.len() > 1;
                let cards_to_render: Vec<Option<CodexProfileRecord>> = if render_profiles {
                    backend_profiles.into_iter().map(Some).collect()
                } else {
                    vec![backend_profiles.into_iter().next()]
                };
                if backend_kind.eq_ignore_ascii_case("opencode") {
                    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    toolbar.add_css_class("chat-profile-section-toolbar");
                    toolbar.set_hexpand(true);

                    let toolbar_intro = gtk::Label::new(Some("Defaults"));
                    toolbar_intro.add_css_class("chat-profile-section-toolbar-intro");
                    toolbar_intro.set_xalign(0.0);
                    toolbar_intro.set_hexpand(true);
                    toolbar.append(&toolbar_intro);

                    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    controls.add_css_class("chat-profile-section-toolbar-controls");
                    controls.set_halign(gtk::Align::End);

                    let access_group = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                    access_group.add_css_class("chat-profile-section-control-group");
                    let access_label = gtk::Label::new(Some("Access"));
                    access_label.add_css_class("chat-profile-card-access-label");
                    access_label.set_xalign(0.0);
                    access_group.append(&access_label);
                    let access_setting_changed: Rc<dyn Fn(String)> = {
                        let db = db.clone();
                        let selector_render_key = selector_render_key_flag.clone();
                        Rc::new(move |value: String| {
                            composer::save_default_composer_setting_value(
                                &db,
                                "opencode_access_mode",
                                &value,
                            );
                            selector_render_key.borrow_mut().clear();
                        })
                    };
                    let (access_selector, _selected_access_mode, _set_access_mode) =
                        runtime_controls::build_access_selector(
                            Some(selected_opencode_access_mode.clone()),
                            Some(access_setting_changed),
                        );
                    access_selector.add_css_class("chat-profile-card-access-selector");
                    access_selector.add_css_class("chat-profile-toolbar-selector");
                    access_group.append(&access_selector);
                    controls.append(&access_group);

                    let command_group = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                    command_group.add_css_class("chat-profile-section-control-group");
                    let command_label = gtk::Label::new(Some("Commands"));
                    command_label.add_css_class("chat-profile-card-access-label");
                    command_label.set_xalign(0.0);
                    command_group.append(&command_label);
                    let command_setting_changed: Rc<dyn Fn(String)> = {
                        let db = db.clone();
                        let selector_render_key = selector_render_key_flag.clone();
                        Rc::new(move |value: String| {
                            composer::save_default_composer_setting_value(
                                &db,
                                "opencode_command_mode",
                                &value,
                            );
                            selector_render_key.borrow_mut().clear();
                        })
                    };
                    let (command_selector, _selected_command_mode, _set_command_mode) =
                        runtime_controls::build_opencode_command_selector(
                            Some(selected_opencode_command_mode.clone()),
                            Some(command_setting_changed),
                        );
                    command_selector.add_css_class("chat-profile-card-access-selector");
                    command_selector.add_css_class("chat-profile-toolbar-selector");
                    command_group.append(&command_selector);
                    controls.append(&command_group);
                    toolbar.append(&controls);
                    section.append(&toolbar);
                }

                let section_cards = gtk::Box::new(gtk::Orientation::Vertical, 4);
                section_cards.add_css_class("chat-profile-section-cards");

                for profile in cards_to_render {
                    let profile_id = profile.as_ref().map(|profile| profile.id);
                    let backend_display_name =
                        crate::services::app::runtime::backend_display_name(backend_kind)
                            .to_string();
                    let is_system_profile = profile
                        .as_ref()
                        .map(selector_is_system_profile)
                        .unwrap_or(false);
                    let is_running = profile
                        .as_ref()
                        .map(|profile| {
                            profile.status.eq_ignore_ascii_case("running")
                                || running_ids.contains(&profile.id)
                        })
                        .unwrap_or(false);
                    let account_label_text = profile
                        .as_ref()
                        .and_then(selector_profile_identity)
                        .unwrap_or_else(|| "Authentication required".to_string());
                    let has_identity = profile
                        .as_ref()
                        .and_then(selector_profile_identity)
                        .is_some();
                    let card_title_text = match profile.as_ref() {
                        Some(profile) if render_profiles || !is_system_profile => {
                            profile.name.clone()
                        }
                        Some(_) => "Default profile".to_string(),
                        None => format!("Set up {backend_display_name}"),
                    };
                    let card_subtitle_text = if has_identity {
                        account_label_text.clone()
                    } else if profile.is_none() {
                        "No profile yet. Create one on first use.".to_string()
                    } else if is_running {
                        "Authentication required before this runtime can be used.".to_string()
                    } else if is_system_profile {
                        format!("{backend_display_name} runtime")
                    } else {
                        format!("{backend_display_name} profile")
                    };

                    if is_running && has_identity && profile_id.is_some() {
                        has_selectable = true;
                    }
                    if !is_running || profile_id.is_none() {
                        has_startable = true;
                    }
                    let is_selectable_card = is_running && has_identity && profile_id.is_some();

                    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
                    card.add_css_class("chat-profile-card");
                    card.set_halign(gtk::Align::Fill);
                    card.set_hexpand(true);
                    if is_selectable_card {
                        card.add_css_class("chat-profile-card-selectable");
                    }

                    let top_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    top_row.add_css_class("chat-profile-card-top");
                    top_row.set_halign(gtk::Align::Fill);
                    top_row.set_hexpand(true);

                    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    header.add_css_class("chat-profile-card-main");
                    header.set_halign(gtk::Align::Fill);
                    header.set_hexpand(true);

                    let avatar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    avatar.add_css_class("chat-profile-card-avatar");
                    let avatar_icon_name = profile
                        .as_ref()
                        .map(|profile| profile.icon_name.trim().to_string())
                        .filter(|icon_name| !icon_name.is_empty())
                        .unwrap_or_else(|| selector_backend_icon_name(backend_kind).to_string());
                    let icon = gtk::Image::from_icon_name(&avatar_icon_name);
                    icon.set_pixel_size(14);
                    icon.add_css_class("chat-profile-card-avatar-image");
                    avatar.append(&icon);
                    header.append(&avatar);

                    let meta = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    meta.set_halign(gtk::Align::Fill);
                    meta.set_hexpand(true);

                    let title = gtk::Label::new(Some(&card_title_text));
                    title.add_css_class("chat-profile-card-title");
                    title.set_xalign(0.0);
                    meta.append(&title);

                    let subtitle = gtk::Label::new(Some(&card_subtitle_text));
                    subtitle.add_css_class("chat-profile-card-email");
                    subtitle.add_css_class("chat-profile-card-subtitle");
                    subtitle.set_xalign(0.0);
                    subtitle.set_wrap(true);
                    subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    meta.append(&subtitle);
                    header.append(&meta);
                    top_row.append(&header);

                    let runtime_state = if is_running { "Running" } else { "Stopped" };
                    let state = gtk::Label::new(Some(runtime_state));
                    state.add_css_class("chat-profile-card-state-pill");
                    if is_running {
                        state.add_css_class("is-running");
                    } else {
                        state.add_css_class("is-stopped");
                    }
                    state.set_halign(gtk::Align::End);
                    top_row.append(&state);
                    card.append(&top_row);

                    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    actions.add_css_class("chat-profile-card-actions");
                    actions.set_halign(gtk::Align::Fill);
                    actions.set_hexpand(true);
                    let mut has_action_content = false;

                    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    button_row.add_css_class("chat-profile-card-action-buttons");
                    button_row.set_halign(gtk::Align::End);

                    if !is_running || profile_id.is_none() {
                        let start_label = if profile_id.is_none() {
                            "Create & start"
                        } else {
                            "Start"
                        };
                        let start_button = gtk::Button::with_label(start_label);
                        start_button.add_css_class("chat-profile-card-start");
                        let db_start = db.clone();
                        let manager_start = manager.clone();
                        let selector_status_start = selector_status.clone();
                        let selector_render_key_start = selector_render_key_flag.clone();
                        let card_title_start = card_title_text.clone();
                        let backend_kind_start = backend_kind.to_string();
                        start_button.connect_clicked(move |_| {
                            selector_status_start.set_visible(true);
                            selector_status_start
                                .set_text(&format!("Starting \"{}\"...", card_title_start));
                            let profile = if let Some(profile_id) = profile_id {
                                db_start
                                    .get_codex_profile(profile_id)
                                    .map_err(|err| err.to_string())
                                    .and_then(|profile| {
                                        profile.ok_or_else(|| {
                                            format!("profile {} not found", profile_id)
                                        })
                                    })
                            } else {
                                manager_start.ensure_profile_for_backend(&backend_kind_start)
                            };
                            let profile = match profile {
                                Ok(profile) => profile,
                                Err(err) => {
                                    selector_status_start
                                        .set_text(&format!("Failed to prepare runtime: {err}"));
                                    return;
                                }
                            };
                            match manager_start.ensure_started(profile.id) {
                                Ok(_) => {
                                    selector_status_start.set_text(
                                        "Runtime started. Select it when authentication is ready.",
                                    );
                                    selector_render_key_start.borrow_mut().clear();
                                }
                                Err(err) => {
                                    selector_status_start
                                        .set_text(&format!("Failed to start runtime: {err}"));
                                }
                            }
                        });
                        button_row.append(&start_button);
                        has_action_content = true;
                    }

                    if is_selectable_card {
                        if let Some(profile_id) = profile_id {
                            let manager_click = manager.clone();
                            let db_click = db.clone();
                            let selected_thread_id_click = selected_thread_id.clone();
                            let active_workspace_path_click = active_workspace_path.clone();
                            let selector_status_click = selector_status.clone();
                            let composer_revealer_click = composer_revealer.clone();
                            let profile_selector_click = profile_selector.clone();
                            let card_title_click = card_title_text.clone();
                            let backend_kind_click = backend_kind.to_string();
                            let selected_opencode_access_mode_for_click =
                                selected_opencode_access_mode.clone();
                            let selected_opencode_command_mode_for_click =
                                selected_opencode_command_mode.clone();
                            let account_type = profile
                                .as_ref()
                                .and_then(|profile| profile.last_account_type.clone());
                            let account_email = profile
                                .as_ref()
                                .and_then(|profile| profile.last_email.clone());
                            let click = gtk::GestureClick::new();
                            click.set_button(0);
                            click.connect_released(move |_, _, _, _| {
                                let workspace = active_workspace_path_click
                                    .borrow()
                                    .clone()
                                    .unwrap_or_else(|| ".".to_string());
                                selector_status_click.set_visible(true);
                                selector_status_click.set_text(&format!(
                                    "Starting thread with \"{}\"...",
                                    card_title_click
                                ));
                                let Some(client) = manager_click.client_for_profile(profile_id) else {
                                    selector_status_click
                                        .set_text("Runtime is not available for this selection.");
                                    return;
                                };
                                let default_model =
                                    selector_default_model_id(client.as_ref(), &backend_kind_click);
                                let sandbox_policy = if backend_kind_click
                                    .eq_ignore_ascii_case("opencode")
                                {
                                    Some(runtime_controls::opencode_session_policy_for(
                                        &selected_opencode_access_mode_for_click,
                                        &selected_opencode_command_mode_for_click,
                                    ))
                                } else {
                                    None
                                };
                                let (tx, rx) = mpsc::channel::<Result<String, String>>();
                                thread::spawn(move || {
                                    let result = client.thread_start(
                                        Some(&workspace),
                                        default_model.as_deref(),
                                        sandbox_policy,
                                    );
                                    let _ = tx.send(result);
                                });

                                let db_result = db_click.clone();
                                let selected_thread_id_result = selected_thread_id_click.clone();
                                let selector_status_result = selector_status_click.clone();
                                let composer_revealer_result = composer_revealer_click.clone();
                                let profile_selector_result = profile_selector_click.clone();
                                let account_type_result = account_type.clone();
                                let account_email_result = account_email.clone();
                                gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                                    if profile_selector_result.root().is_none() {
                                        return gtk::glib::ControlFlow::Break;
                                    }
                                    match rx.try_recv() {
                                        Ok(Ok(remote_thread_id)) => {
                                            let _ = db_result.assign_thread_profile_and_remote(
                                                pending_thread_id,
                                                profile_id,
                                                &remote_thread_id,
                                                account_type_result.as_deref(),
                                                account_email_result.as_deref(),
                                            );
                                            let _ = db_result.set_runtime_profile_id(profile_id);
                                            let _ = db_result.set_setting("pending_profile_thread_id", "");
                                            crate::ui::components::thread_list::refresh_all_profile_icon_visibility();
                                            selected_thread_id_result.replace(Some(remote_thread_id));
                                            profile_selector_result.set_visible(false);
                                            selector_status_result.set_visible(false);
                                            composer_revealer_result.set_reveal_child(true);
                                            gtk::glib::ControlFlow::Break
                                        }
                                        Ok(Err(err)) => {
                                            selector_status_result
                                                .set_text(&format!("Failed to start thread: {err}"));
                                            gtk::glib::ControlFlow::Break
                                        }
                                        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                                        Err(mpsc::TryRecvError::Disconnected) => {
                                            gtk::glib::ControlFlow::Break
                                        }
                                    }
                                });
                            });
                            card.add_controller(click);
                        }
                    } else if is_running {
                        let hint = gtk::Label::new(Some(
                            "Authenticate this runtime to make it selectable.",
                        ));
                        hint.add_css_class("chat-profile-card-hint");
                        hint.set_xalign(0.0);
                        hint.set_hexpand(true);
                        actions.append(&hint);
                        has_action_content = true;
                    }

                    if button_row.first_child().is_some() {
                        actions.append(&button_row);
                    }
                    if has_action_content || button_row.first_child().is_some() {
                        card.append(&actions);
                    }
                    section_cards.append(&card);
                }

                section.append(&section_cards);
                profile_cards.append(&section);
            };

        let codex_profiles: Vec<CodexProfileRecord> = profiles
            .iter()
            .filter(|profile| profile.backend_kind.eq_ignore_ascii_case("codex"))
            .cloned()
            .collect();
        let opencode_profiles: Vec<CodexProfileRecord> = profiles
            .iter()
            .filter(|profile| profile.backend_kind.eq_ignore_ascii_case("opencode"))
            .cloned()
            .collect();
        render_backend_section("codex", codex_profiles);
        render_backend_section("opencode", opencode_profiles);

        if !has_selectable {
            selector_status.set_visible(true);
            if has_startable {
                selector_status.set_text(
                    "Start a runtime first, then select it once authentication is available.",
                );
            } else {
                selector_status.set_text(
                    "No authenticated runtime is available yet. Open profile settings if you need to sign in.",
                );
            }
        } else if !selector_status.is_visible() {
            selector_status.set_visible(false);
        }

        gtk::glib::ControlFlow::Continue
    });
}
