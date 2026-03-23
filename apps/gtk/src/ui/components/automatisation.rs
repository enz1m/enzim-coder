use adw::prelude::*;
use enzimcoder::data::unix_now;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use crate::services::app::CodexProfileManager;
use crate::services::app::automations::{
    AutomationDefinition, AutomationRunRecord, build_prompt, delete_automation,
    list_due_automations, load_automations, load_runs, mark_automation_scheduled, new_run_id,
    normalize_definition, push_run, update_run_status, upsert_automation,
};
use crate::services::app::chat::AppDb;
use crate::ui::components::thread_list;
use crate::ui::{content, widget_tree};

fn access_mode_options() -> [(&'static str, &'static str); 3] {
    [
        ("workspaceWrite", "Workspace Write"),
        ("readOnly", "Read Only"),
        ("dangerFullAccess", "Danger Full Access"),
    ]
}

fn find_workspace_index(items: &[String], target: &str) -> Option<u32> {
    items
        .iter()
        .position(|item| item == target)
        .map(|index| index as u32)
}

fn find_profile_index(items: &[(i64, String)], profile_id: i64) -> Option<u32> {
    items
        .iter()
        .position(|(id, _)| *id == profile_id)
        .map(|index| index as u32)
}

fn load_workspace_options(db: &AppDb) -> Vec<String> {
    db.list_workspaces_with_threads()
        .unwrap_or_default()
        .into_iter()
        .map(|workspace| workspace.workspace.path)
        .collect()
}

fn load_profile_options(db: &AppDb) -> Vec<(i64, String)> {
    db.list_codex_profiles()
        .unwrap_or_default()
        .into_iter()
        .map(|profile| {
            (
                profile.id,
                format!(
                    "{} ({})",
                    profile.name,
                    crate::services::app::runtime::backend_display_name(&profile.backend_kind)
                ),
            )
        })
        .collect()
}

fn activate_local_thread(
    sidebar: &adw::ToolbarView,
    db: &AppDb,
    active_thread_id: &Rc<RefCell<Option<String>>>,
    active_workspace_path: &Rc<RefCell<Option<String>>>,
    selected_page: &Rc<RefCell<String>>,
    local_thread_id: i64,
) -> Result<(), String> {
    let Some(thread) = db
        .get_thread_record(local_thread_id)
        .map_err(|err| err.to_string())?
    else {
        return Err("Thread no longer exists.".to_string());
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

    let linked_thread_id = thread.remote_thread_id_owned();
    if let Some(thread_id) = linked_thread_id {
        active_thread_id.replace(Some(thread_id));
        let _ = db.set_setting("pending_profile_thread_id", "");
    } else {
        active_thread_id.replace(None);
        let _ = db.set_setting("pending_profile_thread_id", &local_thread_id.to_string());
    }
    selected_page.replace(content::MAIN_PAGE_WORKSPACES.to_string());

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

    Ok(())
}

fn run_single_automation(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    _sidebar: adw::ToolbarView,
    automation: AutomationDefinition,
) -> Result<AutomationRunRecord, String> {
    let automation =
        mark_automation_scheduled(db.as_ref(), &automation.id, None)?.unwrap_or(automation);
    let workspace = db
        .list_workspaces_with_threads()
        .map_err(|err| err.to_string())?;
    let workspace = workspace
        .into_iter()
        .find(|workspace| workspace.workspace.path == automation.workspace_path)
        .map(|workspace| workspace.workspace)
        .ok_or_else(|| "Selected workspace no longer exists.".to_string())?;
    let profile = db
        .get_codex_profile(automation.profile_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "Selected profile no longer exists.".to_string())?;
    let client = manager
        .ensure_started(profile.id)
        .map_err(|err| format!("Unable to start runtime for automation: {err}"))?;

    let thread_title = format!("Automation: {}", automation.name);
    let local_thread = db
        .create_thread_with_remote_identity(
            workspace.id,
            profile.id,
            None,
            &thread_title,
            None,
            profile.last_account_type.as_deref(),
            profile.last_email.as_deref(),
        )
        .map_err(|err| err.to_string())?;

    let _ = thread_list::append_thread_to_workspace_from_root_passive(
        &automation.workspace_path,
        local_thread.clone(),
    );

    let run = AutomationRunRecord {
        id: new_run_id(),
        automation_id: automation.id.clone(),
        automation_name: automation.name.clone(),
        workspace_path: automation.workspace_path.clone(),
        local_thread_id: local_thread.id,
        remote_thread_id: None,
        started_at: unix_now(),
        status: "queued".to_string(),
        summary: "Preparing background run...".to_string(),
        error: None,
    };
    push_run(db.as_ref(), run.clone())?;

    let prompt = build_prompt(&automation.prompt, &automation.skill_hints);
    let model_id = automation.model_id.clone();
    let effort = automation.effort.clone();
    let access_mode = automation.access_mode.clone();
    let workspace_path = automation.workspace_path.clone();
    let run_id = run.id.clone();
    let local_thread_id = local_thread.id;
    let remote_account_type = profile.last_account_type.clone();
    let remote_account_email = profile.last_email.clone();
    thread::spawn(move || {
        let db = match AppDb::open_detached() {
            Ok(db) => db,
            Err(err) => {
                eprintln!("failed to open detached DB for automation run: {err}");
                return;
            }
        };
        let sandbox_policy =
            crate::ui::components::chat::runtime_controls::sandbox_policy_for(&access_mode);
        let remote_thread_result = client.thread_start(
            Some(&workspace_path),
            model_id.as_deref(),
            sandbox_policy.clone(),
        );
        match remote_thread_result {
            Ok(remote_thread_id) => {
                let _ = db.set_thread_remote_id_with_account(
                    local_thread_id,
                    &remote_thread_id,
                    remote_account_type.as_deref(),
                    remote_account_email.as_deref(),
                );
                let _ = update_run_status(
                    &db,
                    &run_id,
                    "running",
                    Some("Runtime thread started. Dispatching prompt...".to_string()),
                    None,
                    Some(remote_thread_id.clone()),
                );
                let turn_result = client.turn_start(
                    &remote_thread_id,
                    &prompt,
                    &[],
                    &[],
                    model_id.as_deref(),
                    effort.as_deref(),
                    sandbox_policy,
                    None,
                    None,
                    Some(&workspace_path),
                );
                match turn_result {
                    Ok(_) => {
                        let _ = update_run_status(
                            &db,
                            &run_id,
                            "started",
                            Some(
                                "Background run started. Open the thread to review progress."
                                    .to_string(),
                            ),
                            None,
                            Some(remote_thread_id),
                        );
                    }
                    Err(err) => {
                        let _ = update_run_status(
                            &db,
                            &run_id,
                            "failed",
                            Some("Prompt dispatch failed.".to_string()),
                            Some(err),
                            Some(remote_thread_id),
                        );
                    }
                }
            }
            Err(err) => {
                let _ = update_run_status(
                    &db,
                    &run_id,
                    "failed",
                    Some("Runtime thread could not be created.".to_string()),
                    Some(err),
                    None,
                );
            }
        }
    });

    Ok(run)
}

pub(crate) fn trigger_automation_run(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    sidebar: adw::ToolbarView,
    automation_id: &str,
) -> Result<AutomationRunRecord, String> {
    let automation = load_automations(db.as_ref())
        .into_iter()
        .find(|item| item.id == automation_id)
        .ok_or_else(|| "Automation not found.".to_string())?;
    run_single_automation(db, manager, sidebar, automation)
}

pub(crate) fn run_due_automations(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    sidebar: adw::ToolbarView,
) {
    let now = unix_now();
    for automation in list_due_automations(db.as_ref(), now) {
        if let Err(err) = run_single_automation(
            db.clone(),
            manager.clone(),
            sidebar.clone(),
            automation.clone(),
        ) {
            let mut failed = automation;
            failed.last_error = Some(err);
            failed.updated_at = now;
            let _ = upsert_automation(db.as_ref(), failed);
        }
    }
}

pub(crate) fn build_page(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    sidebar: adw::ToolbarView,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    selected_page: Rc<RefCell<String>>,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("automatisation-page");

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    root.append(&scroll);

    let shell = gtk::Box::new(gtk::Orientation::Vertical, 18);
    shell.add_css_class("automatisation-shell");
    shell.set_margin_start(20);
    shell.set_margin_end(20);
    shell.set_margin_top(20);
    shell.set_margin_bottom(24);
    scroll.set_child(Some(&shell));

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.set_halign(gtk::Align::Fill);
    header.set_hexpand(true);
    let header_copy = gtk::Box::new(gtk::Orientation::Vertical, 4);
    header_copy.set_hexpand(true);
    let title = gtk::Label::new(Some("Automatisation"));
    title.add_css_class("automatisation-title");
    title.set_xalign(0.0);
    let subtitle = gtk::Label::new(Some(
        "Create scheduled background coding runs. Each run opens a reviewable thread inside the target workspace.",
    ));
    subtitle.add_css_class("dim-label");
    subtitle.set_wrap(true);
    subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    subtitle.set_xalign(0.0);
    header_copy.append(&title);
    header_copy.append(&subtitle);
    let back_button = gtk::Button::with_label("Back to Workspaces");
    back_button.add_css_class("sidebar-action-button");
    {
        let selected_page = selected_page.clone();
        back_button.connect_clicked(move |_| {
            selected_page.replace(content::MAIN_PAGE_WORKSPACES.to_string());
        });
    }
    header.append(&header_copy);
    header.append(&back_button);
    shell.append(&header);

    let status_label = gtk::Label::new(None);
    status_label.add_css_class("dim-label");
    status_label.set_wrap(true);
    status_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_label.set_xalign(0.0);
    status_label.set_visible(false);
    shell.append(&status_label);

    let editor = gtk::Box::new(gtk::Orientation::Vertical, 12);
    editor.add_css_class("profile-settings-section");
    editor.add_css_class("automatisation-card");
    shell.append(&editor);

    let editor_title = gtk::Label::new(Some("Automation Editor"));
    editor_title.add_css_class("profile-section-title");
    editor_title.set_xalign(0.0);
    editor.append(&editor_title);

    let selected_automation_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let name_label = gtk::Label::new(Some("Name"));
    name_label.set_xalign(0.0);
    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some("Nightly refactor sweep"));
    editor.append(&name_label);
    editor.append(&name_entry);

    let workspace_label = gtk::Label::new(Some("Workspace"));
    workspace_label.set_xalign(0.0);
    let workspace_model = gtk::StringList::new(&[]);
    let workspace_dropdown =
        gtk::DropDown::new(Some(workspace_model.clone()), None::<&gtk::Expression>);
    editor.append(&workspace_label);
    editor.append(&workspace_dropdown);
    let workspace_items: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let profile_label = gtk::Label::new(Some("Profile"));
    profile_label.set_xalign(0.0);
    let profile_model = gtk::StringList::new(&[]);
    let profile_dropdown =
        gtk::DropDown::new(Some(profile_model.clone()), None::<&gtk::Expression>);
    editor.append(&profile_label);
    editor.append(&profile_dropdown);
    let profile_items: Rc<RefCell<Vec<(i64, String)>>> = Rc::new(RefCell::new(Vec::new()));

    let cadence_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    cadence_row.set_hexpand(true);
    let interval_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    interval_box.set_hexpand(true);
    let interval_label = gtk::Label::new(Some("Schedule"));
    interval_label.set_xalign(0.0);
    let interval_help = gtk::Label::new(Some("Every N minutes. Use 0 for manual-only."));
    interval_help.add_css_class("dim-label");
    interval_help.set_xalign(0.0);
    let interval_spin = gtk::SpinButton::with_range(0.0, 10_080.0, 5.0);
    interval_spin.set_value(60.0);
    interval_box.append(&interval_label);
    interval_box.append(&interval_spin);
    interval_box.append(&interval_help);
    let enabled_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    enabled_box.set_halign(gtk::Align::Start);
    let enabled_label = gtk::Label::new(Some("Enabled"));
    enabled_label.set_xalign(0.0);
    let enabled_switch = gtk::Switch::new();
    enabled_switch.set_active(true);
    enabled_box.append(&enabled_label);
    enabled_box.append(&enabled_switch);
    cadence_row.append(&interval_box);
    cadence_row.append(&enabled_box);
    editor.append(&cadence_row);

    let access_label = gtk::Label::new(Some("Execution Access"));
    access_label.set_xalign(0.0);
    let access_model = gtk::StringList::new(
        &access_mode_options()
            .iter()
            .map(|(_, label)| *label)
            .collect::<Vec<_>>(),
    );
    let access_dropdown = gtk::DropDown::new(Some(access_model), None::<&gtk::Expression>);
    access_dropdown.set_selected(0);
    editor.append(&access_label);
    editor.append(&access_dropdown);

    let model_label = gtk::Label::new(Some("Model Override"));
    model_label.set_xalign(0.0);
    let model_entry = gtk::Entry::new();
    model_entry.set_placeholder_text(Some("Leave blank to use the profile default"));
    editor.append(&model_label);
    editor.append(&model_entry);

    let effort_label = gtk::Label::new(Some("Effort / Variant Override"));
    effort_label.set_xalign(0.0);
    let effort_entry = gtk::Entry::new();
    effort_entry.set_placeholder_text(Some("Optional"));
    editor.append(&effort_label);
    editor.append(&effort_entry);

    let prompt_label = gtk::Label::new(Some("Instructions"));
    prompt_label.set_xalign(0.0);
    let prompt_view = gtk::TextView::new();
    prompt_view.set_wrap_mode(gtk::WrapMode::WordChar);
    prompt_view.set_vexpand(true);
    prompt_view.set_top_margin(8);
    prompt_view.set_bottom_margin(8);
    prompt_view.set_left_margin(8);
    prompt_view.set_right_margin(8);
    let prompt_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(150)
        .child(&prompt_view)
        .build();
    prompt_scroll.add_css_class("automatisation-textbox");
    editor.append(&prompt_label);
    editor.append(&prompt_scroll);

    let skills_label = gtk::Label::new(Some("Skill Hints"));
    skills_label.set_xalign(0.0);
    let skills_entry = gtk::Entry::new();
    skills_entry.set_placeholder_text(Some(
        "Optional. Passed into the prompt as preferred skills or references.",
    ));
    editor.append(&skills_label);
    editor.append(&skills_entry);

    let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let save_button = gtk::Button::with_label("Save Automation");
    save_button.add_css_class("suggested-action");
    let reset_button = gtk::Button::with_label("Reset");
    let run_button = gtk::Button::with_label("Run Now");
    action_row.append(&save_button);
    action_row.append(&reset_button);
    action_row.append(&run_button);
    editor.append(&action_row);

    let list_section = gtk::Box::new(gtk::Orientation::Vertical, 10);
    shell.append(&list_section);
    let list_title = gtk::Label::new(Some("Saved Automations"));
    list_title.add_css_class("profile-section-title");
    list_title.set_xalign(0.0);
    let automation_list = gtk::Box::new(gtk::Orientation::Vertical, 10);
    list_section.append(&list_title);
    list_section.append(&automation_list);

    let runs_section = gtk::Box::new(gtk::Orientation::Vertical, 10);
    shell.append(&runs_section);
    let runs_title = gtk::Label::new(Some("Review Queue"));
    runs_title.add_css_class("profile-section-title");
    runs_title.set_xalign(0.0);
    let runs_list = gtk::Box::new(gtk::Orientation::Vertical, 10);
    runs_section.append(&runs_title);
    runs_section.append(&runs_list);

    let reload_workspace_dropdown: Rc<dyn Fn(Option<&str>)> = {
        let db = db.clone();
        let workspace_model = workspace_model.clone();
        let workspace_dropdown = workspace_dropdown.clone();
        let workspace_items = workspace_items.clone();
        Rc::new(move |selected: Option<&str>| {
            while workspace_model.n_items() > 0 {
                workspace_model.remove(0);
            }
            let items = load_workspace_options(db.as_ref());
            for item in &items {
                workspace_model.append(item);
            }
            let selected_index = selected
                .and_then(|target| find_workspace_index(&items, target))
                .or_else(|| (!items.is_empty()).then_some(0))
                .unwrap_or(gtk::INVALID_LIST_POSITION);
            workspace_dropdown.set_selected(selected_index);
            workspace_items.replace(items);
        })
    };

    let reload_profile_dropdown: Rc<dyn Fn(Option<i64>)> = {
        let db = db.clone();
        let profile_model = profile_model.clone();
        let profile_dropdown = profile_dropdown.clone();
        let profile_items = profile_items.clone();
        Rc::new(move |selected: Option<i64>| {
            while profile_model.n_items() > 0 {
                profile_model.remove(0);
            }
            let items = load_profile_options(db.as_ref());
            for (_, label) in &items {
                profile_model.append(label);
            }
            let selected_index = selected
                .and_then(|target| find_profile_index(&items, target))
                .or_else(|| (!items.is_empty()).then_some(0))
                .unwrap_or(gtk::INVALID_LIST_POSITION);
            profile_dropdown.set_selected(selected_index);
            profile_items.replace(items);
        })
    };

    let clear_form: Rc<dyn Fn()> = {
        let selected_automation_id = selected_automation_id.clone();
        let name_entry = name_entry.clone();
        let prompt_view = prompt_view.clone();
        let skills_entry = skills_entry.clone();
        let interval_spin = interval_spin.clone();
        let enabled_switch = enabled_switch.clone();
        let access_dropdown = access_dropdown.clone();
        let model_entry = model_entry.clone();
        let effort_entry = effort_entry.clone();
        let reload_workspace_dropdown = reload_workspace_dropdown.clone();
        let reload_profile_dropdown = reload_profile_dropdown.clone();
        Rc::new(move || {
            selected_automation_id.replace(None);
            name_entry.set_text("");
            prompt_view.buffer().set_text("");
            skills_entry.set_text("");
            interval_spin.set_value(60.0);
            enabled_switch.set_active(true);
            access_dropdown.set_selected(0);
            model_entry.set_text("");
            effort_entry.set_text("");
            reload_workspace_dropdown(None);
            reload_profile_dropdown(None);
        })
    };

    let load_form_from_automation: Rc<dyn Fn(&AutomationDefinition)> = {
        let selected_automation_id = selected_automation_id.clone();
        let name_entry = name_entry.clone();
        let prompt_view = prompt_view.clone();
        let skills_entry = skills_entry.clone();
        let interval_spin = interval_spin.clone();
        let enabled_switch = enabled_switch.clone();
        let access_dropdown = access_dropdown.clone();
        let model_entry = model_entry.clone();
        let effort_entry = effort_entry.clone();
        let reload_workspace_dropdown = reload_workspace_dropdown.clone();
        let reload_profile_dropdown = reload_profile_dropdown.clone();
        Rc::new(move |automation: &AutomationDefinition| {
            selected_automation_id.replace(Some(automation.id.clone()));
            name_entry.set_text(&automation.name);
            prompt_view.buffer().set_text(&automation.prompt);
            skills_entry.set_text(&automation.skill_hints);
            interval_spin.set_value(automation.interval_minutes as f64);
            enabled_switch.set_active(automation.enabled);
            let selected_access = access_mode_options()
                .iter()
                .position(|(value, _)| *value == automation.access_mode)
                .unwrap_or(0) as u32;
            access_dropdown.set_selected(selected_access);
            model_entry.set_text(automation.model_id.as_deref().unwrap_or(""));
            effort_entry.set_text(automation.effort.as_deref().unwrap_or(""));
            reload_workspace_dropdown(Some(&automation.workspace_path));
            reload_profile_dropdown(Some(automation.profile_id));
        })
    };

    let refresh_lists_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let refresh_lists: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let sidebar = sidebar.clone();
        let selected_page = selected_page.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let automation_list = automation_list.clone();
        let runs_list = runs_list.clone();
        let load_form_from_automation = load_form_from_automation.clone();
        let status_label = status_label.clone();
        let refresh_lists_handle = refresh_lists_handle.clone();
        Rc::new(move || {
            while let Some(child) = automation_list.first_child() {
                automation_list.remove(&child);
            }
            while let Some(child) = runs_list.first_child() {
                runs_list.remove(&child);
            }

            let automations = load_automations(db.as_ref());
            if automations.is_empty() {
                let empty = gtk::Label::new(Some("No automations configured yet."));
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                automation_list.append(&empty);
            } else {
                for automation in automations {
                    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
                    card.add_css_class("profile-settings-section");
                    card.add_css_class("automatisation-list-card");
                    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    title_box.set_hexpand(true);
                    let name = gtk::Label::new(Some(&automation.name));
                    name.add_css_class("profile-section-title");
                    name.set_xalign(0.0);
                    let meta = gtk::Label::new(Some(&format!(
                        "{} • every {} min • {}",
                        Path::new(&automation.workspace_path)
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or(&automation.workspace_path),
                        automation.interval_minutes,
                        if automation.enabled {
                            "enabled"
                        } else {
                            "paused"
                        }
                    )));
                    meta.add_css_class("dim-label");
                    meta.set_xalign(0.0);
                    title_box.append(&name);
                    title_box.append(&meta);
                    header.append(&title_box);

                    let edit = gtk::Button::with_label("Edit");
                    let run = gtk::Button::with_label("Run");
                    let delete = gtk::Button::with_label("Delete");
                    header.append(&edit);
                    header.append(&run);
                    header.append(&delete);
                    card.append(&header);

                    let prompt = gtk::Label::new(Some(&automation.prompt));
                    prompt.set_wrap(true);
                    prompt.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    prompt.set_xalign(0.0);
                    prompt.add_css_class("dim-label");
                    card.append(&prompt);

                    if let Some(error) = automation.last_error.as_deref() {
                        let error_label = gtk::Label::new(Some(&format!("Last error: {error}")));
                        error_label.set_wrap(true);
                        error_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                        error_label.set_xalign(0.0);
                        error_label.add_css_class("dim-label");
                        card.append(&error_label);
                    }

                    {
                        let load_form_from_automation = load_form_from_automation.clone();
                        let automation = automation.clone();
                        edit.connect_clicked(move |_| load_form_from_automation(&automation));
                    }
                    {
                        let db = db.clone();
                        let manager = manager.clone();
                        let sidebar = sidebar.clone();
                        let status_label = status_label.clone();
                        let automation_id = automation.id.clone();
                        let refresh_lists_handle = refresh_lists_handle.clone();
                        run.connect_clicked(move |_| {
                            status_label.set_visible(true);
                            match trigger_automation_run(
                                db.clone(),
                                manager.clone(),
                                sidebar.clone(),
                                &automation_id,
                            ) {
                                Ok(run) => status_label.set_text(&format!(
                                    "Started automation run in thread #{}.",
                                    run.local_thread_id
                                )),
                                Err(err) => status_label
                                    .set_text(&format!("Unable to start automation: {err}")),
                            }
                            if let Some(refresh) = refresh_lists_handle.borrow().clone() {
                                refresh();
                            }
                        });
                    }
                    {
                        let db = db.clone();
                        let status_label = status_label.clone();
                        let automation_id = automation.id.clone();
                        let refresh_lists_handle = refresh_lists_handle.clone();
                        delete.connect_clicked(move |_| {
                            match delete_automation(db.as_ref(), &automation_id) {
                                Ok(()) => {
                                    status_label.set_visible(true);
                                    status_label.set_text("Automation deleted.");
                                    if let Some(refresh) = refresh_lists_handle.borrow().clone() {
                                        refresh();
                                    }
                                }
                                Err(err) => {
                                    status_label.set_visible(true);
                                    status_label
                                        .set_text(&format!("Unable to delete automation: {err}"));
                                }
                            }
                        });
                    }

                    automation_list.append(&card);
                }
            }

            let runs = load_runs(db.as_ref());
            if runs.is_empty() {
                let empty = gtk::Label::new(Some("No automation runs yet."));
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                runs_list.append(&empty);
            } else {
                for run in runs.into_iter().take(24) {
                    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
                    card.add_css_class("profile-settings-section");
                    card.add_css_class("automatisation-list-card");
                    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    title_box.set_hexpand(true);
                    let title = gtk::Label::new(Some(&run.automation_name));
                    title.add_css_class("profile-section-title");
                    title.set_xalign(0.0);
                    let meta = gtk::Label::new(Some(&format!(
                        "#{} • {} • {}",
                        run.local_thread_id, run.status, run.summary
                    )));
                    meta.add_css_class("dim-label");
                    meta.set_wrap(true);
                    meta.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    meta.set_xalign(0.0);
                    title_box.append(&title);
                    title_box.append(&meta);
                    header.append(&title_box);
                    let open = gtk::Button::with_label("Open Thread");
                    header.append(&open);
                    card.append(&header);

                    if let Some(error) = run.error.as_deref() {
                        let error = gtk::Label::new(Some(error));
                        error.set_wrap(true);
                        error.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                        error.set_xalign(0.0);
                        error.add_css_class("dim-label");
                        card.append(&error);
                    }

                    {
                        let sidebar = sidebar.clone();
                        let db = db.clone();
                        let active_thread_id = active_thread_id.clone();
                        let active_workspace_path = active_workspace_path.clone();
                        let selected_page = selected_page.clone();
                        let status_label = status_label.clone();
                        let local_thread_id = run.local_thread_id;
                        open.connect_clicked(move |_| {
                            status_label.set_visible(true);
                            match activate_local_thread(
                                &sidebar,
                                db.as_ref(),
                                &active_thread_id,
                                &active_workspace_path,
                                &selected_page,
                                local_thread_id,
                            ) {
                                Ok(()) => status_label.set_text("Opened automation thread."),
                                Err(err) => status_label
                                    .set_text(&format!("Unable to open automation thread: {err}")),
                            }
                        });
                    }

                    runs_list.append(&card);
                }
            }
        })
    };
    refresh_lists_handle.replace(Some(refresh_lists.clone()));

    reload_workspace_dropdown(None);
    reload_profile_dropdown(None);
    clear_form();

    {
        let clear_form = clear_form.clone();
        reset_button.connect_clicked(move |_| clear_form());
    }

    {
        let db = db.clone();
        let status_label = status_label.clone();
        let refresh_lists = refresh_lists.clone();
        let selected_automation_id = selected_automation_id.clone();
        let workspace_items = workspace_items.clone();
        let profile_items = profile_items.clone();
        let name_entry = name_entry.clone();
        let prompt_view = prompt_view.clone();
        let skills_entry = skills_entry.clone();
        let interval_spin = interval_spin.clone();
        let enabled_switch = enabled_switch.clone();
        let access_dropdown = access_dropdown.clone();
        let model_entry = model_entry.clone();
        let effort_entry = effort_entry.clone();
        let clear_form = clear_form.clone();
        save_button.connect_clicked(move |_| {
            let selected_id = selected_automation_id.borrow().clone();
            let existing = selected_id.as_deref().and_then(|automation_id| {
                load_automations(db.as_ref())
                    .into_iter()
                    .find(|item| item.id == automation_id)
            });
            let name = name_entry.text().trim().to_string();
            let workspace = workspace_items
                .borrow()
                .get(workspace_dropdown.selected() as usize)
                .cloned()
                .unwrap_or_default();
            let profile = profile_items
                .borrow()
                .get(profile_dropdown.selected() as usize)
                .map(|(id, _)| *id)
                .unwrap_or_default();
            let prompt_buffer = prompt_view.buffer();
            let prompt = prompt_buffer
                .text(
                    &prompt_buffer.start_iter(),
                    &prompt_buffer.end_iter(),
                    false,
                )
                .to_string();
            if name.is_empty()
                || workspace.trim().is_empty()
                || profile <= 0
                || prompt.trim().is_empty()
            {
                status_label.set_visible(true);
                status_label.set_text("Name, workspace, profile, and instructions are required.");
                return;
            }
            let access_index = access_dropdown.selected() as usize;
            let access_mode = access_mode_options()
                .get(access_index)
                .map(|(value, _)| (*value).to_string())
                .unwrap_or_else(|| "workspaceWrite".to_string());
            let automation = normalize_definition(AutomationDefinition {
                id: selected_id.unwrap_or_default(),
                name,
                workspace_path: workspace,
                profile_id: profile,
                prompt,
                skill_hints: skills_entry.text().to_string(),
                interval_minutes: interval_spin.value() as i64,
                enabled: enabled_switch.is_active(),
                access_mode,
                model_id: Some(model_entry.text().to_string()),
                effort: Some(effort_entry.text().to_string()),
                created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(0),
                updated_at: 0,
                last_run_at: existing.as_ref().and_then(|item| item.last_run_at),
                next_run_at: existing.as_ref().and_then(|item| item.next_run_at),
                last_error: existing.as_ref().and_then(|item| item.last_error.clone()),
            });
            match upsert_automation(db.as_ref(), automation) {
                Ok(()) => {
                    status_label.set_visible(true);
                    status_label.set_text("Automation saved.");
                    clear_form();
                    refresh_lists();
                }
                Err(err) => {
                    status_label.set_visible(true);
                    status_label.set_text(&format!("Unable to save automation: {err}"));
                }
            }
        });
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let sidebar = sidebar.clone();
        let status_label = status_label.clone();
        let selected_automation_id = selected_automation_id.clone();
        run_button.connect_clicked(move |_| {
            let Some(automation_id) = selected_automation_id.borrow().clone() else {
                status_label.set_visible(true);
                status_label.set_text("Select or save an automation before running it.");
                return;
            };
            status_label.set_visible(true);
            match trigger_automation_run(
                db.clone(),
                manager.clone(),
                sidebar.clone(),
                &automation_id,
            ) {
                Ok(run) => status_label.set_text(&format!(
                    "Automation dispatched to thread #{}.",
                    run.local_thread_id
                )),
                Err(err) => status_label.set_text(&format!("Unable to run automation: {err}")),
            }
        });
    }

    refresh_lists();
    {
        let refresh_lists = refresh_lists.clone();
        let root = root.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(1200), move || {
            if root.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            refresh_lists();
            gtk::glib::ControlFlow::Continue
        });
    }

    root
}
