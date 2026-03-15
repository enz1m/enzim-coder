use adw::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
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

fn is_system_profile_record(profile: &crate::data::CodexProfileRecord) -> bool {
    let system_home =
        crate::data::configured_profile_home_dir(&crate::data::default_app_data_dir());
    profile.home_dir.trim() == system_home.to_string_lossy().trim()
}

fn profile_icon_label(icon_name: &str) -> &'static str {
    PROFILE_ICON_CHOICES
        .iter()
        .find(|(candidate, _)| *candidate == icon_name)
        .map(|(_, label)| *label)
        .unwrap_or("Profile Icon")
}

fn opencode_provider_dropdown_label(provider: &crate::backend::AccountProviderInfo) -> String {
    if provider.connected || provider.has_saved_auth {
        format!("{} (Added)", provider.provider_name)
    } else {
        provider.provider_name.clone()
    }
}

fn open_uri_in_browser(uri: &str) {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return;
    }
    let _ = gtk::gio::AppInfo::launch_default_for_uri(trimmed, None::<&gtk::gio::AppLaunchContext>);
}

fn reload_profile_dropdown(
    profile_model: &gtk::StringList,
    dropdown: &gtk::DropDown,
    db: &AppDb,
    manager: &CodexProfileManager,
    preferred_profile_id: Option<i64>,
    backend_filter: Option<&str>,
    system_only: bool,
) -> Vec<i64> {
    while profile_model.n_items() > 0 {
        profile_model.remove(0);
    }
    let profiles = db
        .list_codex_profiles()
        .unwrap_or_default()
        .into_iter()
        .filter(|profile| {
            backend_filter
                .map(|backend| profile.backend_kind.eq_ignore_ascii_case(backend))
                .unwrap_or(true)
        })
        .filter(|profile| !system_only || is_system_profile_record(profile))
        .collect::<Vec<_>>();
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

pub(super) fn build_profile_settings_page(
    dialog: &gtk::Window,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    backend_filter: Option<&'static str>,
    page_title: &'static str,
    intro_text: &'static str,
    allow_create: bool,
    system_only: bool,
    runtime_only: bool,
) -> (gtk::Box, gtk::Button) {
    let dialog = dialog.clone();
    if runtime_only {
        if let Some(backend_kind) = backend_filter {
            let _ = manager.ensure_profile_for_backend(backend_kind);
        }
    }
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro_label = gtk::Label::new(Some(intro_text));
    intro_label.set_xalign(0.0);
    intro_label.set_wrap(true);
    intro_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    intro_label.add_css_class("dim-label");
    intro_label.set_hexpand(true);
    intro_label.set_halign(gtk::Align::Start);

    let models_button = gtk::Button::with_label("Models");
    models_button.add_css_class("profile-create-button");
    models_button.set_halign(gtk::Align::End);
    models_button.set_visible(runtime_only);
    let intro_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    intro_row.set_hexpand(true);
    intro_row.set_valign(gtk::Align::Start);
    intro_row.append(&intro_label);
    intro_row.append(&models_button);
    root.append(&intro_row);

    let profile_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    profile_section.add_css_class("profile-settings-section");
    let profile_section_title = gtk::Label::new(Some(page_title));
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
                backend_filter,
                system_only,
            )
        })
    };
    let profile_ids: Rc<std::cell::RefCell<Vec<i64>>> =
        Rc::new(std::cell::RefCell::new(reload_profile_dropdown(
            &profile_model,
            &profile_combo,
            &db,
            &manager,
            None,
            backend_filter,
            system_only,
        )));
    profile_combo.set_hexpand(true);
    let profile_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    profile_row.add_css_class("profile-selector-row");
    profile_row.set_visible(!runtime_only);

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

    let status_label = gtk::Label::new(Some(if runtime_only {
        "Runtime status: unknown"
    } else {
        "Profile status: unknown"
    }));
    status_label.set_xalign(0.0);
    profile_section.append(&status_label);

    let account_label = gtk::Label::new(Some(if runtime_only {
        "Providers with auth: loading..."
    } else {
        "Account: loading..."
    }));
    account_label.set_xalign(0.0);
    profile_section.append(&account_label);

    let system_profile_note = gtk::Label::new(Some(
        "System profile auth controls are hidden to avoid changing your global runtime login.",
    ));
    system_profile_note.set_xalign(0.0);
    system_profile_note.set_wrap(true);
    system_profile_note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    system_profile_note.add_css_class("dim-label");
    system_profile_note.set_visible(false);
    profile_section.append(&system_profile_note);
    create_button.set_visible(allow_create && !runtime_only);
    root.append(&profile_section);

    let runtime_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    runtime_section.add_css_class("profile-settings-section");
    let runtime_section_title = gtk::Label::new(Some(if runtime_only {
        "OpenCode Runtime"
    } else {
        "Runtime"
    }));
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
    remove_profile_button.set_visible(!runtime_only);
    process_actions.append(&start_button);
    process_actions.append(&stop_button);
    process_actions.append(&restart_button);
    process_actions.append(&remove_profile_button);
    runtime_section.append(&process_actions);
    root.append(&runtime_section);

    let auth_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    auth_section.add_css_class("profile-settings-section");
    let auth_section_title = gtk::Label::new(Some(if runtime_only {
        "Providers"
    } else {
        "Authentication"
    }));
    auth_section_title.add_css_class("profile-section-title");
    auth_section_title.set_xalign(0.0);
    auth_section.append(&auth_section_title);
    let auth_section_intro = gtk::Label::new(Some(if runtime_only {
        "Manage OpenCode provider credentials here. Providers and auth methods come from the OpenCode server."
    } else {
        "Manage login state for this runtime profile."
    }));
    auth_section_intro.set_xalign(0.0);
    auth_section_intro.set_wrap(true);
    auth_section_intro.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    auth_section_intro.add_css_class("dim-label");
    auth_section.append(&auth_section_intro);
    let provider_selector_button = gtk::Button::new();
    provider_selector_button.set_hexpand(true);
    provider_selector_button.set_halign(gtk::Align::Fill);
    provider_selector_button.set_width_request(320);
    provider_selector_button.set_widget_name("opencode-provider-selector");
    provider_selector_button.add_css_class("opencode-provider-selector");
    let provider_selector_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    provider_selector_content.add_css_class("opencode-provider-selector-content");
    let provider_selector_label = gtk::Label::new(Some("(None)"));
    provider_selector_label.set_widget_name("opencode-provider-selector-label");
    provider_selector_label.set_xalign(0.0);
    provider_selector_label.set_hexpand(true);
    provider_selector_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    provider_selector_label.add_css_class("opencode-provider-selector-label");
    let provider_selector_arrow = gtk::Image::from_icon_name("pan-down-symbolic");
    provider_selector_content.append(&provider_selector_label);
    provider_selector_content.append(&provider_selector_arrow);
    provider_selector_button.set_child(Some(&provider_selector_content));
    let refresh_providers_button = gtk::Button::with_label("Refresh");
    let provider_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    provider_row.set_visible(runtime_only);
    provider_row.append(&provider_selector_button);
    provider_row.append(&refresh_providers_button);
    auth_section.append(&provider_row);
    let provider_search_entry = gtk::SearchEntry::new();
    provider_search_entry.set_placeholder_text(Some("Filter providers"));
    provider_search_entry.set_visible(runtime_only);
    provider_search_entry.set_width_request(320);
    provider_search_entry.set_widget_name("opencode-provider-search");
    provider_search_entry.add_css_class("opencode-provider-search");
    let provider_picker_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    provider_picker_list.set_widget_name("opencode-provider-picker-list");
    provider_picker_list.set_margin_end(8);
    provider_picker_list.add_css_class("opencode-provider-picker-list");
    let provider_picker_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(180)
        .max_content_height(280)
        .child(&provider_picker_list)
        .build();
    provider_picker_scroll.set_has_frame(false);
    provider_picker_scroll.set_overlay_scrolling(false);
    provider_picker_scroll.set_width_request(320);
    provider_picker_scroll.set_widget_name("opencode-provider-picker-scroll");
    provider_picker_scroll.add_css_class("opencode-provider-picker-scroll");
    let provider_picker_root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    provider_picker_root.set_margin_start(8);
    provider_picker_root.set_margin_end(8);
    provider_picker_root.set_margin_top(8);
    provider_picker_root.set_margin_bottom(8);
    provider_picker_root.set_width_request(320);
    provider_picker_root.set_widget_name("opencode-provider-picker-root");
    provider_picker_root.add_css_class("opencode-provider-picker-root");
    provider_picker_root.append(&provider_search_entry);
    provider_picker_root.append(&provider_picker_scroll);
    let provider_picker_popover = gtk::Popover::new();
    provider_picker_popover.set_has_arrow(true);
    provider_picker_popover.set_autohide(true);
    provider_picker_popover.set_position(gtk::PositionType::Bottom);
    provider_picker_popover.set_widget_name("opencode-provider-picker-popover");
    provider_picker_popover.set_parent(&provider_selector_button);
    provider_picker_popover.set_child(Some(&provider_picker_root));
    let provider_hint_label = gtk::Label::new(Some(""));
    provider_hint_label.set_xalign(0.0);
    provider_hint_label.set_wrap(true);
    provider_hint_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    provider_hint_label.add_css_class("dim-label");
    provider_hint_label.set_visible(runtime_only);
    auth_section.append(&provider_hint_label);
    let auth_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    auth_actions.add_css_class("profile-auth-actions");
    let login_button = gtk::Button::with_label("Start Login");
    let api_key_button = gtk::Button::with_label("Set API Key");
    let logout_button = gtk::Button::with_label("Logout");
    auth_actions.append(&login_button);
    auth_actions.append(&api_key_button);
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
    let login_link_title = gtk::Label::new(Some("Provider Login"));
    login_link_title.add_css_class("profile-section-title");
    login_link_title.set_xalign(0.0);
    login_link_section.append(&login_link_title);
    let login_link_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let login_link_entry = gtk::Entry::new();
    login_link_entry.set_editable(false);
    login_link_entry.set_hexpand(true);
    login_link_entry.set_placeholder_text(Some("Provider login URL will appear here"));
    let open_link_button = gtk::Button::with_label("Open");
    open_link_button.set_sensitive(false);
    let copy_link_button = gtk::Button::with_label("Copy");
    copy_link_button.set_sensitive(false);
    login_link_row.append(&login_link_entry);
    login_link_row.append(&open_link_button);
    login_link_row.append(&copy_link_button);
    login_link_section.append(&login_link_row);
    let login_device_code_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    login_device_code_row.set_visible(false);
    let login_device_code_label = gtk::Label::new(Some("Code"));
    login_device_code_label.set_xalign(0.0);
    login_device_code_label.add_css_class("dim-label");
    let login_device_code_value = gtk::Label::new(None);
    login_device_code_value.set_xalign(0.0);
    login_device_code_value.set_hexpand(true);
    login_device_code_value.add_css_class("monospace");
    let copy_device_code_button = gtk::Button::with_label("Copy Code");
    copy_device_code_button.set_sensitive(false);
    login_device_code_row.append(&login_device_code_label);
    login_device_code_row.append(&login_device_code_value);
    login_device_code_row.append(&copy_device_code_button);
    login_link_section.append(&login_device_code_row);
    let login_link_hint = gtk::Label::new(None);
    login_link_hint.set_xalign(0.0);
    login_link_hint.set_wrap(true);
    login_link_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    login_link_hint.add_css_class("dim-label");
    login_link_hint.set_visible(false);
    login_link_section.append(&login_link_hint);
    let login_waiting_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    login_waiting_row.set_visible(false);
    let login_waiting_spinner = gtk::Spinner::new();
    let login_waiting_label = gtk::Label::new(Some("Waiting for authorization..."));
    login_waiting_label.set_xalign(0.0);
    login_waiting_label.add_css_class("dim-label");
    login_waiting_row.append(&login_waiting_spinner);
    login_waiting_row.append(&login_waiting_label);
    login_link_section.append(&login_waiting_row);
    let login_code_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    login_code_row.set_visible(false);
    let login_code_entry = gtk::Entry::new();
    login_code_entry.set_hexpand(true);
    login_code_entry.set_placeholder_text(Some("Paste authorization code"));
    let login_code_button = gtk::Button::with_label("Complete Login");
    login_code_row.append(&login_code_entry);
    login_code_row.append(&login_code_button);
    login_link_section.append(&login_code_row);
    root.append(&login_link_section);

    let all_provider_items: Rc<RefCell<Vec<crate::backend::AccountProviderInfo>>> =
        Rc::new(RefCell::new(Vec::new()));
    let provider_items: Rc<RefCell<Vec<crate::backend::AccountProviderInfo>>> =
        Rc::new(RefCell::new(Vec::new()));
    let selected_provider_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let refresh_provider_actions_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> =
        Rc::new(RefCell::new(None));
    let refresh_provider_selector_label: Rc<dyn Fn()> = {
        let all_provider_items = all_provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        let provider_selector_label = provider_selector_label.clone();
        Rc::new(move || {
            let selected = selected_provider_id.borrow().clone();
            let label = selected
                .as_deref()
                .and_then(|provider_id| {
                    all_provider_items
                        .borrow()
                        .iter()
                        .find(|provider| provider.provider_id == provider_id)
                        .map(opencode_provider_dropdown_label)
                })
                .unwrap_or_else(|| "(None)".to_string());
            provider_selector_label.set_text(&label);
        })
    };
    let apply_provider_filter: Rc<dyn Fn(Option<String>)> = {
        let all_provider_items = all_provider_items.clone();
        let provider_items = provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        let provider_picker_list = provider_picker_list.clone();
        let provider_picker_popover = provider_picker_popover.clone();
        let provider_hint_label = provider_hint_label.clone();
        let provider_search_entry = provider_search_entry.clone();
        let refresh_provider_actions_handle = refresh_provider_actions_handle.clone();
        let refresh_provider_selector_label = refresh_provider_selector_label.clone();
        Rc::new(move |preferred_provider_id: Option<String>| {
            let query = provider_search_entry.text().trim().to_lowercase();
            let filtered = all_provider_items
                .borrow()
                .iter()
                .filter(|provider| {
                    query.is_empty()
                        || provider.provider_name.to_lowercase().contains(&query)
                        || provider.provider_id.to_lowercase().contains(&query)
                })
                .cloned()
                .collect::<Vec<_>>();
            while let Some(child) = provider_picker_list.first_child() {
                provider_picker_list.remove(&child);
            }
            let next_selected = preferred_provider_id
                .and_then(|provider_id| {
                    filtered
                        .iter()
                        .position(|provider| provider.provider_id == provider_id)
                })
                .or_else(|| (!filtered.is_empty()).then_some(0));
            provider_items.replace(filtered);
            if let Some(index) = next_selected {
                if let Some(provider) = provider_items.borrow().get(index).cloned() {
                    selected_provider_id.replace(Some(provider.provider_id.clone()));
                }
            } else {
                if query.is_empty() {
                    selected_provider_id.replace(None);
                } else {
                    provider_hint_label.set_text("No providers match the current filter.");
                    provider_hint_label.set_visible(true);
                }
            }
            for provider in provider_items.borrow().iter().cloned() {
                let row_button = gtk::Button::new();
                row_button.set_widget_name("opencode-provider-picker-row");
                row_button.set_halign(gtk::Align::Fill);
                row_button.set_hexpand(true);
                row_button.set_has_frame(false);
                row_button.add_css_class("opencode-provider-picker-row");
                let row_label = gtk::Label::new(Some(&opencode_provider_dropdown_label(&provider)));
                row_label.set_widget_name("opencode-provider-picker-row-label");
                row_label.set_xalign(0.0);
                row_label.set_hexpand(true);
                row_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                row_label.add_css_class("opencode-provider-picker-row-label");
                row_button.set_child(Some(&row_label));
                let selected_provider_id = selected_provider_id.clone();
                let refresh_provider_selector_label = refresh_provider_selector_label.clone();
                let provider_picker_popover = provider_picker_popover.clone();
                let provider_hint_label = provider_hint_label.clone();
                let refresh_provider_actions_handle = refresh_provider_actions_handle.clone();
                let provider_id = provider.provider_id.clone();
                let provider_name = provider.provider_name.clone();
                row_button.connect_clicked(move |_| {
                    selected_provider_id.replace(Some(provider_id.clone()));
                    refresh_provider_selector_label();
                    provider_hint_label.set_text(&provider_name);
                    provider_picker_popover.popdown();
                    if let Some(refresh) = refresh_provider_actions_handle.borrow().as_ref() {
                        refresh();
                    }
                });
                provider_picker_list.append(&row_button);
            }
            refresh_provider_selector_label();
        })
    };
    let refresh_provider_actions: Rc<dyn Fn()> = {
        let all_provider_items = all_provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        let provider_hint_label = provider_hint_label.clone();
        let login_button = login_button.clone();
        let api_key_button = api_key_button.clone();
        let logout_button = logout_button.clone();
        Rc::new(move || {
            if !runtime_only {
                return;
            }
            let items = all_provider_items.borrow();
            if items.is_empty() {
                provider_hint_label.set_text("No providers reported by OpenCode yet.");
                provider_hint_label.set_visible(true);
                login_button.set_sensitive(false);
                api_key_button.set_sensitive(false);
                logout_button.set_sensitive(false);
                return;
            }
            let Some(selected_id) = selected_provider_id.borrow().clone() else {
                provider_hint_label.set_text("Select a provider.");
                provider_hint_label.set_visible(true);
                login_button.set_sensitive(false);
                api_key_button.set_sensitive(false);
                logout_button.set_sensitive(false);
                return;
            };
            let Some(provider) = items
                .iter()
                .find(|provider| provider.provider_id == selected_id)
            else {
                provider_hint_label.set_text("Select a provider.");
                provider_hint_label.set_visible(true);
                login_button.set_sensitive(false);
                api_key_button.set_sensitive(false);
                logout_button.set_sensitive(false);
                return;
            };
            let status_text = if provider.connected {
                "Connected and ready to use.".to_string()
            } else if provider.has_saved_auth && provider.supports_api_key {
                "API key saved. Ready to use. No extra connect step is required.".to_string()
            } else if provider.has_saved_auth {
                "Credentials saved.".to_string()
            } else {
                match (provider.supports_oauth, provider.supports_api_key) {
                    (true, true) => {
                        "Not configured yet. Use OAuth Login or Set Provider API Key.".to_string()
                    }
                    (true, false) => "Not configured yet. Use OAuth Login.".to_string(),
                    (false, true) => "Not configured yet. Use Set Provider API Key.".to_string(),
                    (false, false) => "No auth methods reported.".to_string(),
                }
            };
            provider_hint_label.set_text(&status_text);
            provider_hint_label.set_visible(true);
            login_button.set_sensitive(provider.supports_oauth);
            api_key_button.set_sensitive(provider.supports_api_key);
            login_button.set_visible(provider.supports_oauth);
            api_key_button.set_visible(provider.supports_api_key);
            let can_remove = provider.connected || provider.has_saved_auth;
            logout_button.set_sensitive(can_remove);
            logout_button.set_visible(true);
        })
    };
    refresh_provider_actions_handle
        .borrow_mut()
        .replace(refresh_provider_actions.clone());
    let refresh_runtime_only_models_cache: Rc<dyn Fn(i64)> = {
        let manager = manager.clone();
        Rc::new(move |profile_id: i64| {
            crate::ui::components::chat::runtime_controls::invalidate_model_options_cache_for_backend(
                "opencode",
            );
            if let Ok(client) = manager.ensure_started(profile_id) {
                crate::ui::components::chat::runtime_controls::refresh_model_options_cache_async(
                    Some(client),
                );
            }
        })
    };
    let reload_runtime_only_providers: Rc<dyn Fn()> = {
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let all_provider_items = all_provider_items.clone();
        let provider_items = provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        let provider_picker_list = provider_picker_list.clone();
        let provider_hint_label = provider_hint_label.clone();
        let provider_search_entry = provider_search_entry.clone();
        let operation_label = operation_label.clone();
        let apply_provider_filter = apply_provider_filter.clone();
        let refresh_provider_actions = refresh_provider_actions.clone();
        let account_label = account_label.clone();
        let refresh_runtime_only_models_cache = refresh_runtime_only_models_cache.clone();
        Rc::new(move || {
            if !runtime_only {
                return;
            }
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                all_provider_items.replace(Vec::new());
                provider_items.replace(Vec::new());
                selected_provider_id.replace(None);
                while let Some(child) = provider_picker_list.first_child() {
                    provider_picker_list.remove(&child);
                }
                account_label.set_text("Providers with auth: none");
                provider_hint_label.set_text("OpenCode profile is unavailable.");
                provider_hint_label.set_visible(true);
                operation_label.set_visible(false);
                refresh_provider_actions();
                return;
            };
            let Ok(client) = manager.ensure_started(profile_id) else {
                all_provider_items.replace(Vec::new());
                provider_items.replace(Vec::new());
                selected_provider_id.replace(None);
                while let Some(child) = provider_picker_list.first_child() {
                    provider_picker_list.remove(&child);
                }
                account_label.set_text("Providers with auth: none");
                provider_hint_label.set_text("Unable to start OpenCode to load providers.");
                provider_hint_label.set_visible(true);
                refresh_provider_actions();
                return;
            };
            refresh_runtime_only_models_cache(profile_id);
            operation_label.set_visible(true);
            operation_label.set_text("Loading OpenCode providers...");
            let (tx, rx) =
                mpsc::channel::<Result<Vec<crate::backend::AccountProviderInfo>, String>>();
            thread::spawn(move || {
                let _ = tx.send(client.account_provider_list());
            });
            let all_provider_items = all_provider_items.clone();
            let provider_items = provider_items.clone();
            let selected_provider_id = selected_provider_id.clone();
            let provider_picker_list = provider_picker_list.clone();
            let provider_hint_label = provider_hint_label.clone();
            let apply_provider_filter = apply_provider_filter.clone();
            let operation_label = operation_label.clone();
            let refresh_provider_actions = refresh_provider_actions.clone();
            let account_label = account_label.clone();
            let provider_search_entry_for_timeout = provider_search_entry.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
                Ok(Ok(providers)) => {
                    let selected_id = selected_provider_id.borrow().clone();
                    let connected = providers
                        .iter()
                        .filter(|provider| provider.connected || provider.has_saved_auth)
                        .map(|provider| provider.provider_name.as_str())
                        .collect::<Vec<_>>();
                    account_label.set_text(&format!(
                        "Providers with auth: {}",
                        if connected.is_empty() {
                            "none".to_string()
                        } else {
                            connected.join(", ")
                        }
                    ));
                    let next_selected = selected_id
                        .and_then(|provider_id| {
                            providers
                                .iter()
                                .position(|provider| provider.provider_id == provider_id)
                        })
                        .or_else(|| (!providers.is_empty()).then_some(0))
                        .and_then(|index| {
                            providers
                                .get(index)
                                .map(|provider| provider.provider_id.clone())
                        });
                    all_provider_items.replace(providers);
                    apply_provider_filter(next_selected);
                    if provider_items.borrow().is_empty()
                        && provider_search_entry_for_timeout.text().trim().is_empty()
                    {
                        provider_hint_label.set_text("No providers reported by OpenCode.");
                        provider_hint_label.set_visible(true);
                    }
                    operation_label.set_visible(false);
                    refresh_provider_actions();
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    all_provider_items.replace(Vec::new());
                    provider_items.replace(Vec::new());
                    selected_provider_id.replace(None);
                    while let Some(child) = provider_picker_list.first_child() {
                        provider_picker_list.remove(&child);
                    }
                    account_label.set_text("Providers with auth: none");
                    provider_hint_label.set_text(&format!("Unable to load providers: {err}"));
                    provider_hint_label.set_visible(true);
                    operation_label.set_visible(false);
                    refresh_provider_actions();
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    operation_label.set_visible(false);
                    gtk::glib::ControlFlow::Break
                }
            });
        })
    };
    let show_runtime_models_dialog: Rc<dyn Fn(i64)> = {
        let manager = manager.clone();
        let dialog = dialog.clone();
        let db = db.clone();
        Rc::new(move |profile_id: i64| {
            let models_window = gtk::Window::builder()
                .title("OpenCode Models")
                .default_width(640)
                .default_height(560)
                .modal(true)
                .transient_for(&dialog)
                .build();
            models_window.add_css_class("settings-window");

            let models_root = gtk::Box::new(gtk::Orientation::Vertical, 8);
            models_root.set_margin_start(12);
            models_root.set_margin_end(12);
            models_root.set_margin_top(12);
            models_root.set_margin_bottom(12);

            let models_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let models_back_button = gtk::Button::with_label("Back");
            models_back_button.set_visible(false);
            let models_title = gtk::Label::new(Some("Available Models"));
            models_title.add_css_class("profile-section-title");
            models_title.set_xalign(0.0);
            models_title.set_hexpand(true);
            let models_refresh_button = gtk::Button::with_label("Refresh");
            models_header.append(&models_back_button);
            models_header.append(&models_title);
            models_header.append(&models_refresh_button);
            models_root.append(&models_header);

            let models_status = gtk::Label::new(Some("Loading models from OpenCode..."));
            models_status.add_css_class("dim-label");
            models_status.set_xalign(0.0);
            models_root.append(&models_status);

            let models_scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .min_content_height(420)
                .build();
            models_scroll.set_hexpand(true);
            models_scroll.set_vexpand(true);
            let models_stack = gtk::Stack::new();
            models_stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
            models_stack.set_transition_duration(180);
            let provider_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
            provider_list.set_valign(gtk::Align::Start);
            let provider_models = gtk::Box::new(gtk::Orientation::Vertical, 8);
            provider_models.set_valign(gtk::Align::Start);
            models_stack.add_named(&provider_list, Some("providers"));
            models_stack.add_named(&provider_models, Some("models"));
            models_stack.set_visible_child_name("providers");
            models_scroll.set_child(Some(&models_stack));
            models_root.append(&models_scroll);
            models_window.set_child(Some(&models_root));
            let selected_provider_name: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

            {
                let models_back_button = models_back_button.clone();
                let models_stack = models_stack.clone();
                let models_title = models_title.clone();
                let selected_provider_name = selected_provider_name.clone();
                let models_back_button_for_click = models_back_button.clone();
                models_back_button.connect_clicked(move |_| {
                    selected_provider_name.replace(None);
                    models_stack.set_visible_child_name("providers");
                    models_title.set_text("Available Providers");
                    models_back_button_for_click.set_visible(false);
                });
            }

            let reload_models: Rc<dyn Fn()> = {
                let manager = manager.clone();
                let db = db.clone();
                let selected_provider_name = selected_provider_name.clone();
                let models_back_button = models_back_button.clone();
                let models_title = models_title.clone();
                let models_status = models_status.clone();
                let models_stack = models_stack.clone();
                let provider_list = provider_list.clone();
                let provider_models = provider_models.clone();
                Rc::new(move || {
                    while let Some(child) = provider_list.first_child() {
                        provider_list.remove(&child);
                    }
                    while let Some(child) = provider_models.first_child() {
                        provider_models.remove(&child);
                    }
                    models_title.set_text("Available Providers");
                    models_back_button.set_visible(false);
                    models_stack.set_visible_child_name("providers");
                    models_status.set_visible(true);
                    models_status.set_text("Loading models from OpenCode...");
                    let Ok(client) = manager.ensure_started(profile_id) else {
                        models_status.set_text("Unable to start OpenCode runtime.");
                        return;
                    };
                    let visible_models =
                        crate::ui::components::chat::runtime_controls::refresh_model_options_cache(
                            Some(&client),
                        );
                    let all_models =
                        crate::ui::components::chat::runtime_controls::model_options_unfiltered(
                            Some(&client),
                        );
                    let hidden_models =
                        crate::ui::components::chat::runtime_controls::hidden_opencode_model_ids(
                            profile_id,
                        );
                    let (tx, rx) = mpsc::channel::<
                        Result<
                            (
                                Vec<crate::backend::AccountProviderInfo>,
                                Vec<crate::codex_appserver::ModelInfo>,
                                HashSet<String>,
                                usize,
                            ),
                            String,
                        >,
                    >();
                    thread::spawn(move || {
                        let result = client.account_provider_list().map(|providers| {
                            (providers, all_models, hidden_models, visible_models.len())
                        });
                        let _ = tx.send(result);
                    });
                    let db = db.clone();
                    let selected_provider_name = selected_provider_name.clone();
                    let models_back_button = models_back_button.clone();
                    let models_title = models_title.clone();
                    let models_status = models_status.clone();
                    let models_stack = models_stack.clone();
                    let provider_list = provider_list.clone();
                    let provider_models = provider_models.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                        match rx.try_recv() {
                            Ok(Ok((providers, models, hidden_models, visible_count))) => {
                                let mut authed_provider_names = BTreeMap::<String, String>::new();
                                let mut provider_name_to_id = HashMap::<String, String>::new();
                                for provider in &providers {
                                    provider_name_to_id.insert(
                                        provider.provider_name.to_lowercase(),
                                        provider.provider_id.clone(),
                                    );
                                    if provider.connected || provider.has_saved_auth {
                                        authed_provider_names.insert(
                                            provider.provider_id.clone(),
                                            provider.provider_name.clone(),
                                        );
                                    }
                                }
                                if authed_provider_names.is_empty() {
                                    models_status.set_text("No providers with auth are connected.");
                                    return gtk::glib::ControlFlow::Break;
                                }

                                let mut grouped_models =
                                    BTreeMap::<String, Vec<(String, String)>>::new();
                                for provider_name in authed_provider_names.values() {
                                    grouped_models.insert(provider_name.clone(), Vec::new());
                                }

                                for model in models {
                                    let provider_id = model
                                        .id
                                        .split_once(':')
                                        .map(|(provider_id, _)| provider_id.to_string())
                                        .or_else(|| {
                                            model.display_name.split_once(" / ").and_then(
                                                |(provider_name, _)| {
                                                    provider_name_to_id
                                                        .get(&provider_name.to_lowercase())
                                                        .cloned()
                                                },
                                            )
                                        });
                                    let Some(provider_id) = provider_id else {
                                        continue;
                                    };
                                    let Some(provider_name) =
                                        authed_provider_names.get(&provider_id)
                                    else {
                                        continue;
                                    };
                                    let model_name = model
                                        .display_name
                                        .split_once(" / ")
                                        .map(|(_, model_name)| model_name.to_string())
                                        .unwrap_or(model.display_name);
                                    grouped_models
                                        .entry(provider_name.clone())
                                        .or_default()
                                        .push((model_name, model.id));
                                }

                                let mut total_models = 0usize;
                                for model_names in grouped_models.values_mut() {
                                    model_names.sort_by(|left, right| left.0.cmp(&right.0));
                                    model_names.dedup_by(|left, right| left.1 == right.1);
                                    total_models += model_names.len();
                                }
                                if total_models == 0 {
                                    models_status
                                        .set_text("No models reported for providers with auth.");
                                } else {
                                    models_status.set_text(&format!(
                                        "{visible_count} visible / {total_models} total across {} providers with auth.",
                                        grouped_models.len(),
                                    ));
                                }

                                let grouped_models = Rc::new(grouped_models);
                                let render_provider_detail: Rc<dyn Fn(&str)> = {
                                    let db = db.clone();
                                    let grouped_models = grouped_models.clone();
                                    let hidden_models = hidden_models.clone();
                                    let models_status = models_status.clone();
                                    let models_title = models_title.clone();
                                    let models_back_button = models_back_button.clone();
                                    let models_stack = models_stack.clone();
                                    let provider_models = provider_models.clone();
                                    let selected_provider_name = selected_provider_name.clone();
                                    Rc::new(move |provider_name: &str| {
                                        while let Some(child) = provider_models.first_child() {
                                            provider_models.remove(&child);
                                        }
                                        selected_provider_name
                                            .replace(Some(provider_name.to_string()));
                                        models_title.set_text(provider_name);
                                        models_back_button.set_visible(true);
                                        models_stack.set_visible_child_name("models");
                                        let model_names = grouped_models
                                            .get(provider_name)
                                            .cloned()
                                            .unwrap_or_default();
                                        if model_names.is_empty() {
                                            let empty =
                                                gtk::Label::new(Some("No models reported."));
                                            empty.add_css_class("dim-label");
                                            empty.set_xalign(0.0);
                                            provider_models.append(&empty);
                                            return;
                                        }
                                        for (model_name, model_id) in model_names {
                                            let row = gtk::CheckButton::with_label(&model_name);
                                            row.set_halign(gtk::Align::Fill);
                                            row.set_hexpand(true);
                                            row.set_active(!hidden_models.contains(&model_id));
                                            let db = db.clone();
                                            let models_status = models_status.clone();
                                            let model_id_for_toggle = model_id.clone();
                                            row.connect_toggled(move |toggle| {
                                                let hidden = !toggle.is_active();
                                                if let Err(err) =
                                                    crate::ui::components::chat::runtime_controls::set_opencode_model_hidden(
                                                        &db,
                                                        profile_id,
                                                        &model_id_for_toggle,
                                                        hidden,
                                                    )
                                                {
                                                    eprintln!(
                                                        "failed to update OpenCode model visibility for {}: {}",
                                                        model_id_for_toggle, err
                                                    );
                                                    return;
                                                }
                                                models_status.set_text(if hidden {
                                                    "Hidden from composer."
                                                } else {
                                                    "Visible in composer."
                                                });
                                            });
                                            provider_models.append(&row);
                                        }
                                    })
                                };

                                for (provider_name, model_names) in grouped_models.iter() {
                                    let provider_button = gtk::Button::new();
                                    provider_button.set_has_frame(false);
                                    provider_button.add_css_class("profile-settings-section");
                                    provider_button.set_halign(gtk::Align::Fill);
                                    provider_button.set_hexpand(true);
                                    let provider_row =
                                        gtk::Box::new(gtk::Orientation::Horizontal, 8);
                                    let provider_title = gtk::Label::new(Some(provider_name));
                                    provider_title.add_css_class("profile-section-title");
                                    provider_title.set_xalign(0.0);
                                    provider_title.set_hexpand(true);
                                    let provider_count = gtk::Label::new(Some(&format!(
                                        "{} models",
                                        model_names.len()
                                    )));
                                    provider_count.add_css_class("dim-label");
                                    provider_row.append(&provider_title);
                                    provider_row.append(&provider_count);
                                    provider_button.set_child(Some(&provider_row));
                                    let render_provider_detail = render_provider_detail.clone();
                                    let provider_name_for_click = provider_name.clone();
                                    provider_button.connect_clicked(move |_| {
                                        render_provider_detail(&provider_name_for_click);
                                    });
                                    provider_list.append(&provider_button);
                                }

                                if let Some(selected_provider_name) =
                                    selected_provider_name.borrow().clone()
                                {
                                    if grouped_models.contains_key(&selected_provider_name) {
                                        render_provider_detail(&selected_provider_name);
                                    }
                                }
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                models_status.set_text(&format!("Unable to load models: {err}"));
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                models_status.set_text("Model loading was interrupted.");
                                gtk::glib::ControlFlow::Break
                            }
                        }
                    });
                })
            };
            {
                let reload_models = reload_models.clone();
                models_refresh_button.connect_clicked(move |_| {
                    reload_models();
                });
            }
            reload_models();
            models_window.present();
        })
    };

    let refresh_ui: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let status_label = status_label.clone();
        let account_label = account_label.clone();
        let login_button = login_button.clone();
        let api_key_button = api_key_button.clone();
        let logout_button = logout_button.clone();
        let start_button = start_button.clone();
        let stop_button = stop_button.clone();
        let restart_button = restart_button.clone();
        let remove_profile_button = remove_profile_button.clone();
        let runtime_section = runtime_section.clone();
        let auth_section = auth_section.clone();
        let process_actions = process_actions.clone();
        let auth_actions = auth_actions.clone();
        let provider_row = provider_row.clone();
        let provider_hint_label = provider_hint_label.clone();
        let provider_items = provider_items.clone();
        let system_profile_note = system_profile_note.clone();
        let selected_profile_icon_button = selected_profile_icon_button.clone();
        let selected_profile_icon_name = selected_profile_icon_name.clone();
        let selected_profile_icon_image = selected_profile_icon_image.clone();
        let refresh_provider_actions = refresh_provider_actions.clone();
        Rc::new(move || {
            let ids = profile_ids.borrow();
            let Some(profile_id) = selected_profile_id(&profile_combo, &ids) else {
                return;
            };
            if let Ok(Some(profile)) = db.get_codex_profile(profile_id) {
                let capabilities =
                    crate::backend::capabilities_for_backend_kind(&profile.backend_kind);
                let backend_name = crate::backend::backend_display_name(&profile.backend_kind);
                let profile_icon_name = normalize_profile_icon_name(Some(&profile.icon_name));
                selected_profile_icon_name.replace(profile_icon_name.clone());
                selected_profile_icon_image.set_icon_name(Some(&profile_icon_name));
                selected_profile_icon_button
                    .set_tooltip_text(Some(profile_icon_label(&profile_icon_name)));
                let is_running = profile.status.eq_ignore_ascii_case("running")
                    || manager.running_client_for_profile(profile_id).is_some();
                let runtime_state = if is_running { "running" } else { "stopped" };
                if runtime_only {
                    status_label.set_text(&format!("Runtime status: {runtime_state}"));
                } else {
                    status_label.set_text(&format!("Profile status: {runtime_state}"));
                }
                let account = profile
                    .last_email
                    .clone()
                    .or(profile.last_account_type.clone())
                    .unwrap_or_else(|| "not logged in".to_string());
                if runtime_only {
                    if provider_items.borrow().is_empty() {
                        account_label.set_text("Providers with auth: loading...");
                    }
                } else {
                    account_label.set_text(&format!("{backend_name} account: {}", account));
                }

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
                let show_runtime = runtime_only || !is_system;
                let show_auth = runtime_only || !is_system;
                remove_profile_button.set_sensitive(!runtime_only && can_remove);
                remove_profile_button.set_visible(!runtime_only && !is_system);
                runtime_section.set_visible(show_runtime);
                auth_section.set_visible(show_auth);
                process_actions.set_visible(show_runtime);
                auth_actions.set_visible(show_auth);
                provider_row.set_visible(show_auth && runtime_only);
                provider_hint_label.set_visible(show_auth && runtime_only);
                system_profile_note.set_visible(!runtime_only && is_system);
                logout_button.set_sensitive(has_account && capabilities.supports_logout);
                logout_button.set_visible(capabilities.supports_logout);
                api_key_button.set_sensitive(capabilities.supports_api_key_login);
                api_key_button.set_visible(profile.backend_kind.eq_ignore_ascii_case("opencode"));
                if runtime_only {
                    login_button.set_label("OAuth Login");
                    api_key_button.set_label("Set Provider API Key");
                    logout_button.set_label("Remove Provider");
                } else if has_account {
                    api_key_button.set_label("Set API Key");
                    logout_button.set_label("Logout");
                    login_button.set_label("Reauthenticate");
                } else {
                    api_key_button.set_label("Set API Key");
                    logout_button.set_label("Logout");
                    login_button.set_label("Start Login");
                }
                login_button.set_sensitive(capabilities.supports_oauth_login);
                if !capabilities.supports_oauth_login {
                    login_button.set_tooltip_text(Some(
                        "This runtime does not expose an OAuth login flow from Enzim.",
                    ));
                } else {
                    login_button.set_tooltip_text(None);
                }
                if runtime_only {
                    refresh_provider_actions();
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
        let provider_picker_popover = provider_picker_popover.clone();
        let provider_search_entry = provider_search_entry.clone();
        provider_selector_button.connect_clicked(move |_| {
            if provider_picker_popover.is_visible() {
                provider_picker_popover.popdown();
            } else {
                provider_picker_popover.popup();
                provider_search_entry.grab_focus();
            }
        });
    }
    {
        let selected_provider_id = selected_provider_id.clone();
        let apply_provider_filter = apply_provider_filter.clone();
        let refresh_provider_actions = refresh_provider_actions.clone();
        provider_search_entry.connect_search_changed(move |_| {
            let selected_id = selected_provider_id.borrow().clone();
            apply_provider_filter(selected_id);
            refresh_provider_actions();
        });
    }
    {
        let reload_runtime_only_providers = reload_runtime_only_providers.clone();
        refresh_providers_button.connect_clicked(move |_| {
            reload_runtime_only_providers();
        });
    }
    {
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let operation_label = operation_label.clone();
        let show_runtime_models_dialog = show_runtime_models_dialog.clone();
        models_button.connect_clicked(move |_| {
            if !runtime_only {
                return;
            }
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                operation_label.set_visible(true);
                operation_label.set_text("OpenCode profile is unavailable.");
                return;
            };
            show_runtime_models_dialog(profile_id);
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
    if runtime_only {
        reload_runtime_only_providers();
    }

    {
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let reload_runtime_only_providers = reload_runtime_only_providers.clone();
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
                reload_runtime_only_providers();
            }
        });
    }
    {
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let provider_items = provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        let provider_picker_list = provider_picker_list.clone();
        let refresh_provider_selector_label = refresh_provider_selector_label.clone();
        let provider_hint_label = provider_hint_label.clone();
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
                if runtime_only {
                    provider_items.replace(Vec::new());
                    selected_provider_id.replace(None);
                    while let Some(child) = provider_picker_list.first_child() {
                        provider_picker_list.remove(&child);
                    }
                    refresh_provider_selector_label();
                    provider_hint_label.set_text("Start OpenCode to load providers.");
                    provider_hint_label.set_visible(true);
                }
            }
        });
    }
    {
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let reload_runtime_only_providers = reload_runtime_only_providers.clone();
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
                reload_runtime_only_providers();
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
            if !allow_create {
                return;
            }
            let prompt = gtk::Window::builder()
                .title("Create Runtime Profile")
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
                    let backend_kind = backend_filter.unwrap_or("codex");
                    if let Ok(profile) = manager.create_profile(&name, backend_kind) {
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
        let login_link_entry = login_link_entry.clone();
        open_link_button.connect_clicked(move |_| {
            open_uri_in_browser(login_link_entry.text().as_str());
        });
    }
    {
        let login_device_code_value = login_device_code_value.clone();
        copy_device_code_button.connect_clicked(move |_| {
            let text = login_device_code_value.text();
            if text.is_empty() {
                return;
            }
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(text.as_str());
            }
        });
    }

    let pending_runtime_oauth_flow: Rc<RefCell<Option<(i64, crate::backend::OAuthFlowInfo)>>> =
        Rc::new(RefCell::new(None));
    let complete_runtime_oauth_flow: Rc<
        dyn Fn(i64, crate::backend::OAuthFlowInfo, Option<String>),
    > = {
        let db = db.clone();
        let manager = manager.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let operation_label = operation_label.clone();
        let status_label = status_label.clone();
        let account_label = account_label.clone();
        let profile_ids = profile_ids.clone();
        let reload_runtime_only_providers = reload_runtime_only_providers.clone();
        let login_link_section = login_link_section.clone();
        let login_code_row = login_code_row.clone();
        let login_code_entry = login_code_entry.clone();
        let login_code_button = login_code_button.clone();
        let login_device_code_row = login_device_code_row.clone();
        let login_device_code_value = login_device_code_value.clone();
        let copy_device_code_button = copy_device_code_button.clone();
        let login_link_hint = login_link_hint.clone();
        let login_waiting_row = login_waiting_row.clone();
        let login_waiting_spinner = login_waiting_spinner.clone();
        let pending_runtime_oauth_flow = pending_runtime_oauth_flow.clone();
        Rc::new(
            move |profile_id: i64, flow: crate::backend::OAuthFlowInfo, code: Option<String>| {
                operation_label.set_visible(true);
                operation_label.set_text("Completing OAuth login...");
                login_code_button.set_sensitive(false);
                login_waiting_row.set_visible(true);
                login_waiting_spinner.start();
                let Ok(client) = manager.ensure_started(profile_id) else {
                    operation_label.set_text("Failed to keep OpenCode runtime active.");
                    login_code_button.set_sensitive(true);
                    login_waiting_row.set_visible(false);
                    login_waiting_spinner.stop();
                    return;
                };
                let provider_id = flow.provider_id.clone();
                let method_index = flow.method_index;
                let (tx, rx) =
                    mpsc::channel::<Result<Option<crate::codex_appserver::AccountInfo>, String>>();
                thread::spawn(move || {
                    let _ = tx.send(client.account_complete_oauth_for_provider(
                        &provider_id,
                        method_index,
                        code.as_deref(),
                    ));
                });
                let db = db.clone();
                let manager = manager.clone();
                let sync_profile_dropdown = sync_profile_dropdown.clone();
                let refresh_ui = refresh_ui.clone();
                let operation_label = operation_label.clone();
                let status_label = status_label.clone();
                let account_label = account_label.clone();
                let profile_ids = profile_ids.clone();
                let reload_runtime_only_providers = reload_runtime_only_providers.clone();
                let login_link_section = login_link_section.clone();
                let login_code_row = login_code_row.clone();
                let login_code_entry = login_code_entry.clone();
                let login_code_button = login_code_button.clone();
                let login_device_code_row = login_device_code_row.clone();
                let login_device_code_value = login_device_code_value.clone();
                let copy_device_code_button = copy_device_code_button.clone();
                let login_link_hint = login_link_hint.clone();
                let login_waiting_row = login_waiting_row.clone();
                let login_waiting_spinner = login_waiting_spinner.clone();
                let pending_runtime_oauth_flow = pending_runtime_oauth_flow.clone();
                gtk::glib::timeout_add_local(Duration::from_millis(120), move || {
                    match rx.try_recv() {
                        Ok(Ok(account)) => {
                            pending_runtime_oauth_flow.replace(None);
                            login_link_section.set_visible(false);
                            login_code_row.set_visible(false);
                            login_code_entry.set_text("");
                            login_code_button.set_sensitive(true);
                            login_device_code_row.set_visible(false);
                            login_device_code_value.set_text("");
                            copy_device_code_button.set_sensitive(false);
                            login_link_hint.set_visible(false);
                            login_waiting_spinner.stop();
                            login_waiting_row.set_visible(false);
                            if let Some(account) = account {
                                let account_text = account
                                    .email
                                    .clone()
                                    .unwrap_or_else(|| account.account_type.clone());
                                status_label.set_text("Runtime status: running");
                                let connected = account_text
                                    .strip_prefix("OpenCode [")
                                    .and_then(|value| value.strip_suffix(']'))
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())
                                    .unwrap_or(account_text.as_str());
                                account_label
                                    .set_text(&format!("Providers with auth: {connected}"));
                                operation_label.set_text("Login completed.");
                                let _ = db.update_codex_profile_status(profile_id, "running");
                                let _ = crate::ui::components::runtime_auth_dialog::sync_runtime_account_to_db(
                                &db,
                                profile_id,
                                Some(account),
                            );
                                crate::ui::components::runtime_auth_dialog::reload_opencode_runtime_after_auth(
                                &manager,
                                profile_id,
                            );
                                let next_ids = sync_profile_dropdown(Some(profile_id));
                                profile_ids.replace(next_ids);
                                refresh_ui();
                                reload_runtime_only_providers();
                            } else {
                                operation_label.set_text(
                                    "Login completed but no account details were returned.",
                                );
                            }
                            gtk::glib::ControlFlow::Break
                        }
                        Ok(Err(err)) => {
                            operation_label.set_text(&format!("OAuth login failed: {err}"));
                            login_code_button.set_sensitive(true);
                            login_waiting_spinner.stop();
                            login_waiting_row.set_visible(false);
                            gtk::glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            operation_label.set_text("OAuth login stopped unexpectedly.");
                            login_code_button.set_sensitive(true);
                            login_waiting_spinner.stop();
                            login_waiting_row.set_visible(false);
                            gtk::glib::ControlFlow::Break
                        }
                    }
                });
            },
        )
    };
    {
        let pending_runtime_oauth_flow = pending_runtime_oauth_flow.clone();
        let login_code_entry = login_code_entry.clone();
        let operation_label = operation_label.clone();
        let complete_runtime_oauth_flow = complete_runtime_oauth_flow.clone();
        login_code_button.connect_clicked(move |_| {
            let Some((profile_id, flow)) = pending_runtime_oauth_flow.borrow().clone() else {
                operation_label.set_visible(true);
                operation_label.set_text("No OAuth flow is waiting for a callback code.");
                return;
            };
            let code = login_code_entry.text().trim().to_string();
            if code.is_empty() {
                operation_label.set_visible(true);
                operation_label.set_text("Paste the authorization code first.");
                return;
            }
            complete_runtime_oauth_flow(profile_id, flow, Some(code));
        });
    }

    let start_oauth_login_for_provider: Rc<dyn Fn(i64, Option<(String, String)>)> = {
        let db = db.clone();
        let manager = manager.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let operation_label = operation_label.clone();
        let login_link_section = login_link_section.clone();
        let login_link_entry = login_link_entry.clone();
        let login_link_hint = login_link_hint.clone();
        let login_code_row = login_code_row.clone();
        let login_code_entry = login_code_entry.clone();
        let login_code_button = login_code_button.clone();
        let login_device_code_row = login_device_code_row.clone();
        let login_device_code_value = login_device_code_value.clone();
        let copy_device_code_button = copy_device_code_button.clone();
        let copy_link_button = copy_link_button.clone();
        let open_link_button = open_link_button.clone();
        let status_label = status_label.clone();
        let account_label = account_label.clone();
        let profile_ids = profile_ids.clone();
        let reload_runtime_only_providers = reload_runtime_only_providers.clone();
        let pending_runtime_oauth_flow = pending_runtime_oauth_flow.clone();
        let complete_runtime_oauth_flow = complete_runtime_oauth_flow.clone();
        let login_waiting_row = login_waiting_row.clone();
        let login_waiting_spinner = login_waiting_spinner.clone();
        let login_waiting_label = login_waiting_label.clone();
        Rc::new(move |profile_id: i64, provider: Option<(String, String)>| {
            let Ok(client) = manager.ensure_started(profile_id) else {
                operation_label.set_visible(true);
                operation_label.set_text("Unable to start the runtime for authentication.");
                return;
            };
            let next_ids = sync_profile_dropdown(Some(profile_id));
            profile_ids.replace(next_ids);
            refresh_ui();
            operation_label.set_visible(true);
            if let Some((_, provider_name)) = provider.as_ref() {
                operation_label.set_text(&format!("Generating login link for {provider_name}..."));
            } else {
                operation_label.set_text("Generating login link...");
            }
            login_link_section.set_visible(false);
            login_link_entry.set_text("");
            login_link_hint.set_text("");
            login_link_hint.set_visible(false);
            login_code_entry.set_text("");
            login_code_row.set_visible(false);
            login_device_code_value.set_text("");
            login_device_code_row.set_visible(false);
            copy_device_code_button.set_sensitive(false);
            login_code_button.set_sensitive(true);
            pending_runtime_oauth_flow.replace(None);
            copy_link_button.set_sensitive(false);
            open_link_button.set_sensitive(false);
            login_waiting_spinner.stop();
            login_waiting_row.set_visible(false);
            login_waiting_label.set_text("Waiting for authorization...");
            if runtime_only {
                let provider_for_thread = provider.clone();
                let (tx, rx) = mpsc::channel::<Result<crate::backend::OAuthFlowInfo, String>>();
                thread::spawn(move || {
                    let result = match provider_for_thread {
                        Some((provider_id, _)) => {
                            client.account_login_start_oauth_for_provider_info(&provider_id)
                        }
                        None => Err("OpenCode provider is required for OAuth login.".to_string()),
                    };
                    let _ = tx.send(result);
                });
                let operation_label_after_start = operation_label.clone();
                let login_link_section_after_start = login_link_section.clone();
                let login_link_entry_after_start = login_link_entry.clone();
                let login_link_hint_after_start = login_link_hint.clone();
                let login_code_row_after_start = login_code_row.clone();
                let login_code_entry_after_start = login_code_entry.clone();
                let login_device_code_row_after_start = login_device_code_row.clone();
                let login_device_code_value_after_start = login_device_code_value.clone();
                let copy_device_code_button_after_start = copy_device_code_button.clone();
                let copy_link_button_after_start = copy_link_button.clone();
                let open_link_button_after_start = open_link_button.clone();
                let login_waiting_row_after_start = login_waiting_row.clone();
                let login_waiting_spinner_after_start = login_waiting_spinner.clone();
                let login_waiting_label_after_start = login_waiting_label.clone();
                let provider_name = provider.as_ref().map(|(_, name)| name.clone());
                let pending_runtime_oauth_flow_after_start = pending_runtime_oauth_flow.clone();
                let complete_runtime_oauth_flow_after_start = complete_runtime_oauth_flow.clone();
                gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                    match rx.try_recv() {
                        Ok(Ok(flow)) => {
                            login_link_section_after_start.set_visible(true);
                            login_link_entry_after_start.set_text(&flow.url);
                            copy_link_button_after_start.set_sensitive(true);
                            open_link_button_after_start.set_sensitive(true);
                            let is_device_flow = flow.device_code.is_some();
                            let instructions = if is_device_flow {
                                "Open the page above, enter the code below, and finish login in your browser."
                                    .to_string()
                            } else {
                                flow.instructions
                                    .clone()
                                    .filter(|value| !value.trim().is_empty())
                                    .unwrap_or_else(|| {
                                        if flow.method == "code" {
                                            "Open the link above, finish the provider login, then paste the returned authorization code below.".to_string()
                                        } else {
                                            "Open the link above and finish the provider login in your browser.".to_string()
                                        }
                                    })
                            };
                            login_link_hint_after_start.set_text(&instructions);
                            login_link_hint_after_start.set_visible(true);
                            if let Some(device_code) = flow.device_code.as_deref() {
                                login_device_code_value_after_start.set_text(device_code);
                                login_device_code_row_after_start.set_visible(true);
                                copy_device_code_button_after_start.set_sensitive(true);
                            } else {
                                login_device_code_value_after_start.set_text("");
                                login_device_code_row_after_start.set_visible(false);
                                copy_device_code_button_after_start.set_sensitive(false);
                            }
                            if flow.method == "code" {
                                pending_runtime_oauth_flow_after_start
                                    .replace(Some((profile_id, flow)));
                                login_code_entry_after_start.set_text("");
                                login_code_row_after_start.set_visible(true);
                                login_waiting_spinner_after_start.stop();
                                login_waiting_row_after_start.set_visible(false);
                                if let Some(provider_name) = provider_name.as_ref() {
                                    operation_label_after_start.set_text(&format!(
                                        "Open the link above to connect {provider_name}, then paste the authorization code below."
                                    ));
                                } else {
                                    operation_label_after_start.set_text(
                                        "Open the link above, then paste the authorization code below.",
                                    );
                                }
                            } else {
                                login_code_row_after_start.set_visible(false);
                                login_waiting_label_after_start
                                    .set_text("Waiting for authorization...");
                                login_waiting_row_after_start.set_visible(true);
                                login_waiting_spinner_after_start.start();
                                if let Some(provider_name) = provider_name.as_ref() {
                                    if is_device_flow {
                                        operation_label_after_start.set_text(&format!(
                                            "Enter the code for {provider_name} in your browser. OpenCode will finish login automatically."
                                        ));
                                    } else {
                                        operation_label_after_start.set_text(&format!(
                                            "Open the link above to connect {provider_name}. Waiting for OpenCode to finish the OAuth callback..."
                                        ));
                                    }
                                } else {
                                    if is_device_flow {
                                        operation_label_after_start.set_text(
                                            "Enter the code in your browser. OpenCode will finish login automatically.",
                                        );
                                    } else {
                                        operation_label_after_start.set_text(
                                            "Open the link above. Waiting for OpenCode to finish the OAuth callback...",
                                        );
                                    }
                                }
                                complete_runtime_oauth_flow_after_start(profile_id, flow, None);
                            }
                            gtk::glib::ControlFlow::Break
                        }
                        Ok(Err(err)) => {
                            operation_label_after_start.set_visible(true);
                            operation_label_after_start.set_text(&format!("Login failed: {err}"));
                            gtk::glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
                    }
                });
                return;
            }
            let (tx, rx) = mpsc::channel::<Result<(String, String), String>>();
            let provider_for_thread = provider.clone();
            thread::spawn(move || {
                let result = match provider_for_thread {
                    Some((provider_id, _)) => {
                        client.account_login_start_oauth_for_provider(&provider_id)
                    }
                    None => client.account_login_start_chatgpt(),
                };
                let _ = tx.send(result);
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
            let provider_name = provider.as_ref().map(|(_, name)| name.clone());
            let reload_runtime_only_after_start = reload_runtime_only_providers.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
                Ok(Ok((_login_id, url))) => {
                    login_link_section_after_start.set_visible(true);
                    login_link_entry_after_start.set_text(&url);
                    copy_link_button_after_start.set_sensitive(true);
                    if let Some(provider_name) = provider_name.as_ref() {
                        operation_label_after_start.set_text(&format!(
                            "Open the link above in your browser to connect {provider_name}, then return here."
                        ));
                    } else {
                        operation_label_after_start.set_text(
                            "Open the link above in your browser, complete login, then return here.",
                        );
                    }
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
                    let reload_runtime_only_poll = reload_runtime_only_after_start.clone();
                    let manager_poll = manager_after_start.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(120), move || match poll_rx
                        .try_recv()
                    {
                        Ok(Ok(Some((account_type, email)))) => {
                            let account_text =
                                email.clone().unwrap_or_else(|| account_type.clone());
                            if runtime_only {
                                status_label_poll.set_text("Runtime status: running");
                                let connected = account_text
                                    .strip_prefix("OpenCode [")
                                    .and_then(|value| value.strip_suffix(']'))
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())
                                    .unwrap_or(account_text.as_str());
                                account_label_poll
                                    .set_text(&format!("Providers with auth: {connected}"));
                            } else {
                                status_label_poll.set_text("Profile status: running");
                                account_label_poll.set_text(&format!("Account: {}", account_text));
                            }
                            operation_label_poll.set_text("Login completed.");
                            let _ = db_poll.update_codex_profile_status(profile_id, "running");
                            let _ = crate::ui::components::runtime_auth_dialog::sync_runtime_account_to_db(
                                &db_poll,
                                profile_id,
                                Some(crate::codex_appserver::AccountInfo {
                                    account_type,
                                    email,
                                }),
                            );
                            if runtime_only {
                                crate::ui::components::runtime_auth_dialog::reload_opencode_runtime_after_auth(
                                    &manager_poll,
                                    profile_id,
                                );
                            }
                            let next_ids = sync_profile_dropdown_poll(Some(profile_id));
                            profile_ids_poll.replace(next_ids);
                            refresh_ui_poll();
                            if runtime_only {
                                reload_runtime_only_poll();
                            }
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
        })
    };

    {
        let db = db.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let operation_label = operation_label.clone();
        let start_oauth_login_for_provider = start_oauth_login_for_provider.clone();
        let all_provider_items = all_provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        login_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                return;
            };
            if is_system_home_profile(&db, profile_id) && !runtime_only {
                eprintln!("refusing profile login flow on system HOME profile");
                return;
            }
            if runtime_only {
                let Some(selected_id) = selected_provider_id.borrow().clone() else {
                    operation_label.set_visible(true);
                    operation_label.set_text("Select an OpenCode provider first.");
                    return;
                };
                let provider = all_provider_items
                    .borrow()
                    .iter()
                    .find(|provider| provider.provider_id == selected_id)
                    .cloned();
                let Some(provider) = provider else {
                    operation_label.set_visible(true);
                    operation_label.set_text("Selected OpenCode provider is unavailable.");
                    return;
                };
                if !provider.supports_oauth {
                    operation_label.set_visible(true);
                    operation_label.set_text("The selected provider does not support OAuth login.");
                    return;
                }
                let next_ids = sync_profile_dropdown(Some(profile_id));
                profile_ids.replace(next_ids);
                refresh_ui();
                start_oauth_login_for_provider(
                    profile_id,
                    Some((provider.provider_id, provider.provider_name)),
                );
                return;
            }
            start_oauth_login_for_provider(profile_id, None);
        });
    }

    {
        let dialog = dialog.clone();
        let db = db.clone();
        let manager = manager.clone();
        let profile_combo = profile_combo.clone();
        let profile_ids = profile_ids.clone();
        let sync_profile_dropdown = sync_profile_dropdown.clone();
        let refresh_ui = refresh_ui.clone();
        let operation_label = operation_label.clone();
        let api_key_button_for_signal = api_key_button.clone();
        let all_provider_items = all_provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        let reload_runtime_only_providers = reload_runtime_only_providers.clone();
        api_key_button_for_signal.clone().connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                return;
            };
            if is_system_home_profile(&db, profile_id) && !runtime_only {
                return;
            }
            if runtime_only {
                let Some(selected_id) = selected_provider_id.borrow().clone() else {
                    operation_label.set_visible(true);
                    operation_label.set_text("Select an OpenCode provider first.");
                    return;
                };
                let provider = all_provider_items
                    .borrow()
                    .iter()
                    .find(|provider| provider.provider_id == selected_id)
                    .cloned();
                let Some(provider) = provider else {
                    operation_label.set_visible(true);
                    operation_label.set_text("Selected OpenCode provider is unavailable.");
                    return;
                };
                if !provider.supports_api_key {
                    operation_label.set_visible(true);
                    operation_label.set_text("The selected provider does not support API-key login.");
                    return;
                }
                let sync_profile_dropdown_after_save = sync_profile_dropdown.clone();
                let profile_ids_after_save = profile_ids.clone();
                let refresh_ui_after_save = refresh_ui.clone();
                let operation_label_after_save = operation_label.clone();
                let reload_runtime_only_after_save = reload_runtime_only_providers.clone();
                crate::ui::components::runtime_auth_dialog::start_opencode_api_key_flow_for_provider(
                    Some(&dialog),
                    db.clone(),
                    manager.clone(),
                    profile_id,
                    provider.provider_id,
                    provider.provider_name,
                    api_key_button_for_signal.clone(),
                    operation_label.clone(),
                    "dim-label",
                    Rc::new(move |_account, provider_name| {
                        let next_ids = sync_profile_dropdown_after_save(Some(profile_id));
                        profile_ids_after_save.replace(next_ids);
                        refresh_ui_after_save();
                        operation_label_after_save.set_visible(true);
                        operation_label_after_save
                            .set_text(&format!("Saved API key for {provider_name}."));
                        reload_runtime_only_after_save();
                    }),
                );
                return;
            }
            let db = db.clone();
            let sync_profile_dropdown = sync_profile_dropdown.clone();
            let refresh_ui = refresh_ui.clone();
            let operation_label = operation_label.clone();
            let profile_ids = profile_ids.clone();
            crate::ui::components::runtime_auth_dialog::start_opencode_api_key_flow(
                Some(&dialog),
                db,
                manager.clone(),
                profile_id,
                api_key_button_for_signal.clone(),
                operation_label.clone(),
                "dim-label",
                Rc::new(move |_account, provider_name| {
                    let next_ids = sync_profile_dropdown(Some(profile_id));
                    profile_ids.replace(next_ids);
                    refresh_ui();
                    operation_label.set_visible(true);
                    operation_label.set_text(&format!("Saved API key for {provider_name}."));
                }),
            );
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
        let reload_runtime_only_providers = reload_runtime_only_providers.clone();
        let all_provider_items = all_provider_items.clone();
        let selected_provider_id = selected_provider_id.clone();
        logout_button.connect_clicked(move |_| {
            let profile_id = {
                let ids = profile_ids.borrow();
                selected_profile_id(&profile_combo, &ids)
            };
            let Some(profile_id) = profile_id else {
                return;
            };
            if is_system_home_profile(&db, profile_id) && !runtime_only {
                eprintln!("refusing logout on system HOME profile");
                return;
            }
            let Ok(client) = manager.ensure_started(profile_id) else {
                return;
            };
            if !client.capabilities().supports_logout {
                operation_label.set_visible(true);
                operation_label.set_text("Logout is not supported for this runtime profile.");
                refresh_ui();
                return;
            }
            if runtime_only {
                let Some(selected_id) = selected_provider_id.borrow().clone() else {
                    operation_label.set_visible(true);
                    operation_label.set_text("Select an OpenCode provider first.");
                    return;
                };
                let provider = all_provider_items
                    .borrow()
                    .iter()
                    .find(|provider| provider.provider_id == selected_id)
                    .cloned();
                let Some(provider) = provider else {
                    operation_label.set_visible(true);
                    operation_label.set_text("Selected OpenCode provider is unavailable.");
                    return;
                };
                if !provider.connected && !provider.has_saved_auth {
                    operation_label.set_visible(true);
                    operation_label.set_text("Selected provider has no saved auth to remove.");
                    return;
                }
                let _ = client.account_logout_provider(&provider.provider_id);
                let account = client.account_read(true).ok().flatten();
                let _ = crate::ui::components::runtime_auth_dialog::sync_runtime_account_to_db(
                    &db, profile_id, account,
                );
                crate::ui::components::runtime_auth_dialog::reload_opencode_runtime_after_auth(
                    &manager, profile_id,
                );
                operation_label.set_visible(true);
                operation_label.set_text(&format!(
                    "Removed {} from OpenCode.",
                    provider.provider_name
                ));
            } else {
                let _ = client.account_logout();
                let _ =
                    crate::ui::components::runtime_auth_dialog::clear_runtime_account_for_profile(
                        &db, profile_id,
                    );
                operation_label.set_visible(true);
                operation_label.set_text("Logged out from this profile.");
            }
            let next_ids = sync_profile_dropdown(Some(profile_id));
            profile_ids.replace(next_ids);
            refresh_ui();
            if runtime_only {
                reload_runtime_only_providers();
            }
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
                .title("Remove Runtime Profile")
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
    settings_dialog::show(parent, db, manager, settings_dialog::SettingsPage::Codex);
}
