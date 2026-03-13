use crate::codex_appserver::CodexAppServer;
use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use crate::ui::components::{actions_menu, chat, file_browser, git_tab, skills_mcp_menu};
use crate::ui::settings::SETTING_PANE_LAYOUT_V1;
use adw::prelude::*;
use gtk::glib::value::ToValue;
use serde_json::Value;
use serde_json::json;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct PaneUi {
    id: u64,
    shell: gtk::Box,
    root: gtk::Box,
    header: gtk::CenterBox,
    profile_icon: gtk::Image,
    title_label: gtk::Label,
    workspace_label: gtk::Label,
    stack: gtk::Stack,
    tab_buttons: Vec<gtk::Widget>,
    close_button: gtk::Box,
    active_codex_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    chat: chat::ChatPaneWidgets,
}

#[derive(Clone, Debug)]
struct PersistedPane {
    id: u64,
    codex_thread_id: Option<String>,
    workspace_path: Option<String>,
    tab: String,
    column: Option<usize>,
    row: Option<usize>,
}

#[derive(Clone, Debug)]
struct PersistedLayout {
    focused_pane_id: u64,
    panes: Vec<PersistedPane>,
    columns: Option<Vec<Vec<u64>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum InsertTarget {
    Horizontal { target_pane_id: u64, after: bool },
    Vertical { target_pane_id: u64, below: bool },
}

fn active_idx_for_tab(tab: &str) -> usize {
    match tab {
        "git" => 1,
        "files" => 2,
        _ => 0,
    }
}

fn normalize_tab_name(tab: &str) -> String {
    match tab {
        "git" => "git".to_string(),
        "files" => "files".to_string(),
        _ => "chat".to_string(),
    }
}

fn selected_tab_name(stack: &gtk::Stack) -> String {
    stack
        .visible_child_name()
        .map(|name| normalize_tab_name(name.as_str()))
        .unwrap_or_else(|| "chat".to_string())
}

fn set_tab_active(buttons: &[gtk::Widget], active_idx: usize) {
    for (idx, button) in buttons.iter().enumerate() {
        if idx == active_idx {
            button.add_css_class("top-tab-active");
        } else {
            button.remove_css_class("top-tab-active");
        }
    }
}

fn set_pane_tab(pane: &PaneUi, tab: &str) {
    let tab_name = normalize_tab_name(tab);
    pane.stack.set_visible_child_name(&tab_name);
    set_tab_active(&pane.tab_buttons, active_idx_for_tab(&tab_name));
}

fn workspace_display_name(path: Option<&str>) -> String {
    path.and_then(|p| {
        let trimmed = p.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(
                Path::new(trimmed)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.trim().is_empty())
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| trimmed.to_string()),
            )
        }
    })
    .unwrap_or_else(|| "No workspace".to_string())
}

fn profile_icon_name_for_profile(db: &AppDb, profile_id: i64) -> String {
    db.get_codex_profile(profile_id)
        .ok()
        .flatten()
        .map(|profile| profile.icon_name.trim().to_string())
        .filter(|icon_name| !icon_name.is_empty())
        .unwrap_or_else(|| "person-symbolic".to_string())
}

fn sidebar_profile_icon_visibility_by_workspace_id(db: &AppDb) -> HashMap<i64, bool> {
    db.list_workspaces_with_threads()
        .ok()
        .map(|workspaces| {
            workspaces
                .into_iter()
                .map(|workspace| {
                    let has_linked_profile = workspace.threads.into_iter().any(|thread| {
                        thread
                            .codex_thread_id
                            .as_deref()
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty())
                            || thread
                                .codex_account_type
                                .as_deref()
                                .map(str::trim)
                                .is_some_and(|value| !value.is_empty())
                            || thread
                                .codex_account_email
                                .as_deref()
                                .map(str::trim)
                                .is_some_and(|value| !value.is_empty())
                    });
                    (workspace.workspace.id, has_linked_profile)
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default()
}

fn codex_thread_exists(db: &AppDb, codex_thread_id: &str) -> bool {
    if codex_thread_id.trim().is_empty() {
        return false;
    }
    db.has_open_thread_for_codex_thread_id(codex_thread_id)
        .ok()
        .unwrap_or(false)
}

fn resolve_workspace_path(
    db: &AppDb,
    codex_thread_id: Option<&str>,
    explicit: Option<String>,
    fallback: Option<String>,
) -> Option<String> {
    if let Some(path) = explicit.filter(|value| !value.trim().is_empty()) {
        return Some(path);
    }
    if let Some(codex_thread_id) = codex_thread_id {
        if let Ok(Some(path)) = db.workspace_path_for_codex_thread(codex_thread_id) {
            return Some(path);
        }
    }
    fallback.filter(|value| !value.trim().is_empty())
}

fn parse_persisted_layout(raw: &str) -> Option<PersistedLayout> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let focused_pane_id = parsed
        .get("focusedPaneId")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let panes = parsed
        .get("panes")
        .and_then(Value::as_array)
        .map(|raw_panes| {
            raw_panes
                .iter()
                .filter_map(|pane| {
                    let id = pane.get("id").and_then(Value::as_u64)?;
                    let codex_thread_id = pane
                        .get("codexThreadId")
                        .and_then(Value::as_str)
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty());
                    let workspace_path = pane
                        .get("workspacePath")
                        .and_then(Value::as_str)
                        .map(|value| value.to_string())
                        .filter(|value| !value.trim().is_empty());
                    let tab = pane
                        .get("tab")
                        .and_then(Value::as_str)
                        .map(normalize_tab_name)
                        .unwrap_or_else(|| "chat".to_string());
                    let column = pane
                        .get("column")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize);
                    let row = pane
                        .get("row")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize);
                    Some(PersistedPane {
                        id,
                        codex_thread_id,
                        workspace_path,
                        tab,
                        column,
                        row,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let columns = parsed.get("columns").and_then(Value::as_array).map(|cols| {
        cols.iter()
            .filter_map(|col| {
                let ids = col
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_u64)
                    .collect::<Vec<_>>();
                if ids.is_empty() { None } else { Some(ids) }
            })
            .collect::<Vec<_>>()
    });
    Some(PersistedLayout {
        focused_pane_id,
        panes,
        columns,
    })
}

fn serialize_persisted_layout(
    panes: &[PaneUi],
    columns: &[Vec<u64>],
    focused_pane_id: u64,
) -> String {
    let mut pos = std::collections::HashMap::new();
    for (col_idx, col) in columns.iter().enumerate() {
        for (row_idx, pane_id) in col.iter().enumerate() {
            pos.insert(*pane_id, (col_idx, row_idx));
        }
    }
    let persisted_panes: Vec<PersistedPane> = panes
        .iter()
        .map(|pane| {
            let (column, row) = pos.get(&pane.id).copied().unwrap_or((0, 0));
            PersistedPane {
                id: pane.id,
                codex_thread_id: pane.active_codex_thread_id.borrow().clone(),
                workspace_path: pane.active_workspace_path.borrow().clone(),
                tab: selected_tab_name(&pane.stack),
                column: Some(column),
                row: Some(row),
            }
        })
        .collect();
    serialize_persisted_layout_state(&persisted_panes, columns, focused_pane_id)
}

fn serialize_persisted_layout_state(
    panes: &[PersistedPane],
    columns: &[Vec<u64>],
    focused_pane_id: u64,
) -> String {
    let serialized_panes: Vec<Value> = panes
        .iter()
        .map(|pane| {
            json!({
                "id": pane.id,
                "codexThreadId": pane.codex_thread_id,
                "workspacePath": pane.workspace_path,
                "tab": normalize_tab_name(&pane.tab),
                "column": pane.column.unwrap_or(0),
                "row": pane.row.unwrap_or(0),
            })
        })
        .collect();
    let serialized_columns: Vec<Value> = columns
        .iter()
        .map(|col| Value::Array(col.iter().map(|id| Value::from(*id)).collect()))
        .collect();
    json!({
        "version": 1,
        "focusedPaneId": focused_pane_id,
        "panes": serialized_panes,
        "columns": serialized_columns,
    })
    .to_string()
}

fn normalize_columns_for_ids(mut columns: Vec<Vec<u64>>, pane_ids: &[u64]) -> Vec<Vec<u64>> {
    let pane_set: HashSet<u64> = pane_ids.iter().copied().collect();
    let mut seen = HashSet::new();
    for col in &mut columns {
        col.retain(|id| pane_set.contains(id) && seen.insert(*id));
        if col.len() > 2 {
            col.truncate(2);
        }
    }
    columns.retain(|col| !col.is_empty());
    for pane_id in pane_ids {
        if !seen.contains(pane_id) {
            columns.push(vec![*pane_id]);
            seen.insert(*pane_id);
        }
    }
    if columns.is_empty() && !pane_ids.is_empty() {
        columns.push(vec![pane_ids[0]]);
    }
    columns
}

fn columns_from_persisted_panes(panes: &[PersistedPane]) -> Vec<Vec<u64>> {
    let mut rows = panes
        .iter()
        .map(|pane| {
            (
                pane.column.unwrap_or(usize::MAX),
                pane.row.unwrap_or(usize::MAX),
                pane.id,
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(col, row, _)| (*col, *row));
    let mut cols: Vec<Vec<u64>> = Vec::new();
    let mut current_col = usize::MAX;
    for (col, _, pane_id) in rows {
        if cols.is_empty() || col != current_col {
            cols.push(Vec::new());
            current_col = col;
        }
        let last = cols.last_mut().expect("column present");
        if last.len() < 2 {
            last.push(pane_id);
        }
    }
    cols
}

fn load_initial_layout(
    db: &AppDb,
    fallback_thread: Option<String>,
    fallback_workspace: Option<String>,
) -> (Vec<PersistedPane>, Vec<Vec<u64>>, u64, Option<String>) {
    let raw_saved = db.get_setting(SETTING_PANE_LAYOUT_V1).ok().flatten();
    let parsed = raw_saved
        .as_deref()
        .and_then(parse_persisted_layout)
        .unwrap_or(PersistedLayout {
            focused_pane_id: 1,
            panes: Vec::new(),
            columns: None,
        });
    let PersistedLayout {
        focused_pane_id: parsed_focused,
        panes: parsed_panes,
        columns: parsed_columns,
    } = parsed;

    let mut seen_threads = HashSet::new();
    let mut panes = Vec::new();
    for pane in parsed_panes {
        let thread_id = pane
            .codex_thread_id
            .filter(|thread_id| codex_thread_exists(db, thread_id));
        if let Some(thread_id) = thread_id {
            if !seen_threads.insert(thread_id.clone()) {
                continue;
            }
            let workspace = resolve_workspace_path(
                db,
                Some(thread_id.as_str()),
                pane.workspace_path,
                fallback_workspace.clone(),
            );
            panes.push(PersistedPane {
                id: pane.id,
                codex_thread_id: Some(thread_id),
                workspace_path: workspace,
                tab: normalize_tab_name(&pane.tab),
                column: pane.column,
                row: pane.row,
            });
        }
    }

    if panes.is_empty() {
        let thread_id = fallback_thread
            .clone()
            .filter(|thread_id| codex_thread_exists(db, thread_id))
            .or_else(|| fallback_thread.filter(|value| !value.trim().is_empty()));
        panes.push(PersistedPane {
            id: 1,
            codex_thread_id: thread_id,
            workspace_path: fallback_workspace,
            tab: "chat".to_string(),
            column: Some(0),
            row: Some(0),
        });
    }

    let focused = if panes.iter().any(|pane| pane.id == parsed_focused) {
        parsed_focused
    } else {
        panes[0].id
    };

    let pane_ids: Vec<u64> = panes.iter().map(|pane| pane.id).collect();
    let derived_cols = parsed_columns.unwrap_or_else(|| columns_from_persisted_panes(&panes));
    let columns = normalize_columns_for_ids(derived_cols, &pane_ids);

    (panes, columns, focused, raw_saved)
}

fn pane_position(columns: &[Vec<u64>], pane_id: u64) -> Option<(usize, usize)> {
    columns.iter().enumerate().find_map(|(col_idx, col)| {
        col.iter()
            .position(|id| *id == pane_id)
            .map(|row| (col_idx, row))
    })
}

fn build_pane_ui(
    pane_id: u64,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<CodexAppServer>>,
    active_codex_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> Option<PaneUi> {
    let pane_shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane_shell.add_css_class("multi-chat-pane-shell");
    pane_shell.set_vexpand(true);
    pane_shell.set_hexpand(false);

    let pane_root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    pane_root.set_widget_name(&format!("multi-chat-pane-{}", pane_id));
    pane_root.add_css_class("multi-chat-pane");
    pane_root.set_vexpand(true);
    pane_root.set_hexpand(false);
    pane_root.set_size_request(460, -1);

    let header = gtk::CenterBox::new();
    header.add_css_class("multi-chat-pane-header");
    header.set_hexpand(true);
    header.set_margin_start(6);
    header.set_margin_end(6);
    header.set_margin_top(6);
    header.set_margin_bottom(0);

    let meta = gtk::Box::new(gtk::Orientation::Vertical, 0);
    meta.add_css_class("multi-chat-pane-meta");
    meta.set_hexpand(false);
    meta.set_halign(gtk::Align::Start);
    let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    title_row.set_hexpand(true);
    title_row.set_halign(gtk::Align::Start);
    title_row.set_valign(gtk::Align::Center);
    let profile_icon = gtk::Image::from_icon_name("person-symbolic");
    profile_icon.set_pixel_size(11);
    profile_icon.add_css_class("thread-profile-icon");
    profile_icon.set_visible(false);
    title_row.append(&profile_icon);
    let title_label = gtk::Label::new(Some("New thread"));
    title_label.add_css_class("multi-chat-pane-thread-title");
    title_label.set_xalign(0.0);
    title_label.set_hexpand(true);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title_row.append(&title_label);
    let workspace_label = gtk::Label::new(Some("No workspace"));
    workspace_label.add_css_class("multi-chat-pane-workspace-name");
    workspace_label.set_xalign(0.0);
    workspace_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    meta.append(&title_row);
    meta.append(&workspace_label);
    header.set_start_widget(Some(&meta));

    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tabs.add_css_class("top-tabs");
    tabs.set_halign(gtk::Align::Center);
    tabs.set_hexpand(false);

    let chat_tab = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    chat_tab.add_css_class("top-tab");
    chat_tab.set_halign(gtk::Align::Center);
    chat_tab.set_valign(gtk::Align::Center);
    chat_tab.set_can_focus(false);
    let chat_content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    chat_content.append(&gtk::Image::from_icon_name("chat-new-symbolic"));
    chat_content.append(&gtk::Label::new(Some("Chat")));
    chat_tab.append(&chat_content);

    let git_tab_btn = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    git_tab_btn.add_css_class("top-tab");
    git_tab_btn.set_halign(gtk::Align::Center);
    git_tab_btn.set_valign(gtk::Align::Center);
    git_tab_btn.set_can_focus(false);
    let git_content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    git_content.append(&gtk::Image::from_icon_name("git-symbolic"));
    git_content.append(&gtk::Label::new(Some("Git")));
    git_tab_btn.append(&git_content);

    let files_tab = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    files_tab.add_css_class("top-tab");
    files_tab.set_halign(gtk::Align::Center);
    files_tab.set_valign(gtk::Align::Center);
    files_tab.set_can_focus(false);
    let files_content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    files_content.append(&gtk::Image::from_icon_name("folder-silhouette-symbolic"));
    files_content.append(&gtk::Label::new(Some("Files")));
    files_tab.append(&files_content);
    let buttons = vec![
        chat_tab.clone().upcast::<gtk::Widget>(),
        git_tab_btn.clone().upcast::<gtk::Widget>(),
        files_tab.clone().upcast::<gtk::Widget>(),
    ];
    set_tab_active(&buttons, 0);
    tabs.append(&chat_tab);
    let sep1 = gtk::Label::new(Some("|"));
    sep1.add_css_class("tab-separator");
    tabs.append(&sep1);
    tabs.append(&git_tab_btn);
    let sep2 = gtk::Label::new(Some("|"));
    sep2.add_css_class("tab-separator");
    tabs.append(&sep2);
    tabs.append(&files_tab);
    header.set_center_widget(Some(&tabs));

    let close_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    close_button.add_css_class("multi-chat-pane-close");
    close_button.set_can_focus(false);
    close_button.set_widget_name("multi-chat-pane-close");
    let close_icon = gtk::Image::from_icon_name("x-symbolic");
    close_icon.set_pixel_size(14);
    close_icon.add_css_class("multi-chat-pane-close-icon");
    close_icon.set_widget_name("multi-chat-pane-close-icon");
    close_button.append(&close_icon);
    close_button.set_halign(gtk::Align::End);
    close_button.set_visible(false);
    let actions_button =
        actions_menu::build_actions_button(db.clone(), active_workspace_path.clone(), true);
    actions_button.set_halign(gtk::Align::End);
    actions_button.set_valign(gtk::Align::Center);
    let skills_mcp_button = skills_mcp_menu::build_skills_mcp_button(
        db.clone(),
        manager.clone(),
        active_workspace_path.clone(),
        true,
    );
    skills_mcp_button.set_halign(gtk::Align::End);
    skills_mcp_button.set_valign(gtk::Align::Center);

    let header_end = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    header_end.add_css_class("multi-chat-pane-header-end");
    header_end.set_halign(gtk::Align::End);
    header_end.append(&skills_mcp_button);
    header_end.append(&actions_button);
    header_end.append(&close_button);
    header.set_end_widget(Some(&header_end));

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);

    let top_drop_indicator = gtk::Box::new(gtk::Orientation::Vertical, 0);
    top_drop_indicator.add_css_class("multi-chat-pane-edge-indicator");
    top_drop_indicator.add_css_class("multi-chat-pane-edge-indicator-top");
    top_drop_indicator.set_halign(gtk::Align::Fill);
    top_drop_indicator.set_hexpand(true);

    let bottom_drop_indicator = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bottom_drop_indicator.add_css_class("multi-chat-pane-edge-indicator");
    bottom_drop_indicator.add_css_class("multi-chat-pane-edge-indicator-bottom");
    bottom_drop_indicator.set_halign(gtk::Align::Fill);
    bottom_drop_indicator.set_hexpand(true);

    let chat_pane = chat::build_chat_pane_without_composer(
        db.clone(),
        manager.clone(),
        codex.clone(),
        active_codex_thread_id.clone(),
        active_workspace_path.clone(),
    )?;
    stack.add_named(&chat_pane.root, Some("chat"));
    stack.add_named(
        &git_tab::build_git_tab(db.clone(), active_workspace_path.clone()),
        Some("git"),
    );
    stack.add_named(
        &file_browser::build_files_tab(db, active_workspace_path.clone()),
        Some("files"),
    );
    stack.set_visible_child_name("chat");

    {
        let stack = stack.clone();
        let buttons = buttons.clone();
        let click = gtk::GestureClick::builder()
            .button(1)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        click.connect_released(move |_, _, _, _| {
            stack.set_visible_child_name("chat");
            set_tab_active(&buttons, 0);
        });
        chat_tab.add_controller(click);
    }
    {
        let stack = stack.clone();
        let buttons = buttons.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            stack.set_visible_child_name("git");
            set_tab_active(&buttons, 1);
        });
        git_tab_btn.add_controller(click);
    }
    {
        let stack = stack.clone();
        let buttons = buttons.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            stack.set_visible_child_name("files");
            set_tab_active(&buttons, 2);
        });
        files_tab.add_controller(click);
    }

    pane_root.append(&header);
    pane_root.append(&stack);
    pane_shell.append(&top_drop_indicator);
    pane_shell.append(&pane_root);
    pane_shell.append(&bottom_drop_indicator);

    Some(PaneUi {
        id: pane_id,
        shell: pane_shell,
        root: pane_root,
        header,
        profile_icon,
        title_label,
        workspace_label,
        stack,
        tab_buttons: buttons,
        close_button,
        active_codex_thread_id,
        active_workspace_path,
        chat: chat_pane,
    })
}

fn attach_pane_handlers(
    pane: &PaneUi,
    pane_id: u64,
    db: Rc<AppDb>,
    focused_pane_id: Rc<RefCell<u64>>,
    dragging_pane_id: Rc<RefCell<Option<u64>>>,
    clear_drop_markers: Rc<dyn Fn()>,
    set_insert_target: Rc<dyn Fn(Option<InsertTarget>)>,
    can_vertical_drop: Rc<dyn Fn(u64, u64) -> bool>,
    can_vertical_thread_drop: Rc<dyn Fn(u64) -> bool>,
    thread_drop_handler: Rc<
        RefCell<Option<Rc<dyn Fn(Option<String>, Option<String>, InsertTarget)>>>,
    >,
    set_reorder_drag_active: Rc<dyn Fn(bool)>,
    move_pane_horizontal: Rc<dyn Fn(u64, u64, bool)>,
    move_pane_vertical: Rc<dyn Fn(u64, u64, bool)>,
    apply_focus_styles: Rc<dyn Fn()>,
    rebuild_shared_composer: Rc<dyn Fn()>,
    sync_global_active: Rc<dyn Fn()>,
    persist_layout: Rc<dyn Fn()>,
    close_pane: Rc<dyn Fn(u64)>,
) {
    {
        let focused_pane_id = focused_pane_id.clone();
        let apply_focus_styles = apply_focus_styles.clone();
        let rebuild_shared_composer = rebuild_shared_composer.clone();
        let sync_global_active = sync_global_active.clone();
        let persist_layout = persist_layout.clone();
        let click = gtk::GestureClick::builder()
            .button(1)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        click.connect_pressed(move |_, _, _, _| {
            if *focused_pane_id.borrow() == pane_id {
                return;
            }
            let focused_pane_id = focused_pane_id.clone();
            let apply_focus_styles = apply_focus_styles.clone();
            let rebuild_shared_composer = rebuild_shared_composer.clone();
            let sync_global_active = sync_global_active.clone();
            let persist_layout = persist_layout.clone();
            gtk::glib::idle_add_local_once(move || {
                if *focused_pane_id.borrow() == pane_id {
                    return;
                }
                focused_pane_id.replace(pane_id);
                apply_focus_styles();
                rebuild_shared_composer();
                sync_global_active();
                persist_layout();
            });
        });
        pane.shell.add_controller(click);
    }

    {
        let close_pane = close_pane.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| close_pane(pane_id));
        pane.close_button.add_controller(click);
    }

    {
        let persist_layout = persist_layout.clone();
        pane.stack
            .connect_visible_child_name_notify(move |_| persist_layout());
    }

    {
        let dragging_pane_id_begin = dragging_pane_id.clone();
        let clear_drop_markers = clear_drop_markers.clone();
        let set_insert_target = set_insert_target.clone();
        let set_reorder_drag_active_begin = set_reorder_drag_active.clone();
        let db = db.clone();
        let pane_for_drag_begin = pane.clone();
        let drag_source = gtk::DragSource::builder()
            .actions(gtk::gdk::DragAction::MOVE)
            .build();
        drag_source.connect_drag_begin(move |source, _| {
            dragging_pane_id_begin.replace(Some(pane_id));
            pane_for_drag_begin
                .root
                .add_css_class("multi-chat-pane-dragging");
            set_reorder_drag_active_begin(true);

            let label_text = pane_for_drag_begin
                .active_codex_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| {
                    db.get_thread_record_by_codex_thread_id(thread_id)
                        .ok()
                        .flatten()
                        .map(|thread| thread.title)
                })
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| format!("Pane {pane_id}"));

            let icon = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            icon.add_css_class("thread-drag-chip");
            icon.add_css_class("multi-chat-pane-drag-chip");
            let title = gtk::Label::new(Some(&label_text));
            title.add_css_class("thread-drag-chip-label");
            title.set_xalign(0.0);
            icon.append(&title);
            let paintable = gtk::WidgetPaintable::new(Some(&icon));
            source.set_icon(Some(&paintable), 12, 10);
        });
        {
            let dragging_pane_id = dragging_pane_id.clone();
            let pane = pane.clone();
            let clear_drop_markers = clear_drop_markers.clone();
            let set_insert_target = set_insert_target.clone();
            let set_reorder_drag_active_end = set_reorder_drag_active.clone();
            drag_source.connect_drag_end(move |_, _, _| {
                dragging_pane_id.replace(None);
                pane.root.remove_css_class("multi-chat-pane-dragging");
                clear_drop_markers();
                set_insert_target(None);
                set_reorder_drag_active_end(false);
            });
        }
        drag_source.connect_prepare(move |_, _, _| {
            let payload = json!({
                "kind": "paneReorder",
                "paneId": pane_id,
            })
            .to_string();
            Some(gtk::gdk::ContentProvider::for_value(&payload.to_value()))
        });
        pane.header.add_controller(drag_source);
    }

    {
        let dragging_pane_id = dragging_pane_id.clone();
        let clear_drop_markers = clear_drop_markers.clone();
        let set_insert_target = set_insert_target.clone();
        let can_vertical_drop = can_vertical_drop.clone();
        let set_reorder_drag_active = set_reorder_drag_active.clone();
        let move_pane_horizontal = move_pane_horizontal.clone();
        let move_pane_vertical = move_pane_vertical.clone();
        let pane = pane.clone();
        let pane_root_for_drop = pane.root.clone();
        let drop_target = gtk::DropTarget::new(
            String::static_type(),
            gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY,
        );
        {
            let pane = pane.clone();
            let dragging_pane_id = dragging_pane_id.clone();
            let clear_drop_markers = clear_drop_markers.clone();
            let set_insert_target = set_insert_target.clone();
            let can_vertical_drop = can_vertical_drop.clone();
            let can_vertical_thread_drop = can_vertical_thread_drop.clone();
            drop_target.connect_enter(move |_, x, y| {
                clear_drop_markers();
                let width = pane.root.width().max(1) as f64;
                let height = pane.root.height().max(1) as f64;
                let vertical_zone = (height * 0.32).clamp(72.0, 220.0);
                let vertical_ok = if let Some(dragged) = *dragging_pane_id.borrow() {
                    if dragged == pane_id {
                        set_insert_target(None);
                        return gtk::gdk::DragAction::empty();
                    }
                    can_vertical_drop(dragged, pane_id)
                } else {
                    can_vertical_thread_drop(pane_id)
                };
                let target = if vertical_ok && y <= vertical_zone {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: false,
                    }
                } else if vertical_ok && y >= (height - vertical_zone) {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: true,
                    }
                } else {
                    InsertTarget::Horizontal {
                        target_pane_id: pane_id,
                        after: x > (width / 2.0),
                    }
                };
                set_insert_target(Some(target));
                gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY
            });
        }
        {
            let pane = pane.clone();
            let dragging_pane_id = dragging_pane_id.clone();
            let clear_drop_markers = clear_drop_markers.clone();
            let set_insert_target = set_insert_target.clone();
            let can_vertical_drop = can_vertical_drop.clone();
            let can_vertical_thread_drop = can_vertical_thread_drop.clone();
            drop_target.connect_motion(move |_, x, y| {
                clear_drop_markers();
                let width = pane.root.width().max(1) as f64;
                let height = pane.root.height().max(1) as f64;
                let vertical_zone = (height * 0.32).clamp(72.0, 220.0);
                let vertical_ok = if let Some(dragged) = *dragging_pane_id.borrow() {
                    if dragged == pane_id {
                        set_insert_target(None);
                        return gtk::gdk::DragAction::empty();
                    }
                    can_vertical_drop(dragged, pane_id)
                } else {
                    can_vertical_thread_drop(pane_id)
                };
                let target = if vertical_ok && y <= vertical_zone {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: false,
                    }
                } else if vertical_ok && y >= (height - vertical_zone) {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: true,
                    }
                } else {
                    InsertTarget::Horizontal {
                        target_pane_id: pane_id,
                        after: x > (width / 2.0),
                    }
                };
                set_insert_target(Some(target));
                gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY
            });
        }
        {
            let clear_drop_markers = clear_drop_markers.clone();
            let set_insert_target = set_insert_target.clone();
            drop_target.connect_leave(move |_| {
                clear_drop_markers();
                set_insert_target(None);
            });
        }
        let can_vertical_drop = can_vertical_drop.clone();
        let can_vertical_thread_drop = can_vertical_thread_drop.clone();
        let thread_drop_handler = thread_drop_handler.clone();
        drop_target.connect_drop(move |_, value, x, y| {
            let Ok(raw) = value.get::<String>() else {
                clear_drop_markers();
                set_insert_target(None);
                set_reorder_drag_active(false);
                return false;
            };
            let width = pane_root_for_drop.width().max(1) as f64;
            let height = pane_root_for_drop.height().max(1) as f64;
            let vertical_zone = (height * 0.32).clamp(72.0, 220.0);
            if let Some(dragged_pane_id) = parse_pane_reorder_payload(&raw) {
                if dragged_pane_id == pane_id {
                    clear_drop_markers();
                    set_insert_target(None);
                    set_reorder_drag_active(false);
                    return false;
                }
                let vertical_ok = can_vertical_drop(dragged_pane_id, pane_id);
                let target = if vertical_ok && y <= vertical_zone {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: false,
                    }
                } else if vertical_ok && y >= (height - vertical_zone) {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: true,
                    }
                } else {
                    InsertTarget::Horizontal {
                        target_pane_id: pane_id,
                        after: x > (width / 2.0),
                    }
                };
                let move_pane_horizontal = move_pane_horizontal.clone();
                let move_pane_vertical = move_pane_vertical.clone();
                gtk::glib::idle_add_local_once(move || match target {
                    InsertTarget::Vertical {
                        target_pane_id,
                        below,
                    } => move_pane_vertical(dragged_pane_id, target_pane_id, below),
                    InsertTarget::Horizontal {
                        target_pane_id,
                        after,
                    } => move_pane_horizontal(dragged_pane_id, target_pane_id, after),
                });
            } else if let Some((codex_thread, workspace_path)) = parse_thread_drop_payload(&raw) {
                let vertical_ok = can_vertical_thread_drop(pane_id);
                let target = if vertical_ok && y <= vertical_zone {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: false,
                    }
                } else if vertical_ok && y >= (height - vertical_zone) {
                    InsertTarget::Vertical {
                        target_pane_id: pane_id,
                        below: true,
                    }
                } else {
                    InsertTarget::Horizontal {
                        target_pane_id: pane_id,
                        after: x > (width / 2.0),
                    }
                };
                if let Some(handler) = thread_drop_handler.borrow().clone() {
                    gtk::glib::idle_add_local_once(move || {
                        handler(codex_thread, workspace_path, target);
                    });
                } else {
                    clear_drop_markers();
                    set_insert_target(None);
                    set_reorder_drag_active(false);
                    return false;
                }
            } else {
                clear_drop_markers();
                set_insert_target(None);
                set_reorder_drag_active(false);
                return false;
            }
            dragging_pane_id.replace(None);
            clear_drop_markers();
            set_insert_target(None);
            set_reorder_drag_active(false);
            true
        });
        pane.root.add_controller(drop_target);
    }
}

fn parse_thread_drop_payload(raw: &str) -> Option<(Option<String>, Option<String>)> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let codex_thread_id = parsed
        .get("codexThreadId")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let workspace_path = parsed
        .get("workspacePath")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    Some((codex_thread_id, workspace_path))
}

fn parse_pane_reorder_payload(raw: &str) -> Option<u64> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let kind = parsed.get("kind").and_then(Value::as_str)?;
    if kind != "paneReorder" {
        return None;
    }
    parsed.get("paneId").and_then(Value::as_u64)
}

fn gap_widget_name(left_pane_id: u64, right_pane_id: u64) -> String {
    format!("multi-chat-gap-{}-{}", left_pane_id, right_pane_id)
}

fn edge_gap_widget_name(is_start: bool) -> &'static str {
    if is_start {
        "multi-chat-gap-edge-start"
    } else {
        "multi-chat-gap-edge-end"
    }
}

include!("multi_chat/build_content.rs");

#[cfg(test)]
mod tests {
    use super::{PersistedPane, parse_persisted_layout, serialize_persisted_layout_state};

    #[test]
    fn serialize_and_parse_layout_state_roundtrip() {
        let panes = vec![
            PersistedPane {
                id: 1,
                codex_thread_id: Some("thread-a".to_string()),
                workspace_path: Some("/tmp/ws-a".to_string()),
                tab: "chat".to_string(),
                column: Some(0),
                row: Some(0),
            },
            PersistedPane {
                id: 2,
                codex_thread_id: Some("thread-b".to_string()),
                workspace_path: Some("/tmp/ws-b".to_string()),
                tab: "git".to_string(),
                column: Some(1),
                row: Some(0),
            },
        ];
        let columns = vec![vec![1], vec![2]];
        let raw = serialize_persisted_layout_state(&panes, &columns, 2);
        let parsed = parse_persisted_layout(&raw).expect("layout should parse");
        assert_eq!(parsed.focused_pane_id, 2);
        assert_eq!(parsed.panes.len(), 2);
        assert_eq!(
            parsed.columns.expect("columns should be present"),
            vec![vec![1], vec![2]]
        );
    }

    #[test]
    fn parse_layout_filters_invalid_panes() {
        let raw = r#"{
            "focusedPaneId": 7,
            "panes": [
                {"id": 7, "codexThreadId": "  ", "workspacePath": "", "tab": "files", "column": 0, "row": 0},
                {"codexThreadId": "missing-id"}
            ]
        }"#;
        let parsed = parse_persisted_layout(raw).expect("layout should parse");
        assert_eq!(parsed.focused_pane_id, 7);
        assert_eq!(parsed.panes.len(), 1);
        assert_eq!(parsed.panes[0].id, 7);
        assert_eq!(parsed.panes[0].codex_thread_id, None);
        assert_eq!(parsed.panes[0].workspace_path, None);
    }
}
