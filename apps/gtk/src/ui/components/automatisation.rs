use adw::prelude::*;
use enzimcoder::data::{format_relative_age, unix_now};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use crate::services::app::CodexProfileManager;
use crate::services::app::automations::{
    AutomationDefinition, AutomationRunEvent, AutomationRunRecord, build_prompt, delete_automation,
    list_due_automations, load_automations, load_runs, mark_automation_scheduled, new_run_id,
    normalize_definition, push_run, set_automation_error, update_run_status, upsert_automation,
};
use crate::services::app::chat::AppDb;
use crate::ui::components::thread_list;
use crate::ui::{content, widget_tree};

const DETAIL_INFO: &str = "info";
const DETAIL_HISTORY: &str = "history";
const DETAIL_SETTINGS: &str = "settings";
const DETAIL_TIMELINE: &str = "timeline";

fn access_mode_options() -> [(&'static str, &'static str); 3] {
    [
        ("workspaceWrite", "Workspace Write"),
        ("readOnly", "Read Only"),
        ("dangerFullAccess", "Danger Full Access"),
    ]
}

fn schedule_mode_options() -> [(&'static str, &'static str); 3] {
    [
        ("manual", "Manual"),
        ("interval", "Every interval"),
        ("weekly", "Specific days & times"),
    ]
}

fn interval_unit_options() -> [(&'static str, &'static str); 3] {
    [("minute", "Minutes"), ("hour", "Hours"), ("day", "Days")]
}

fn weekday_options() -> [(&'static str, &'static str); 7] {
    [
        ("mon", "Mon"),
        ("tue", "Tue"),
        ("wed", "Wed"),
        ("thu", "Thu"),
        ("fri", "Fri"),
        ("sat", "Sat"),
        ("sun", "Sun"),
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

fn find_option_index(items: &[(&str, &str)], target: &str) -> u32 {
    items
        .iter()
        .position(|(value, _)| *value == target)
        .map(|index| index as u32)
        .unwrap_or(0)
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

fn automation_schedule_label(automation: &AutomationDefinition) -> String {
    match automation.schedule_mode.as_str() {
        "manual" => "Manual".to_string(),
        "weekly" => {
            let days = automation
                .weekly_days
                .iter()
                .map(|day| {
                    weekday_options()
                        .into_iter()
                        .find(|(value, _)| value == day)
                        .map(|(_, label)| label)
                        .unwrap_or(day.as_str())
                })
                .collect::<Vec<_>>();
            let times = if automation.weekly_times.is_empty() {
                "Times pending".to_string()
            } else {
                automation.weekly_times.join(", ")
            };
            if days.is_empty() {
                format!("Weekly at {times}")
            } else {
                format!("{} at {times}", days.join(", "))
            }
        }
        _ => {
            let unit_label = interval_unit_options()
                .into_iter()
                .find(|(value, _)| *value == automation.interval_unit)
                .map(|(_, label)| label)
                .unwrap_or("Hours");
            format!("Every {} {}", automation.interval_value, unit_label)
        }
    }
}

fn automation_workspace_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_string()
}

fn fallback_timeline_for_run(run: &AutomationRunRecord) -> Vec<AutomationRunEvent> {
    if !run.timeline.is_empty() {
        return run.timeline.clone();
    }
    vec![AutomationRunEvent {
        at: run.started_at,
        status: run.status.clone(),
        title: run.summary.clone(),
        detail: run.error.clone().unwrap_or_else(|| run.summary.clone()),
    }]
}

fn selection_summary(names: &[String], empty_label: &str) -> String {
    match names.len() {
        0 => empty_label.to_string(),
        1 => names[0].clone(),
        2 => format!("{}, {}", names[0], names[1]),
        _ => format!("{}, {} +{}", names[0], names[1], names.len() - 2),
    }
}

fn apply_prompt_editor_theme(
    scroll: &gtk::ScrolledWindow,
    view: &gtk::TextView,
    scroll_name: &str,
    view_name: &str,
) {
    scroll.set_widget_name(scroll_name);
    view.set_widget_name(view_name);

    let provider = gtk::CssProvider::new();
    let css = format!(
        r#"
#{scroll_name},
#{scroll_name} > viewport,
#{scroll_name} > viewport.view,
#{scroll_name} > viewport > textview,
#{scroll_name} > viewport > textview.view,
textview#{view_name},
textview#{view_name}.view,
textview#{view_name} text {{
  border-radius: 12px;
  border: 1px solid alpha(@window_fg_color, 0.09);
  background: alpha(@window_fg_color, 0.06);
  background-color: alpha(@window_fg_color, 0.06);
  background-image: none;
  color: @window_fg_color;
  box-shadow: none;
  outline: none;
}}

scrolledwindow#{scroll_name} > scrollbar.vertical,
scrolledwindow#{scroll_name} > scrollbar.vertical > range,
scrolledwindow#{scroll_name} > scrollbar.vertical > range > trough,
scrolledwindow#{scroll_name} > scrollbar.vertical > range > trough > slider {{
  min-width: 0;
  min-height: 0;
  margin: 0;
  padding: 0;
  border: none;
  box-shadow: none;
  background: transparent;
  background-color: transparent;
  background-image: none;
  opacity: 0;
}}
"#
    );
    provider.load_from_string(&css);

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}

fn set_mode_button_active(button: &gtk::Button, active: bool) {
    if active {
        button.add_css_class("workspace-attention");
        button.add_css_class("automatisation-mode-button-active");
    } else {
        button.remove_css_class("workspace-attention");
        button.remove_css_class("automatisation-mode-button-active");
    }
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

    if let Some(remote_thread_id) = thread.remote_thread_id_owned() {
        active_thread_id.replace(Some(remote_thread_id));
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
        .map_err(|err| err.to_string())?
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
        summary: "Queued for background execution.".to_string(),
        error: None,
        timeline: vec![AutomationRunEvent {
            at: unix_now(),
            status: "queued".to_string(),
            title: "Queued".to_string(),
            detail: "Automation run was queued and is preparing a runtime thread.".to_string(),
        }],
    };
    push_run(db.as_ref(), run.clone())?;

    let automation_id = automation.id.clone();
    let prompt = build_prompt(
        db.as_ref(),
        &automation.prompt,
        &automation.skill_hints,
        &automation.selected_skill_keys,
        &automation.selected_mcp_keys,
    );
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
                    "Runtime Ready",
                    Some("Runtime thread created. Dispatching the prompt.".to_string()),
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
                        let _ = set_automation_error(&db, &automation_id, None);
                        let _ = update_run_status(
                            &db,
                            &run_id,
                            "started",
                            "Prompt Dispatched",
                            Some(
                                "The prompt was sent successfully. Open the thread to review progress."
                                    .to_string(),
                            ),
                            None,
                            Some(remote_thread_id),
                        );
                    }
                    Err(err) => {
                        let _ = set_automation_error(&db, &automation_id, Some(err.clone()));
                        let _ = update_run_status(
                            &db,
                            &run_id,
                            "failed",
                            "Prompt Dispatch Failed",
                            Some(
                                "The runtime thread was created, but the prompt failed."
                                    .to_string(),
                            ),
                            Some(err),
                            Some(remote_thread_id),
                        );
                    }
                }
            }
            Err(err) => {
                let _ = set_automation_error(&db, &automation_id, Some(err.clone()));
                let _ = update_run_status(
                    &db,
                    &run_id,
                    "failed",
                    "Runtime Start Failed",
                    Some("The runtime thread could not be created.".to_string()),
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
            let mut failed = automation.clone();
            failed.last_error = Some(err.clone());
            failed.updated_at = now;
            let _ = upsert_automation(db.as_ref(), failed);
            let _ = set_automation_error(db.as_ref(), &automation.id, Some(err));
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

    let page_shell = gtk::Box::new(gtk::Orientation::Vertical, 16);
    page_shell.set_margin_start(18);
    page_shell.set_margin_end(18);
    page_shell.set_margin_top(18);
    page_shell.set_margin_bottom(18);
    root.append(&page_shell);

    let page_header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let page_header_copy = gtk::Box::new(gtk::Orientation::Vertical, 3);
    page_header_copy.set_hexpand(true);
    let page_title = gtk::Label::new(Some("Automatisation"));
    page_title.add_css_class("automatisation-title");
    page_title.set_xalign(0.0);
    let page_subtitle = gtk::Label::new(Some(
        "Schedule repeatable coding jobs, inspect run history, and drill into each run’s timeline.",
    ));
    page_subtitle.add_css_class("dim-label");
    page_subtitle.set_wrap(true);
    page_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    page_subtitle.set_xalign(0.0);
    page_header_copy.append(&page_title);
    page_header_copy.append(&page_subtitle);
    let back_button = gtk::Button::with_label("Back to Workspaces");
    back_button.add_css_class("sidebar-action-button");
    {
        let selected_page = selected_page.clone();
        back_button.connect_clicked(move |_| {
            selected_page.replace(content::MAIN_PAGE_WORKSPACES.to_string());
        });
    }
    page_header.append(&page_header_copy);
    page_header.append(&back_button);
    page_shell.append(&page_header);

    let status_label = gtk::Label::new(None);
    status_label.add_css_class("dim-label");
    status_label.set_wrap(true);
    status_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_label.set_xalign(0.0);
    status_label.set_visible(false);
    page_shell.append(&status_label);

    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.add_css_class("automatisation-split");
    split.set_wide_handle(false);
    split.set_position(360);
    split.set_resize_start_child(true);
    split.set_resize_end_child(true);
    split.set_shrink_start_child(false);
    split.set_shrink_end_child(false);
    split.set_vexpand(true);
    page_shell.append(&split);

    let left_shell = gtk::Box::new(gtk::Orientation::Vertical, 12);
    left_shell.add_css_class("automatisation-left-shell");
    let left_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let left_copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
    left_copy.set_hexpand(true);
    let left_title = gtk::Label::new(Some("Automations"));
    left_title.add_css_class("profile-section-title");
    left_title.set_xalign(0.0);
    let left_hint = gtk::Label::new(Some("Existing jobs"));
    left_hint.add_css_class("dim-label");
    left_hint.set_xalign(0.0);
    left_copy.append(&left_title);
    left_copy.append(&left_hint);
    let new_button = gtk::Button::with_label("New");
    new_button.add_css_class("sidebar-action-button");
    left_header.append(&left_copy);
    left_header.append(&new_button);
    left_shell.append(&left_header);

    let left_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    left_scroll.add_css_class("automatisation-left-scroll");
    let left_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    left_list.add_css_class("automatisation-left-list");
    left_scroll.set_child(Some(&left_list));
    left_shell.append(&left_scroll);
    split.set_start_child(Some(&left_shell));

    let right_shell = gtk::Box::new(gtk::Orientation::Vertical, 12);
    right_shell.add_css_class("automatisation-right-shell");
    let right_header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let right_copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
    right_copy.set_hexpand(true);
    let detail_title = gtk::Label::new(Some("Overview"));
    detail_title.add_css_class("profile-section-title");
    detail_title.set_xalign(0.0);
    let detail_subtitle = gtk::Label::new(Some("Select an automation or create a new one."));
    detail_subtitle.add_css_class("dim-label");
    detail_subtitle.set_wrap(true);
    detail_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    detail_subtitle.set_xalign(0.0);
    right_copy.append(&detail_title);
    right_copy.append(&detail_subtitle);
    let right_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    right_actions.add_css_class("automatisation-header-actions");
    let history_button = gtk::Button::with_label("History");
    history_button.add_css_class("sidebar-action-button");
    history_button.add_css_class("automatisation-mode-button");
    let settings_button = gtk::Button::with_label("Settings");
    settings_button.add_css_class("sidebar-action-button");
    settings_button.add_css_class("automatisation-mode-button");
    let timeline_back_button = gtk::Button::with_label("Back");
    timeline_back_button.add_css_class("sidebar-action-button");
    right_actions.append(&history_button);
    right_actions.append(&settings_button);
    right_actions.append(&timeline_back_button);
    right_header.append(&right_copy);
    right_header.append(&right_actions);
    right_shell.append(&right_header);

    let detail_stack = gtk::Stack::new();
    detail_stack.set_hexpand(true);
    detail_stack.set_vexpand(true);
    detail_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    detail_stack.set_transition_duration(120);
    right_shell.append(&detail_stack);
    split.set_end_child(Some(&right_shell));

    let info_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
    info_box.add_css_class("profile-settings-section");
    info_box.add_css_class("automatisation-card");
    let info_title = gtk::Label::new(Some("How this page works"));
    info_title.add_css_class("profile-section-title");
    info_title.set_xalign(0.0);
    let info_body = gtk::Label::new(Some(
        "Choose an automation on the left to inspect its run history. Create a new automation to edit settings. Open a historical run to view its timeline and jump into the resulting thread.",
    ));
    info_body.add_css_class("dim-label");
    info_body.set_wrap(true);
    info_body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    info_body.set_xalign(0.0);
    let info_points = gtk::Label::new(Some(
        "Current implementation: jobs run locally while Enzim stays open, each run creates a reviewable thread, and timeline events are recorded as the runtime starts and dispatches prompts.",
    ));
    info_points.add_css_class("dim-label");
    info_points.set_wrap(true);
    info_points.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    info_points.set_xalign(0.0);
    info_box.append(&info_title);
    info_box.append(&info_body);
    info_box.append(&info_points);
    detail_stack.add_named(&info_box, Some(DETAIL_INFO));

    let settings_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    settings_scroll.add_css_class("automatisation-detail-scroll");
    let settings_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
    settings_box.add_css_class("automatisation-settings-shell");
    settings_scroll.set_child(Some(&settings_box));
    detail_stack.add_named(&settings_scroll, Some(DETAIL_SETTINGS));

    let workspace_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    workspace_section.add_css_class("profile-settings-section");
    workspace_section.add_css_class("automatisation-settings-card");
    let workspace_header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let workspace_title = gtk::Label::new(Some("Workspace"));
    workspace_title.add_css_class("profile-section-title");
    workspace_title.set_xalign(0.0);
    let workspace_subtitle = gtk::Label::new(Some(
        "Choose the workspace, runtime profile, and access mode for this automation.",
    ));
    workspace_subtitle.add_css_class("dim-label");
    workspace_subtitle.set_wrap(true);
    workspace_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    workspace_subtitle.set_xalign(0.0);
    workspace_header.append(&workspace_title);
    workspace_header.append(&workspace_subtitle);
    workspace_section.append(&workspace_header);

    let workspace_grid = gtk::Grid::new();
    workspace_grid.add_css_class("automatisation-settings-grid");
    workspace_grid.set_column_spacing(14);
    workspace_grid.set_row_spacing(12);
    workspace_grid.set_hexpand(true);
    workspace_section.append(&workspace_grid);

    let name_label = gtk::Label::new(Some("Name"));
    name_label.set_xalign(0.0);
    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some("Nightly refactor sweep"));
    workspace_grid.attach(&name_label, 0, 0, 1, 1);
    workspace_grid.attach(&name_entry, 0, 1, 1, 1);

    let workspace_label = gtk::Label::new(Some("Workspace"));
    workspace_label.set_xalign(0.0);
    let workspace_model = gtk::StringList::new(&[]);
    let workspace_dropdown =
        gtk::DropDown::new(Some(workspace_model.clone()), None::<&gtk::Expression>);
    workspace_dropdown.set_hexpand(true);
    workspace_grid.attach(&workspace_label, 1, 0, 1, 1);
    workspace_grid.attach(&workspace_dropdown, 1, 1, 1, 1);

    let profile_label = gtk::Label::new(Some("Profile"));
    profile_label.set_xalign(0.0);
    let profile_model = gtk::StringList::new(&[]);
    let profile_dropdown =
        gtk::DropDown::new(Some(profile_model.clone()), None::<&gtk::Expression>);
    profile_dropdown.set_hexpand(true);
    workspace_grid.attach(&profile_label, 0, 2, 1, 1);
    workspace_grid.attach(&profile_dropdown, 0, 3, 1, 1);

    let access_label = gtk::Label::new(Some("Access"));
    access_label.set_xalign(0.0);
    let access_model = gtk::StringList::new(
        &access_mode_options()
            .iter()
            .map(|(_, label)| *label)
            .collect::<Vec<_>>(),
    );
    let access_dropdown = gtk::DropDown::new(Some(access_model), None::<&gtk::Expression>);
    access_dropdown.set_hexpand(true);
    workspace_grid.attach(&access_label, 1, 2, 1, 1);
    workspace_grid.attach(&access_dropdown, 1, 3, 1, 1);
    settings_box.append(&workspace_section);

    let model_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    model_section.add_css_class("profile-settings-section");
    model_section.add_css_class("automatisation-settings-card");
    let model_header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let model_header_title = gtk::Label::new(Some("Model"));
    model_header_title.add_css_class("profile-section-title");
    model_header_title.set_xalign(0.0);
    let model_header_subtitle = gtk::Label::new(Some(
        "Select the model and variant or reasoning level exposed by the chosen profile.",
    ));
    model_header_subtitle.add_css_class("dim-label");
    model_header_subtitle.set_wrap(true);
    model_header_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    model_header_subtitle.set_xalign(0.0);
    model_header.append(&model_header_title);
    model_header.append(&model_header_subtitle);
    model_section.append(&model_header);

    let model_grid = gtk::Grid::new();
    model_grid.add_css_class("automatisation-settings-grid");
    model_grid.set_column_spacing(14);
    model_grid.set_row_spacing(12);
    model_grid.set_hexpand(true);
    model_section.append(&model_grid);

    let model_label = gtk::Label::new(Some("Model Override"));
    model_label.set_xalign(0.0);
    let model_list = gtk::StringList::new(&[]);
    let model_dropdown = gtk::DropDown::new(Some(model_list.clone()), None::<&gtk::Expression>);
    model_dropdown.set_hexpand(true);
    let effort_label = gtk::Label::new(Some("Effort / Variant"));
    effort_label.set_xalign(0.0);
    let effort_list = gtk::StringList::new(&[]);
    let effort_dropdown = gtk::DropDown::new(Some(effort_list.clone()), None::<&gtk::Expression>);
    effort_dropdown.set_hexpand(true);
    model_grid.attach(&model_label, 0, 0, 1, 1);
    model_grid.attach(&model_dropdown, 0, 1, 1, 1);
    model_grid.attach(&effort_label, 1, 0, 1, 1);
    model_grid.attach(&effort_dropdown, 1, 1, 1, 1);
    settings_box.append(&model_section);

    let schedule_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    schedule_section.add_css_class("profile-settings-section");
    schedule_section.add_css_class("automatisation-settings-card");
    let schedule_header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let schedule_title = gtk::Label::new(Some("Schedule"));
    schedule_title.add_css_class("profile-section-title");
    schedule_title.set_xalign(0.0);
    let schedule_subtitle = gtk::Label::new(Some(
        "Keep it simple with an interval, or target exact times on selected weekdays.",
    ));
    schedule_subtitle.add_css_class("dim-label");
    schedule_subtitle.set_wrap(true);
    schedule_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    schedule_subtitle.set_xalign(0.0);
    schedule_header.append(&schedule_title);
    schedule_header.append(&schedule_subtitle);
    schedule_section.append(&schedule_header);

    let schedule_top_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    schedule_top_row.add_css_class("automatisation-schedule-top-row");
    let schedule_mode_model = gtk::StringList::new(
        &schedule_mode_options()
            .iter()
            .map(|(_, label)| *label)
            .collect::<Vec<_>>(),
    );
    let schedule_mode_dropdown =
        gtk::DropDown::new(Some(schedule_mode_model), None::<&gtk::Expression>);
    schedule_mode_dropdown.set_hexpand(true);
    let enabled_switch = gtk::Switch::new();
    enabled_switch.set_active(true);
    let enabled_shell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    enabled_shell.add_css_class("automatisation-toggle-shell");
    let enabled_title = gtk::Label::new(Some("Enabled"));
    enabled_title.add_css_class("profile-section-title");
    enabled_title.set_xalign(0.0);
    enabled_shell.append(&enabled_title);
    enabled_shell.append(&enabled_switch);
    schedule_top_row.append(&schedule_mode_dropdown);
    schedule_top_row.append(&enabled_shell);
    schedule_section.append(&schedule_top_row);

    let schedule_stack = gtk::Stack::new();
    schedule_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    schedule_stack.set_transition_duration(120);
    schedule_section.append(&schedule_stack);

    let manual_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let manual_hint = gtk::Label::new(Some(
        "Manual mode disables automatic runs. Use Run Now from history or settings.",
    ));
    manual_hint.add_css_class("dim-label");
    manual_hint.set_wrap(true);
    manual_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    manual_hint.set_xalign(0.0);
    manual_box.append(&manual_hint);
    schedule_stack.add_named(&manual_box, Some("manual"));

    let interval_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let interval_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    interval_row.add_css_class("automatisation-schedule-row");
    let interval_spin = gtk::SpinButton::with_range(1.0, 365.0, 1.0);
    interval_spin.set_value(1.0);
    interval_spin.set_hexpand(true);
    let interval_unit_model = gtk::StringList::new(
        &interval_unit_options()
            .iter()
            .map(|(_, label)| *label)
            .collect::<Vec<_>>(),
    );
    let interval_unit_dropdown =
        gtk::DropDown::new(Some(interval_unit_model), None::<&gtk::Expression>);
    interval_unit_dropdown.set_hexpand(true);
    interval_row.append(&interval_spin);
    interval_row.append(&interval_unit_dropdown);
    let interval_hint = gtk::Label::new(Some("Example: every 2 hours or every 1 day."));
    interval_hint.add_css_class("dim-label");
    interval_hint.set_xalign(0.0);
    interval_box.append(&interval_row);
    interval_box.append(&interval_hint);
    schedule_stack.add_named(&interval_box, Some("interval"));

    let weekly_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let weekly_days_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    weekly_days_row.add_css_class("automatisation-weekday-row");
    let weekday_buttons = weekday_options()
        .into_iter()
        .map(|(value, label)| {
            let button = gtk::ToggleButton::with_label(label);
            button.add_css_class("automatisation-weekday-button");
            weekly_days_row.append(&button);
            (value.to_string(), button)
        })
        .collect::<Vec<_>>();
    weekly_box.append(&weekly_days_row);

    let weekly_time_shell = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let weekly_time_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let weekly_time_title = gtk::Label::new(Some("Times"));
    weekly_time_title.add_css_class("profile-section-title");
    weekly_time_title.set_xalign(0.0);
    weekly_time_title.set_hexpand(true);
    let add_time_button = gtk::Button::with_label("Add Time");
    add_time_button.add_css_class("sidebar-action-button");
    weekly_time_header.append(&weekly_time_title);
    weekly_time_header.append(&add_time_button);
    weekly_time_shell.append(&weekly_time_header);
    let weekly_time_flow = gtk::FlowBox::new();
    weekly_time_flow.set_selection_mode(gtk::SelectionMode::None);
    weekly_time_flow.set_column_spacing(8);
    weekly_time_flow.set_row_spacing(8);
    weekly_time_flow.add_css_class("automatisation-time-flow");
    weekly_time_shell.append(&weekly_time_flow);
    let weekly_time_hint = gtk::Label::new(Some(
        "Pick one or more times. Example: 11:35 and 17:30 on Wednesdays and Fridays.",
    ));
    weekly_time_hint.add_css_class("dim-label");
    weekly_time_hint.set_wrap(true);
    weekly_time_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    weekly_time_hint.set_xalign(0.0);
    weekly_time_shell.append(&weekly_time_hint);
    weekly_box.append(&weekly_time_shell);
    schedule_stack.add_named(&weekly_box, Some("weekly"));
    settings_box.append(&schedule_section);

    let add_time_popover = gtk::Popover::new();
    add_time_popover.set_has_arrow(true);
    add_time_popover.set_position(gtk::PositionType::Bottom);
    add_time_popover.set_parent(&add_time_button);
    let add_time_root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    add_time_root.set_margin_start(10);
    add_time_root.set_margin_end(10);
    add_time_root.set_margin_top(10);
    add_time_root.set_margin_bottom(10);
    let add_time_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let hour_spin = gtk::SpinButton::with_range(0.0, 23.0, 1.0);
    let minute_spin = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
    let add_time_confirm = gtk::Button::with_label("Add");
    add_time_confirm.add_css_class("suggested-action");
    add_time_row.append(&hour_spin);
    add_time_row.append(&minute_spin);
    add_time_root.append(&add_time_row);
    add_time_root.append(&add_time_confirm);
    add_time_popover.set_child(Some(&add_time_root));

    let tools_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    tools_section.add_css_class("profile-settings-section");
    tools_section.add_css_class("automatisation-settings-card");
    let tools_header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let tools_title = gtk::Label::new(Some("Tools"));
    tools_title.add_css_class("profile-section-title");
    tools_title.set_xalign(0.0);
    let tools_subtitle = gtk::Label::new(Some(
        "Select the skills and MCP servers the automation should prefer when it runs.",
    ));
    tools_subtitle.add_css_class("dim-label");
    tools_subtitle.set_wrap(true);
    tools_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    tools_subtitle.set_xalign(0.0);
    tools_header.append(&tools_title);
    tools_header.append(&tools_subtitle);
    tools_section.append(&tools_header);

    let skills_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let skills_button = gtk::Button::with_label("Skills");
    skills_button.add_css_class("sidebar-action-button");
    let skills_summary = gtk::Label::new(Some("No skills selected"));
    skills_summary.add_css_class("dim-label");
    skills_summary.set_xalign(0.0);
    skills_summary.set_hexpand(true);
    skills_row.append(&skills_button);
    skills_row.append(&skills_summary);
    tools_section.append(&skills_row);

    let mcp_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let mcp_button = gtk::Button::with_label("MCP Servers");
    mcp_button.add_css_class("sidebar-action-button");
    let mcp_summary = gtk::Label::new(Some("No MCP servers selected"));
    mcp_summary.add_css_class("dim-label");
    mcp_summary.set_xalign(0.0);
    mcp_summary.set_hexpand(true);
    mcp_row.append(&mcp_button);
    mcp_row.append(&mcp_summary);
    tools_section.append(&mcp_row);

    let tool_notes_label = gtk::Label::new(Some("Extra guidance"));
    tool_notes_label.set_xalign(0.0);
    let tool_notes_entry = gtk::Entry::new();
    tool_notes_entry.set_placeholder_text(Some(
        "Optional extra guidance for selected skills or MCP servers.",
    ));
    tools_section.append(&tool_notes_label);
    tools_section.append(&tool_notes_entry);
    settings_box.append(&tools_section);

    let skills_popover = gtk::Popover::new();
    skills_popover.set_has_arrow(true);
    skills_popover.set_position(gtk::PositionType::Bottom);
    skills_popover.set_parent(&skills_button);
    skills_popover.add_css_class("actions-popover");
    let skills_popover_root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    skills_popover_root.set_margin_start(8);
    skills_popover_root.set_margin_end(8);
    skills_popover_root.set_margin_top(8);
    skills_popover_root.set_margin_bottom(8);
    skills_popover_root.set_size_request(420, -1);
    let skills_popover_title = gtk::Label::new(Some("Skills"));
    skills_popover_title.add_css_class("actions-popover-title");
    skills_popover_title.set_xalign(0.0);
    let skills_popover_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let skills_popover_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(220)
        .child(&skills_popover_list)
        .build();
    skills_popover_root.append(&skills_popover_title);
    skills_popover_root.append(&skills_popover_scroll);
    skills_popover.set_child(Some(&skills_popover_root));

    let mcp_popover = gtk::Popover::new();
    mcp_popover.set_has_arrow(true);
    mcp_popover.set_position(gtk::PositionType::Bottom);
    mcp_popover.set_parent(&mcp_button);
    mcp_popover.add_css_class("actions-popover");
    let mcp_popover_root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    mcp_popover_root.set_margin_start(8);
    mcp_popover_root.set_margin_end(8);
    mcp_popover_root.set_margin_top(8);
    mcp_popover_root.set_margin_bottom(8);
    mcp_popover_root.set_size_request(420, -1);
    let mcp_popover_title = gtk::Label::new(Some("MCP Servers"));
    mcp_popover_title.add_css_class("actions-popover-title");
    mcp_popover_title.set_xalign(0.0);
    let mcp_popover_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let mcp_popover_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(220)
        .child(&mcp_popover_list)
        .build();
    mcp_popover_root.append(&mcp_popover_title);
    mcp_popover_root.append(&mcp_popover_scroll);
    mcp_popover.set_child(Some(&mcp_popover_root));

    let prompt_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    prompt_section.add_css_class("profile-settings-section");
    prompt_section.add_css_class("automatisation-settings-card");
    let prompt_header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let prompt_header_title = gtk::Label::new(Some("Prompt"));
    prompt_header_title.add_css_class("profile-section-title");
    prompt_header_title.set_xalign(0.0);
    let prompt_header_subtitle = gtk::Label::new(Some(
        "Define the job clearly. This uses the same editor treatment as Enzim Agent prompts.",
    ));
    prompt_header_subtitle.add_css_class("dim-label");
    prompt_header_subtitle.set_wrap(true);
    prompt_header_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    prompt_header_subtitle.set_xalign(0.0);
    prompt_header.append(&prompt_header_title);
    prompt_header.append(&prompt_header_subtitle);
    prompt_section.append(&prompt_header);

    let prompt_view = gtk::TextView::new();
    prompt_view.set_wrap_mode(gtk::WrapMode::WordChar);
    prompt_view.set_top_margin(10);
    prompt_view.set_bottom_margin(10);
    prompt_view.set_left_margin(10);
    prompt_view.set_right_margin(10);
    prompt_view.add_css_class("composer-input-view");
    let prompt_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(170)
        .child(&prompt_view)
        .build();
    prompt_scroll.set_has_frame(false);
    prompt_scroll.add_css_class("composer-input");
    prompt_scroll.add_css_class("automatisation-prompt-editor");
    apply_prompt_editor_theme(
        &prompt_scroll,
        &prompt_view,
        "automatisation-prompt-scroll",
        "automatisation-prompt-view",
    );
    prompt_section.append(&prompt_scroll);
    settings_box.append(&prompt_section);

    let settings_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    settings_actions.add_css_class("automatisation-settings-actions");
    let save_button = gtk::Button::with_label("Save");
    save_button.add_css_class("suggested-action");
    let reset_button = gtk::Button::with_label("Reset");
    let run_button = gtk::Button::with_label("Run Now");
    let delete_button = gtk::Button::with_label("Delete");
    reset_button.add_css_class("sidebar-action-button");
    run_button.add_css_class("sidebar-action-button");
    delete_button.add_css_class("destructive-action");
    settings_actions.append(&save_button);
    settings_actions.append(&reset_button);
    settings_actions.append(&run_button);
    settings_actions.append(&delete_button);
    settings_box.append(&settings_actions);

    let history_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    history_scroll.add_css_class("automatisation-detail-scroll");
    let history_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    history_scroll.set_child(Some(&history_box));
    detail_stack.add_named(&history_scroll, Some(DETAIL_HISTORY));

    let timeline_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    timeline_scroll.add_css_class("automatisation-detail-scroll");
    let timeline_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    timeline_scroll.set_child(Some(&timeline_box));
    detail_stack.add_named(&timeline_scroll, Some(DETAIL_TIMELINE));

    let selected_automation_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let selected_run_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let detail_mode: Rc<RefCell<String>> = Rc::new(RefCell::new(DETAIL_INFO.to_string()));
    let workspace_items: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let profile_items: Rc<RefCell<Vec<(i64, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let model_value_items: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let effort_value_items: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let available_models: Rc<RefCell<Vec<crate::services::app::runtime::ModelInfo>>> =
        Rc::new(RefCell::new(Vec::new()));
    let selected_skill_keys: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let selected_mcp_keys: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let weekly_times_state: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let profile_is_opencode: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let loaded_form_automation_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

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

    let refresh_skills_summary: Rc<dyn Fn()> = {
        let db = db.clone();
        let selected_skill_keys = selected_skill_keys.clone();
        let skills_summary = skills_summary.clone();
        Rc::new(move || {
            let catalog = crate::services::app::skills::load_catalog(db.as_ref());
            let names = catalog
                .skills
                .into_iter()
                .filter(|item| selected_skill_keys.borrow().contains(&item.key))
                .map(|item| item.name)
                .collect::<Vec<_>>();
            skills_summary.set_text(&selection_summary(&names, "No skills selected"));
        })
    };

    let refresh_mcp_summary: Rc<dyn Fn()> = {
        let db = db.clone();
        let selected_mcp_keys = selected_mcp_keys.clone();
        let mcp_summary = mcp_summary.clone();
        Rc::new(move || {
            let catalog = crate::services::app::skills::load_catalog(db.as_ref());
            let names = catalog
                .mcps
                .into_iter()
                .filter(|item| selected_mcp_keys.borrow().contains(&item.key))
                .map(|item| item.name)
                .collect::<Vec<_>>();
            mcp_summary.set_text(&selection_summary(&names, "No MCP servers selected"));
        })
    };

    let refresh_time_flow_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let refresh_time_flow: Rc<dyn Fn()> = {
        let weekly_time_flow = weekly_time_flow.clone();
        let weekly_times_state = weekly_times_state.clone();
        let refresh_time_flow_handle = refresh_time_flow_handle.clone();
        Rc::new(move || {
            while let Some(child) = weekly_time_flow.first_child() {
                weekly_time_flow.remove(&child);
            }
            let values = weekly_times_state.borrow().clone();
            if values.is_empty() {
                let empty = gtk::Label::new(Some("No exact times selected yet."));
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                weekly_time_flow.insert(&empty, -1);
                return;
            }
            for time_value in values {
                let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                chip.add_css_class("automatisation-time-chip");
                let label = gtk::Label::new(Some(&time_value));
                label.set_xalign(0.0);
                let remove = gtk::Button::with_label("Remove");
                remove.add_css_class("sidebar-action-button");
                remove.add_css_class("automatisation-chip-button");
                {
                    let weekly_times_state = weekly_times_state.clone();
                    let refresh_time_flow_handle = refresh_time_flow_handle.clone();
                    let time_value = time_value.clone();
                    remove.connect_clicked(move |_| {
                        weekly_times_state
                            .borrow_mut()
                            .retain(|value| value != &time_value);
                        if let Some(refresh) = refresh_time_flow_handle.borrow().as_ref() {
                            refresh();
                        }
                    });
                }
                chip.append(&label);
                chip.append(&remove);
                weekly_time_flow.insert(&chip, -1);
            }
        })
    };
    refresh_time_flow_handle.replace(Some(refresh_time_flow.clone()));

    let set_schedule_mode_ui: Rc<dyn Fn(&str)> = {
        let schedule_stack = schedule_stack.clone();
        let schedule_mode_dropdown = schedule_mode_dropdown.clone();
        Rc::new(move |mode: &str| {
            let normalized = match mode {
                "manual" | "weekly" => mode,
                _ => "interval",
            };
            schedule_mode_dropdown
                .set_selected(find_option_index(&schedule_mode_options(), normalized));
            schedule_stack.set_visible_child_name(normalized);
        })
    };

    let set_weekly_days: Rc<dyn Fn(&[String])> = {
        let weekday_buttons = weekday_buttons.clone();
        Rc::new(move |days: &[String]| {
            for (value, button) in &weekday_buttons {
                button.set_active(days.iter().any(|day| day == value));
            }
        })
    };

    let refresh_effort_dropdown: Rc<dyn Fn(Option<&str>)> = {
        let available_models = available_models.clone();
        let model_dropdown = model_dropdown.clone();
        let effort_list = effort_list.clone();
        let effort_dropdown = effort_dropdown.clone();
        let effort_value_items = effort_value_items.clone();
        let effort_label = effort_label.clone();
        let profile_is_opencode = profile_is_opencode.clone();
        Rc::new(move |selected_value: Option<&str>| {
            while effort_list.n_items() > 0 {
                effort_list.remove(0);
            }
            let models = available_models.borrow().clone();
            let model_values = models
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            let selected_model_id = model_values
                .get(model_dropdown.selected() as usize)
                .cloned()
                .unwrap_or_default();
            let options = if *profile_is_opencode.borrow() {
                effort_label.set_text("Variant");
                crate::ui::components::chat::runtime_controls::opencode_variant_options_from_models(
                    &models,
                    &selected_model_id,
                )
            } else {
                effort_label.set_text("Effort");
                crate::ui::components::chat::runtime_controls::reasoning_effort_options_from_models(
                    &models,
                    &selected_model_id,
                )
                .0
            };
            let mut values = Vec::new();
            let mut selected_index = gtk::INVALID_LIST_POSITION;
            if options.is_empty() {
                effort_list.append("Default");
                values.push(String::new());
                selected_index = 0;
            } else {
                for (index, (label, value)) in options.iter().enumerate() {
                    effort_list.append(label);
                    values.push(value.clone());
                    if selected_value == Some(value.as_str()) {
                        selected_index = index as u32;
                    }
                }
                if selected_index == gtk::INVALID_LIST_POSITION {
                    selected_index = 0;
                }
            }
            effort_dropdown.set_selected(selected_index);
            effort_value_items.replace(values);
        })
    };

    let refresh_model_dropdown: Rc<dyn Fn(Option<i64>, Option<&str>, Option<&str>)> = {
        let manager = manager.clone();
        let model_list = model_list.clone();
        let model_dropdown = model_dropdown.clone();
        let model_value_items = model_value_items.clone();
        let available_models = available_models.clone();
        let profile_is_opencode = profile_is_opencode.clone();
        let refresh_effort_dropdown = refresh_effort_dropdown.clone();
        Rc::new(
            move |profile_id: Option<i64>,
                  selected_model: Option<&str>,
                  selected_effort: Option<&str>| {
                while model_list.n_items() > 0 {
                    model_list.remove(0);
                }
                let mut model_values = Vec::new();
                let mut models = Vec::new();
                let mut selected_index = gtk::INVALID_LIST_POSITION;
                let mut is_opencode = false;
                if let Some(profile_id) = profile_id.filter(|value| *value > 0) {
                    if let Ok(client) = manager.ensure_started(profile_id) {
                        is_opencode = client.backend_kind().eq_ignore_ascii_case("opencode");
                        let _ = crate::ui::components::chat::runtime_controls::refresh_model_options_cache(
                            Some(&client),
                        );
                        models = crate::ui::components::chat::runtime_controls::model_options(
                            Some(&client),
                        );
                    }
                }
                if models.is_empty() {
                    model_list.append("Profile default");
                    model_values.push(String::new());
                    selected_index = 0;
                } else {
                    for (index, model) in models.iter().enumerate() {
                        model_list.append(&model.display_name);
                        model_values.push(model.id.clone());
                        if selected_model == Some(model.id.as_str()) {
                            selected_index = index as u32;
                        }
                    }
                    if selected_index == gtk::INVALID_LIST_POSITION {
                        selected_index = models
                            .iter()
                            .position(|item| item.is_default)
                            .map(|index| index as u32)
                            .unwrap_or(0);
                    }
                }
                profile_is_opencode.replace(is_opencode);
                available_models.replace(models);
                model_value_items.replace(model_values);
                model_dropdown.set_selected(selected_index);
                refresh_effort_dropdown(selected_effort);
            },
        )
    };

    let clear_form: Rc<dyn Fn()> = {
        let loaded_form_automation_id = loaded_form_automation_id.clone();
        let name_entry = name_entry.clone();
        let prompt_view = prompt_view.clone();
        let tool_notes_entry = tool_notes_entry.clone();
        let interval_spin = interval_spin.clone();
        let interval_unit_dropdown = interval_unit_dropdown.clone();
        let enabled_switch = enabled_switch.clone();
        let access_dropdown = access_dropdown.clone();
        let selected_skill_keys = selected_skill_keys.clone();
        let selected_mcp_keys = selected_mcp_keys.clone();
        let weekly_times_state = weekly_times_state.clone();
        let refresh_time_flow = refresh_time_flow.clone();
        let refresh_skills_summary = refresh_skills_summary.clone();
        let refresh_mcp_summary = refresh_mcp_summary.clone();
        let reload_workspace_dropdown = reload_workspace_dropdown.clone();
        let reload_profile_dropdown = reload_profile_dropdown.clone();
        let profile_items = profile_items.clone();
        let refresh_model_dropdown = refresh_model_dropdown.clone();
        let set_schedule_mode_ui = set_schedule_mode_ui.clone();
        let set_weekly_days = set_weekly_days.clone();
        Rc::new(move || {
            loaded_form_automation_id.replace(None);
            name_entry.set_text("");
            prompt_view.buffer().set_text("");
            tool_notes_entry.set_text("");
            interval_spin.set_value(1.0);
            interval_unit_dropdown
                .set_selected(find_option_index(&interval_unit_options(), "hour"));
            enabled_switch.set_active(true);
            access_dropdown.set_selected(0);
            selected_skill_keys.borrow_mut().clear();
            selected_mcp_keys.borrow_mut().clear();
            weekly_times_state.borrow_mut().clear();
            refresh_time_flow();
            refresh_skills_summary();
            refresh_mcp_summary();
            set_schedule_mode_ui("interval");
            set_weekly_days(&[]);
            reload_workspace_dropdown(None);
            reload_profile_dropdown(None);
            let default_profile_id = profile_items.borrow().first().map(|(id, _)| *id);
            refresh_model_dropdown(default_profile_id, None, None);
        })
    };

    let load_form_from_automation: Rc<dyn Fn(Option<&AutomationDefinition>)> = {
        let loaded_form_automation_id = loaded_form_automation_id.clone();
        let clear_form = clear_form.clone();
        let name_entry = name_entry.clone();
        let prompt_view = prompt_view.clone();
        let tool_notes_entry = tool_notes_entry.clone();
        let interval_spin = interval_spin.clone();
        let interval_unit_dropdown = interval_unit_dropdown.clone();
        let enabled_switch = enabled_switch.clone();
        let access_dropdown = access_dropdown.clone();
        let selected_skill_keys = selected_skill_keys.clone();
        let selected_mcp_keys = selected_mcp_keys.clone();
        let weekly_times_state = weekly_times_state.clone();
        let refresh_time_flow = refresh_time_flow.clone();
        let refresh_skills_summary = refresh_skills_summary.clone();
        let refresh_mcp_summary = refresh_mcp_summary.clone();
        let reload_workspace_dropdown = reload_workspace_dropdown.clone();
        let reload_profile_dropdown = reload_profile_dropdown.clone();
        let refresh_model_dropdown = refresh_model_dropdown.clone();
        let set_schedule_mode_ui = set_schedule_mode_ui.clone();
        let set_weekly_days = set_weekly_days.clone();
        Rc::new(move |automation: Option<&AutomationDefinition>| {
            let Some(automation) = automation else {
                clear_form();
                return;
            };
            loaded_form_automation_id.replace(Some(automation.id.clone()));
            name_entry.set_text(&automation.name);
            prompt_view.buffer().set_text(&automation.prompt);
            tool_notes_entry.set_text(&automation.skill_hints);
            interval_spin.set_value(automation.interval_value as f64);
            interval_unit_dropdown.set_selected(find_option_index(
                &interval_unit_options(),
                &automation.interval_unit,
            ));
            enabled_switch.set_active(automation.enabled);
            access_dropdown.set_selected(find_option_index(
                &access_mode_options(),
                &automation.access_mode,
            ));
            selected_skill_keys.replace(automation.selected_skill_keys.clone());
            selected_mcp_keys.replace(automation.selected_mcp_keys.clone());
            weekly_times_state.replace(automation.weekly_times.clone());
            refresh_time_flow();
            refresh_skills_summary();
            refresh_mcp_summary();
            set_schedule_mode_ui(&automation.schedule_mode);
            set_weekly_days(&automation.weekly_days);
            reload_workspace_dropdown(Some(&automation.workspace_path));
            reload_profile_dropdown(Some(automation.profile_id));
            refresh_model_dropdown(
                Some(automation.profile_id),
                automation.model_id.as_deref(),
                automation.effort.as_deref(),
            );
        })
    };

    {
        let schedule_stack = schedule_stack.clone();
        schedule_mode_dropdown.connect_selected_notify(move |dropdown| {
            let mode = schedule_mode_options()
                .get(dropdown.selected() as usize)
                .map(|(value, _)| *value)
                .unwrap_or("interval");
            schedule_stack.set_visible_child_name(mode);
        });
    }

    {
        let profile_items = profile_items.clone();
        let refresh_model_dropdown = refresh_model_dropdown.clone();
        profile_dropdown.connect_selected_notify(move |dropdown| {
            let selected_profile_id = profile_items
                .borrow()
                .get(dropdown.selected() as usize)
                .map(|(id, _)| *id);
            refresh_model_dropdown(selected_profile_id, None, None);
        });
    }

    {
        let refresh_effort_dropdown = refresh_effort_dropdown.clone();
        model_dropdown.connect_selected_notify(move |_| {
            refresh_effort_dropdown(None);
        });
    }

    {
        let weekly_times_state = weekly_times_state.clone();
        let refresh_time_flow = refresh_time_flow.clone();
        let add_time_popover = add_time_popover.clone();
        let hour_spin = hour_spin.clone();
        let minute_spin = minute_spin.clone();
        add_time_button.connect_clicked(move |_| {
            hour_spin.set_value(9.0);
            minute_spin.set_value(0.0);
            add_time_popover.popup();
            let _ = &weekly_times_state;
            let _ = &refresh_time_flow;
        });
    }

    {
        let weekly_times_state = weekly_times_state.clone();
        let refresh_time_flow = refresh_time_flow.clone();
        let add_time_popover = add_time_popover.clone();
        let hour_spin = hour_spin.clone();
        let minute_spin = minute_spin.clone();
        add_time_confirm.connect_clicked(move |_| {
            let next_value = format!(
                "{:02}:{:02}",
                hour_spin.value() as i32,
                minute_spin.value() as i32
            );
            if !weekly_times_state
                .borrow()
                .iter()
                .any(|value| value == &next_value)
            {
                weekly_times_state.borrow_mut().push(next_value);
                weekly_times_state.borrow_mut().sort();
            }
            refresh_time_flow();
            add_time_popover.popdown();
        });
    }

    let refresh_skills_popover: Rc<dyn Fn()> = {
        let db = db.clone();
        let skills_popover_list = skills_popover_list.clone();
        let selected_skill_keys = selected_skill_keys.clone();
        let refresh_skills_summary = refresh_skills_summary.clone();
        Rc::new(move || {
            while let Some(child) = skills_popover_list.first_child() {
                skills_popover_list.remove(&child);
            }
            let catalog = crate::services::app::skills::load_catalog(db.as_ref());
            if catalog.skills.is_empty() {
                let empty = gtk::Label::new(Some("No skills in the catalog yet."));
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                skills_popover_list.append(&empty);
                return;
            }
            for skill in catalog.skills {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
                row.add_css_class("actions-command-card");
                let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let check = gtk::CheckButton::new();
                check.set_active(selected_skill_keys.borrow().contains(&skill.key));
                let name = gtk::Label::new(Some(&skill.name));
                name.add_css_class("actions-command-title");
                name.set_xalign(0.0);
                name.set_hexpand(true);
                top.append(&check);
                top.append(&name);
                row.append(&top);
                if !skill.description.trim().is_empty() {
                    let description = gtk::Label::new(Some(&skill.description));
                    description.add_css_class("dim-label");
                    description.set_wrap(true);
                    description.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    description.set_xalign(0.0);
                    row.append(&description);
                }
                {
                    let selected_skill_keys = selected_skill_keys.clone();
                    let refresh_skills_summary = refresh_skills_summary.clone();
                    let skill_key = skill.key.clone();
                    check.connect_toggled(move |toggle| {
                        let mut selected = selected_skill_keys.borrow_mut();
                        if toggle.is_active() {
                            if !selected.iter().any(|value| value == &skill_key) {
                                selected.push(skill_key.clone());
                                selected.sort();
                            }
                        } else {
                            selected.retain(|value| value != &skill_key);
                        }
                        drop(selected);
                        refresh_skills_summary();
                    });
                }
                skills_popover_list.append(&row);
            }
        })
    };

    {
        let refresh_skills_popover = refresh_skills_popover.clone();
        let skills_popover = skills_popover.clone();
        skills_button.connect_clicked(move |_| {
            refresh_skills_popover();
            if skills_popover.is_visible() {
                skills_popover.popdown();
            } else {
                skills_popover.popup();
            }
        });
    }

    let refresh_mcp_popover: Rc<dyn Fn()> = {
        let db = db.clone();
        let mcp_popover_list = mcp_popover_list.clone();
        let selected_mcp_keys = selected_mcp_keys.clone();
        let refresh_mcp_summary = refresh_mcp_summary.clone();
        Rc::new(move || {
            while let Some(child) = mcp_popover_list.first_child() {
                mcp_popover_list.remove(&child);
            }
            let catalog = crate::services::app::skills::load_catalog(db.as_ref());
            if catalog.mcps.is_empty() {
                let empty = gtk::Label::new(Some("No MCP servers in the catalog yet."));
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                mcp_popover_list.append(&empty);
                return;
            }
            for mcp in catalog.mcps {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
                row.add_css_class("actions-command-card");
                let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let check = gtk::CheckButton::new();
                check.set_active(selected_mcp_keys.borrow().contains(&mcp.key));
                let name = gtk::Label::new(Some(&mcp.name));
                name.add_css_class("actions-command-title");
                name.set_xalign(0.0);
                name.set_hexpand(true);
                top.append(&check);
                top.append(&name);
                row.append(&top);
                if !mcp.description.trim().is_empty() {
                    let description = gtk::Label::new(Some(&mcp.description));
                    description.add_css_class("dim-label");
                    description.set_wrap(true);
                    description.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    description.set_xalign(0.0);
                    row.append(&description);
                }
                {
                    let selected_mcp_keys = selected_mcp_keys.clone();
                    let refresh_mcp_summary = refresh_mcp_summary.clone();
                    let mcp_key = mcp.key.clone();
                    check.connect_toggled(move |toggle| {
                        let mut selected = selected_mcp_keys.borrow_mut();
                        if toggle.is_active() {
                            if !selected.iter().any(|value| value == &mcp_key) {
                                selected.push(mcp_key.clone());
                                selected.sort();
                            }
                        } else {
                            selected.retain(|value| value != &mcp_key);
                        }
                        drop(selected);
                        refresh_mcp_summary();
                    });
                }
                mcp_popover_list.append(&row);
            }
        })
    };

    {
        let refresh_mcp_popover = refresh_mcp_popover.clone();
        let mcp_popover = mcp_popover.clone();
        mcp_button.connect_clicked(move |_| {
            refresh_mcp_popover();
            if mcp_popover.is_visible() {
                mcp_popover.popdown();
            } else {
                mcp_popover.popup();
            }
        });
    }

    let refresh_left_list_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let render_detail_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    let refresh_left_list: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let sidebar = sidebar.clone();
        let left_list = left_list.clone();
        let selected_automation_id = selected_automation_id.clone();
        let detail_mode = detail_mode.clone();
        let selected_run_id = selected_run_id.clone();
        let load_form_from_automation = load_form_from_automation.clone();
        let refresh_left_list_handle = refresh_left_list_handle.clone();
        let render_detail_handle = render_detail_handle.clone();
        let status_label = status_label.clone();
        Rc::new(move || {
            while let Some(child) = left_list.first_child() {
                left_list.remove(&child);
            }

            let automations = load_automations(db.as_ref());
            if automations.is_empty() {
                let empty = gtk::Label::new(Some("No automations yet."));
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                left_list.append(&empty);
                return;
            }

            for automation in automations {
                let button = gtk::Button::new();
                button.set_has_frame(false);
                button.add_css_class("automatisation-list-row");
                button.set_halign(gtk::Align::Fill);
                button.set_hexpand(true);
                if selected_automation_id.borrow().as_deref() == Some(automation.id.as_str()) {
                    button.add_css_class("automatisation-list-card-active");
                }

                let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
                card.add_css_class("automatisation-list-card");
                card.add_css_class("profile-settings-section");

                let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let title_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
                title_box.set_hexpand(true);
                let title = gtk::Label::new(Some(&automation.name));
                title.add_css_class("profile-section-title");
                title.set_xalign(0.0);
                let subtitle = gtk::Label::new(Some(&format!(
                    "{} • {}",
                    automation_workspace_label(&automation.workspace_path),
                    automation_schedule_label(&automation)
                )));
                subtitle.add_css_class("dim-label");
                subtitle.set_xalign(0.0);
                title_box.append(&title);
                title_box.append(&subtitle);
                header.append(&title_box);

                let run_button = gtk::Button::with_label("Run");
                run_button.add_css_class("sidebar-action-button");
                run_button.add_css_class("automatisation-inline-button");
                header.append(&run_button);
                card.append(&header);

                let prompt = gtk::Label::new(Some(&automation.prompt));
                prompt.add_css_class("dim-label");
                prompt.set_wrap(true);
                prompt.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                prompt.set_xalign(0.0);
                prompt.set_max_width_chars(1);
                card.append(&prompt);

                if let Some(error) = automation.last_error.as_deref() {
                    let error = gtk::Label::new(Some(&format!("Last error: {error}")));
                    error.add_css_class("dim-label");
                    error.set_wrap(true);
                    error.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    error.set_xalign(0.0);
                    card.append(&error);
                }

                button.set_child(Some(&card));

                {
                    let selected_automation_id = selected_automation_id.clone();
                    let selected_run_id = selected_run_id.clone();
                    let detail_mode = detail_mode.clone();
                    let automation_id = automation.id.clone();
                    let render_detail_handle = render_detail_handle.clone();
                    button.connect_clicked(move |_| {
                        selected_automation_id.replace(Some(automation_id.clone()));
                        selected_run_id.replace(None);
                        detail_mode.replace(DETAIL_HISTORY.to_string());
                        if let Some(render) = render_detail_handle.borrow().clone() {
                            render();
                        }
                    });
                }

                {
                    let db = db.clone();
                    let manager = manager.clone();
                    let sidebar = sidebar.clone();
                    let automation_id = automation.id.clone();
                    let status_label = status_label.clone();
                    let refresh_left_list_handle = refresh_left_list_handle.clone();
                    let render_detail_handle = render_detail_handle.clone();
                    run_button.connect_clicked(move |_| {
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
                            Err(err) => {
                                status_label.set_text(&format!("Unable to start automation: {err}"))
                            }
                        }
                        if let Some(refresh) = refresh_left_list_handle.borrow().clone() {
                            refresh();
                        }
                        if let Some(render) = render_detail_handle.borrow().clone() {
                            render();
                        }
                    });
                }

                {
                    let selected_automation_id = selected_automation_id.clone();
                    let detail_mode = detail_mode.clone();
                    let automation_id = automation.id.clone();
                    let load_form_from_automation = load_form_from_automation.clone();
                    let automation = automation.clone();
                    let render_detail_handle = render_detail_handle.clone();
                    let click = gtk::GestureClick::builder().button(3).build();
                    click.connect_pressed(move |_, _, _, _| {
                        selected_automation_id.replace(Some(automation_id.clone()));
                        detail_mode.replace(DETAIL_SETTINGS.to_string());
                        load_form_from_automation(Some(&automation));
                        if let Some(render) = render_detail_handle.borrow().clone() {
                            render();
                        }
                    });
                    button.add_controller(click);
                }

                left_list.append(&button);
            }
        })
    };
    refresh_left_list_handle.replace(Some(refresh_left_list.clone()));

    let render_detail: Rc<dyn Fn()> = {
        let db = db.clone();
        let sidebar = sidebar.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let selected_page = selected_page.clone();
        let selected_automation_id = selected_automation_id.clone();
        let selected_run_id = selected_run_id.clone();
        let detail_mode = detail_mode.clone();
        let clear_form = clear_form.clone();
        let detail_title = detail_title.clone();
        let detail_subtitle = detail_subtitle.clone();
        let detail_stack = detail_stack.clone();
        let history_button = history_button.clone();
        let settings_button = settings_button.clone();
        let timeline_back_button = timeline_back_button.clone();
        let history_box = history_box.clone();
        let timeline_box = timeline_box.clone();
        let load_form_from_automation = load_form_from_automation.clone();
        let loaded_form_automation_id = loaded_form_automation_id.clone();
        let render_detail_handle_for_rows = render_detail_handle.clone();
        Rc::new(move || {
            let automations = load_automations(db.as_ref());
            let runs = load_runs(db.as_ref());
            let selected_automation = selected_automation_id.borrow().as_deref().and_then(|id| {
                automations
                    .iter()
                    .find(|automation| automation.id == id)
                    .cloned()
            });
            let selected_run = selected_run_id
                .borrow()
                .as_deref()
                .and_then(|run_id| runs.iter().find(|run| run.id == run_id).cloned());
            let current_mode = detail_mode.borrow().clone();

            history_button.set_visible(selected_automation.is_some());
            settings_button
                .set_visible(selected_automation.is_some() || current_mode == DETAIL_SETTINGS);
            timeline_back_button.set_visible(current_mode == DETAIL_TIMELINE);
            set_mode_button_active(&history_button, current_mode == DETAIL_HISTORY);
            set_mode_button_active(&settings_button, current_mode == DETAIL_SETTINGS);

            while let Some(child) = history_box.first_child() {
                history_box.remove(&child);
            }
            while let Some(child) = timeline_box.first_child() {
                timeline_box.remove(&child);
            }

            match current_mode.as_str() {
                DETAIL_SETTINGS => {
                    if let Some(automation) = selected_automation.as_ref() {
                        let should_load = loaded_form_automation_id.borrow().as_deref()
                            != Some(automation.id.as_str());
                        if should_load {
                            load_form_from_automation(Some(automation));
                        }
                    } else if loaded_form_automation_id.borrow().is_some() {
                        clear_form();
                    }
                    detail_title.set_text(if selected_automation.is_some() {
                        "Automation Settings"
                    } else {
                        "New Automation"
                    });
                    detail_subtitle.set_text(if selected_automation.is_some() {
                        "Compact settings for the selected automation."
                    } else {
                        "Create a new automation and save it to the list."
                    });
                    detail_stack.set_visible_child_name(DETAIL_SETTINGS);
                }
                DETAIL_TIMELINE => {
                    let Some(run) = selected_run else {
                        detail_mode.replace(DETAIL_INFO.to_string());
                        detail_title.set_text("Overview");
                        detail_subtitle.set_text("Select an automation or create a new one.");
                        detail_stack.set_visible_child_name(DETAIL_INFO);
                        return;
                    };
                    detail_title.set_text("Run Timeline");
                    detail_subtitle.set_text(&format!(
                        "{} • {} • {}",
                        run.automation_name,
                        automation_workspace_label(&run.workspace_path),
                        format_relative_age(run.started_at)
                    ));
                    let summary = gtk::Box::new(gtk::Orientation::Vertical, 8);
                    summary.add_css_class("profile-settings-section");
                    summary.add_css_class("automatisation-card");
                    let summary_title = gtk::Label::new(Some(&format!(
                        "Thread #{} • {}",
                        run.local_thread_id, run.status
                    )));
                    summary_title.add_css_class("profile-section-title");
                    summary_title.set_xalign(0.0);
                    let summary_body = gtk::Label::new(Some(&run.summary));
                    summary_body.add_css_class("dim-label");
                    summary_body.set_wrap(true);
                    summary_body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    summary_body.set_xalign(0.0);
                    summary.append(&summary_title);
                    summary.append(&summary_body);
                    if let Some(error) = run.error.as_deref() {
                        let error = gtk::Label::new(Some(error));
                        error.add_css_class("dim-label");
                        error.set_wrap(true);
                        error.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                        error.set_xalign(0.0);
                        summary.append(&error);
                    }
                    let open_thread = gtk::Button::with_label("Open Thread");
                    {
                        let sidebar = sidebar.clone();
                        let db = db.clone();
                        let active_thread_id = active_thread_id.clone();
                        let active_workspace_path = active_workspace_path.clone();
                        let selected_page = selected_page.clone();
                        let local_thread_id = run.local_thread_id;
                        open_thread.connect_clicked(move |_| {
                            let _ = activate_local_thread(
                                &sidebar,
                                db.as_ref(),
                                &active_thread_id,
                                &active_workspace_path,
                                &selected_page,
                                local_thread_id,
                            );
                        });
                    }
                    summary.append(&open_thread);
                    timeline_box.append(&summary);

                    for event in fallback_timeline_for_run(&run) {
                        let item = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                        item.add_css_class("automatisation-timeline-item");
                        let rail = gtk::Box::new(gtk::Orientation::Vertical, 0);
                        rail.add_css_class("automatisation-timeline-rail");
                        rail.set_vexpand(true);
                        let dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
                        dot.add_css_class("automatisation-timeline-dot");
                        rail.append(&dot);
                        let content = gtk::Box::new(gtk::Orientation::Vertical, 3);
                        let title = gtk::Label::new(Some(&event.title));
                        title.add_css_class("profile-section-title");
                        title.set_xalign(0.0);
                        let meta = gtk::Label::new(Some(&format!(
                            "{} • {}",
                            event.status,
                            format_relative_age(event.at)
                        )));
                        meta.add_css_class("dim-label");
                        meta.set_xalign(0.0);
                        let detail = gtk::Label::new(Some(&event.detail));
                        detail.add_css_class("dim-label");
                        detail.set_wrap(true);
                        detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                        detail.set_xalign(0.0);
                        content.append(&title);
                        content.append(&meta);
                        content.append(&detail);
                        item.append(&rail);
                        item.append(&content);
                        timeline_box.append(&item);
                    }
                    detail_stack.set_visible_child_name(DETAIL_TIMELINE);
                }
                DETAIL_HISTORY => {
                    let Some(automation) = selected_automation else {
                        detail_mode.replace(DETAIL_INFO.to_string());
                        detail_title.set_text("Overview");
                        detail_subtitle.set_text("Select an automation or create a new one.");
                        detail_stack.set_visible_child_name(DETAIL_INFO);
                        return;
                    };
                    detail_title.set_text(&automation.name);
                    detail_subtitle.set_text(&format!(
                        "{} • {} • {}",
                        automation_workspace_label(&automation.workspace_path),
                        automation_schedule_label(&automation),
                        if automation.enabled {
                            "Enabled"
                        } else {
                            "Paused"
                        }
                    ));

                    let hero = gtk::Box::new(gtk::Orientation::Vertical, 8);
                    hero.add_css_class("profile-settings-section");
                    hero.add_css_class("automatisation-card");
                    let hero_prompt = gtk::Label::new(Some(&automation.prompt));
                    hero_prompt.set_wrap(true);
                    hero_prompt.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    hero_prompt.set_xalign(0.0);
                    let hero_meta = gtk::Label::new(Some(&format!(
                        "Last run: {}",
                        automation
                            .last_run_at
                            .map(format_relative_age)
                            .unwrap_or_else(|| "never".to_string())
                    )));
                    hero_meta.add_css_class("dim-label");
                    hero_meta.set_xalign(0.0);
                    hero.append(&hero_prompt);
                    hero.append(&hero_meta);
                    history_box.append(&hero);

                    let related_runs = runs
                        .iter()
                        .filter(|run| run.automation_id == automation.id)
                        .cloned()
                        .collect::<Vec<_>>();
                    if related_runs.is_empty() {
                        let empty = gtk::Label::new(Some("No runs for this automation yet."));
                        empty.add_css_class("dim-label");
                        empty.set_xalign(0.0);
                        history_box.append(&empty);
                    } else {
                        for run in related_runs {
                            let row = gtk::Button::new();
                            row.set_has_frame(false);
                            row.add_css_class("automatisation-history-row");
                            row.set_halign(gtk::Align::Fill);
                            row.set_hexpand(true);
                            let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
                            card.add_css_class("profile-settings-section");
                            card.add_css_class("automatisation-list-card");
                            let title = gtk::Label::new(Some(&format!(
                                "Run {} • Thread #{}",
                                format_relative_age(run.started_at),
                                run.local_thread_id
                            )));
                            title.add_css_class("profile-section-title");
                            title.set_xalign(0.0);
                            let summary =
                                gtk::Label::new(Some(&format!("{} • {}", run.status, run.summary)));
                            summary.add_css_class("dim-label");
                            summary.set_wrap(true);
                            summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                            summary.set_xalign(0.0);
                            card.append(&title);
                            card.append(&summary);
                            row.set_child(Some(&card));
                            {
                                let selected_run_id = selected_run_id.clone();
                                let detail_mode = detail_mode.clone();
                                let render_detail_handle = render_detail_handle_for_rows.clone();
                                let run_id = run.id.clone();
                                row.connect_clicked(move |_| {
                                    selected_run_id.replace(Some(run_id.clone()));
                                    detail_mode.replace(DETAIL_TIMELINE.to_string());
                                    if let Some(render) = render_detail_handle.borrow().clone() {
                                        render();
                                    }
                                });
                            }
                            history_box.append(&row);
                        }
                    }
                    detail_stack.set_visible_child_name(DETAIL_HISTORY);
                }
                _ => {
                    detail_title.set_text("Overview");
                    detail_subtitle.set_text("Select an automation or create a new one.");
                    detail_stack.set_visible_child_name(DETAIL_INFO);
                }
            }
        })
    };
    render_detail_handle.replace(Some(render_detail.clone()));

    reload_workspace_dropdown(None);
    reload_profile_dropdown(None);
    clear_form();
    detail_stack.set_visible_child_name(DETAIL_INFO);

    {
        let selected_automation_id = selected_automation_id.clone();
        let selected_run_id = selected_run_id.clone();
        let detail_mode = detail_mode.clone();
        let clear_form = clear_form.clone();
        let render_detail = render_detail.clone();
        new_button.connect_clicked(move |_| {
            selected_automation_id.replace(None);
            selected_run_id.replace(None);
            detail_mode.replace(DETAIL_SETTINGS.to_string());
            clear_form();
            render_detail();
        });
    }
    {
        let detail_mode = detail_mode.clone();
        let selected_run_id = selected_run_id.clone();
        let render_detail = render_detail.clone();
        history_button.connect_clicked(move |_| {
            selected_run_id.replace(None);
            detail_mode.replace(DETAIL_HISTORY.to_string());
            render_detail();
        });
    }
    {
        let detail_mode = detail_mode.clone();
        let render_detail = render_detail.clone();
        settings_button.connect_clicked(move |_| {
            detail_mode.replace(DETAIL_SETTINGS.to_string());
            render_detail();
        });
    }
    {
        let selected_run_id = selected_run_id.clone();
        let detail_mode = detail_mode.clone();
        let render_detail = render_detail.clone();
        timeline_back_button.connect_clicked(move |_| {
            selected_run_id.replace(None);
            detail_mode.replace(DETAIL_HISTORY.to_string());
            render_detail();
        });
    }
    {
        let clear_form = clear_form.clone();
        reset_button.connect_clicked(move |_| clear_form());
    }

    {
        let db = db.clone();
        let selected_automation_id = selected_automation_id.clone();
        let selected_run_id = selected_run_id.clone();
        let detail_mode = detail_mode.clone();
        let status_label = status_label.clone();
        let refresh_left_list = refresh_left_list.clone();
        let render_detail = render_detail.clone();
        let workspace_items = workspace_items.clone();
        let profile_items = profile_items.clone();
        let model_value_items = model_value_items.clone();
        let effort_value_items = effort_value_items.clone();
        let name_entry = name_entry.clone();
        let prompt_view = prompt_view.clone();
        let tool_notes_entry = tool_notes_entry.clone();
        let schedule_mode_dropdown = schedule_mode_dropdown.clone();
        let interval_spin = interval_spin.clone();
        let interval_unit_dropdown = interval_unit_dropdown.clone();
        let enabled_switch = enabled_switch.clone();
        let access_dropdown = access_dropdown.clone();
        let model_dropdown = model_dropdown.clone();
        let effort_dropdown = effort_dropdown.clone();
        let selected_skill_keys = selected_skill_keys.clone();
        let selected_mcp_keys = selected_mcp_keys.clone();
        let weekly_times_state = weekly_times_state.clone();
        let weekday_buttons = weekday_buttons.clone();
        let clear_form = clear_form.clone();
        save_button.connect_clicked(move |_| {
            let selected_id = selected_automation_id.borrow().clone();
            let existing = selected_id.as_deref().and_then(|automation_id| {
                load_automations(db.as_ref())
                    .into_iter()
                    .find(|item| item.id == automation_id)
            });
            let prompt_buffer = prompt_view.buffer();
            let prompt = prompt_buffer
                .text(
                    &prompt_buffer.start_iter(),
                    &prompt_buffer.end_iter(),
                    false,
                )
                .to_string();
            let name = name_entry.text().trim().to_string();
            let workspace = workspace_items
                .borrow()
                .get(workspace_dropdown.selected() as usize)
                .cloned()
                .unwrap_or_default();
            let profile_id = profile_items
                .borrow()
                .get(profile_dropdown.selected() as usize)
                .map(|(id, _)| *id)
                .unwrap_or_default();
            if name.is_empty()
                || workspace.is_empty()
                || profile_id <= 0
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
            let schedule_mode = schedule_mode_options()
                .get(schedule_mode_dropdown.selected() as usize)
                .map(|(value, _)| (*value).to_string())
                .unwrap_or_else(|| "interval".to_string());
            let interval_unit = interval_unit_options()
                .get(interval_unit_dropdown.selected() as usize)
                .map(|(value, _)| (*value).to_string())
                .unwrap_or_else(|| "hour".to_string());
            let weekly_days = weekday_buttons
                .iter()
                .filter(|(_, button)| button.is_active())
                .map(|(value, _)| value.clone())
                .collect::<Vec<_>>();
            let weekly_times = weekly_times_state.borrow().clone();
            if schedule_mode == "weekly" && (weekly_days.is_empty() || weekly_times.is_empty()) {
                status_label.set_visible(true);
                status_label.set_text("Weekly schedules need at least one day and one time.");
                return;
            }
            let model_id = model_value_items
                .borrow()
                .get(model_dropdown.selected() as usize)
                .cloned()
                .unwrap_or_default();
            let effort = effort_value_items
                .borrow()
                .get(effort_dropdown.selected() as usize)
                .cloned()
                .unwrap_or_default();
            let saved = normalize_definition(AutomationDefinition {
                id: selected_id.unwrap_or_default(),
                name,
                workspace_path: workspace,
                profile_id,
                prompt,
                skill_hints: tool_notes_entry.text().to_string(),
                interval_minutes: 0,
                enabled: enabled_switch.is_active(),
                access_mode,
                model_id: Some(model_id),
                effort: Some(effort),
                schedule_mode,
                interval_value: interval_spin.value() as i64,
                interval_unit,
                weekly_days,
                weekly_times,
                selected_skill_keys: selected_skill_keys.borrow().clone(),
                selected_mcp_keys: selected_mcp_keys.borrow().clone(),
                created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(0),
                updated_at: 0,
                last_run_at: existing.as_ref().and_then(|item| item.last_run_at),
                next_run_at: existing.as_ref().and_then(|item| item.next_run_at),
                last_error: existing.as_ref().and_then(|item| item.last_error.clone()),
            });
            match upsert_automation(db.as_ref(), saved.clone()) {
                Ok(()) => {
                    selected_automation_id.replace(Some(saved.id.clone()));
                    selected_run_id.replace(None);
                    detail_mode.replace(DETAIL_HISTORY.to_string());
                    status_label.set_visible(true);
                    status_label.set_text("Automation saved.");
                    clear_form();
                    refresh_left_list();
                    render_detail();
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
        let selected_automation_id = selected_automation_id.clone();
        let refresh_left_list = refresh_left_list.clone();
        let render_detail = render_detail.clone();
        let status_label = status_label.clone();
        run_button.connect_clicked(move |_| {
            let Some(automation_id) = selected_automation_id.borrow().clone() else {
                status_label.set_visible(true);
                status_label.set_text("Save or select an automation first.");
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
                    "Started automation run in thread #{}.",
                    run.local_thread_id
                )),
                Err(err) => status_label.set_text(&format!("Unable to run automation: {err}")),
            }
            refresh_left_list();
            render_detail();
        });
    }

    {
        let db = db.clone();
        let selected_automation_id = selected_automation_id.clone();
        let selected_run_id = selected_run_id.clone();
        let detail_mode = detail_mode.clone();
        let clear_form = clear_form.clone();
        let refresh_left_list = refresh_left_list.clone();
        let render_detail = render_detail.clone();
        let status_label = status_label.clone();
        delete_button.connect_clicked(move |_| {
            let Some(automation_id) = selected_automation_id.borrow().clone() else {
                status_label.set_visible(true);
                status_label.set_text("Select an automation first.");
                return;
            };
            match delete_automation(db.as_ref(), &automation_id) {
                Ok(()) => {
                    selected_automation_id.replace(None);
                    selected_run_id.replace(None);
                    detail_mode.replace(DETAIL_INFO.to_string());
                    clear_form();
                    status_label.set_visible(true);
                    status_label.set_text("Automation deleted.");
                    refresh_left_list();
                    render_detail();
                }
                Err(err) => {
                    status_label.set_visible(true);
                    status_label.set_text(&format!("Unable to delete automation: {err}"));
                }
            }
        });
    }

    refresh_left_list();
    render_detail();
    {
        let refresh_left_list = refresh_left_list.clone();
        let render_detail = render_detail.clone();
        let root = root.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(1200), move || {
            if root.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            refresh_left_list();
            render_detail();
            gtk::glib::ControlFlow::Continue
        });
    }

    root
}
