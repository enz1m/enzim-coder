use crate::services::app::runtime::RuntimeClient;
use crate::services::app::chat::{AppDb, LocalChatTurnRecord};
use crate::services::app::restore::{RestoreAction, RestoreCheckpoint, RestorePreview};
use crate::ui::widget_tree;
use gtk::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[path = "restore_preview/worker.rs"]
mod worker;
use worker::{
    apply_opencode_restore_worker, apply_restore_worker, undo_opencode_restore_worker,
    undo_restore_worker, ApplyRestoreWorkerOutcome, ChatRestoreWorkerOutcome,
    ThreadSyncOutcome, UndoRestoreWorkerOutcome,
};

#[derive(Clone, Copy)]
enum GitRiskLevel {
    High,
    Info,
}

#[derive(Clone)]
struct RestoreGitRiskSummary {
    high_risk_count: usize,
    info_count: usize,
    recent_actions: Vec<String>,
}

fn workspace_is_git_backed(workspace_path: &str) -> bool {
    crate::services::ops::git::run_git_text(Path::new(workspace_path), &["rev-parse", "--show-toplevel"])
        .map(|output| !output.trim().is_empty())
        .unwrap_or(false)
}

fn backend_kind_for_restore_thread(
    db: &AppDb,
    thread_id: &str,
    client: Option<&Arc<RuntimeClient>>,
) -> String {
    client
        .map(|client| client.backend_kind().to_string())
        .or_else(|| {
            db.get_thread_profile_id_by_remote_thread_id(thread_id)
                .ok()
                .flatten()
                .and_then(|profile_id| db.get_codex_profile(profile_id).ok().flatten())
                .map(|profile| profile.backend_kind)
        })
        .unwrap_or_else(|| "codex".to_string())
}

fn open_confirmation_dialog(
    parent: &gtk::Window,
    title: &str,
    message: &str,
    conflict_paths: &[String],
    on_confirm: Rc<dyn Fn(Vec<String>)>,
) {
    let dialog = gtk::Window::builder()
        .title(title)
        .default_width(420)
        .modal(true)
        .transient_for(parent)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(14);
    root.set_margin_bottom(14);

    let label = gtk::Label::new(Some(message));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_max_width_chars(56);
    label.add_css_class("restore-preview-card-body");
    root.append(&label);

    let selected_force_paths: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    if !conflict_paths.is_empty() {
        let info = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let hint = gtk::Label::new(Some(
            "Force restore only for conflicting files you want to overwrite.",
        ));
        hint.set_xalign(0.0);
        hint.add_css_class("restore-preview-info-text");
        info.append(&hint);

        let conflict_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        for path in conflict_paths {
            let toggle = gtk::CheckButton::with_label(path);
            toggle.add_css_class("restore-preview-info-text");
            let path = path.clone();
            let selected_force_paths = selected_force_paths.clone();
            toggle.connect_toggled(move |button| {
                if button.is_active() {
                    let mut selected = selected_force_paths.borrow_mut();
                    if !selected.iter().any(|existing| existing == &path) {
                        selected.push(path.clone());
                    }
                } else {
                    selected_force_paths
                        .borrow_mut()
                        .retain(|existing| existing != &path);
                }
            });
            conflict_list.append(&toggle);
        }

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_height(120)
            .child(&conflict_list)
            .build();
        scroll.set_has_frame(false);
        info.append(&scroll);
        root.append(&info);
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let cancel = gtk::Button::with_label("Cancel");
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    actions.append(&cancel);

    let confirm = gtk::Button::with_label("Confirm");
    confirm.add_css_class("suggested-action");
    {
        let dialog = dialog.clone();
        let on_confirm = on_confirm.clone();
        let selected_force_paths = selected_force_paths.clone();
        confirm.connect_clicked(move |_| {
            on_confirm(selected_force_paths.borrow().clone());
            dialog.close();
        });
    }
    actions.append(&confirm);

    root.append(&actions);
    dialog.set_child(Some(&root));
    dialog.present();
}

fn action_label(action: &RestoreAction) -> &'static str {
    match action {
        RestoreAction::Noop => "No change",
        RestoreAction::Write => "Restore",
        RestoreAction::Delete => "Delete",
        RestoreAction::Recreate => "Recreate",
    }
}

fn touched_paths_preview(preview: &RestorePreview, limit: usize) -> String {
    let mut paths: Vec<String> = preview
        .items
        .iter()
        .filter(|item| !matches!(item.action, RestoreAction::Noop))
        .map(|item| item.path.clone())
        .collect();
    paths.sort();

    if paths.is_empty() {
        return "(none)".to_string();
    }

    let shown: Vec<String> = paths.iter().take(limit).cloned().collect();
    let mut body = shown
        .iter()
        .map(|path| format!("• {path}"))
        .collect::<Vec<_>>()
        .join("\n");

    if paths.len() > limit {
        body.push_str(&format!("\n• …and {} more", paths.len() - limit));
    }

    body
}

fn create_info_box_with_label(message: &str) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("restore-preview-info");

    let icon = gtk::Image::from_icon_name("info-outline-symbolic");
    icon.set_pixel_size(14);
    icon.add_css_class("restore-preview-info-icon");
    row.append(&icon);

    let label = gtk::Label::new(Some(message));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.add_css_class("restore-preview-info-text");
    row.append(&label);

    (row, label)
}

fn create_info_box(message: &str) -> gtk::Box {
    create_info_box_with_label(message).0
}

fn set_metric_pills(container: &gtk::Box, metrics: &[(&str, usize)]) {
    crate::ui::widget_tree::clear_box_children(container);
    for (label, value) in metrics {
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        pill.add_css_class("restore-preview-stat-pill");

        let value_label = gtk::Label::new(Some(&value.to_string()));
        value_label.add_css_class("restore-preview-stat-value");
        pill.append(&value_label);

        let text_label = gtk::Label::new(Some(label));
        text_label.add_css_class("restore-preview-stat-label");
        pill.append(&text_label);

        container.append(&pill);
    }
}

fn to_workspace_file_path(workspace_path: &str, path: &str) -> std::path::PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        Path::new(workspace_path).join(candidate)
    }
}

fn format_restore_result_text(
    prefix: &str,
    restored_count: usize,
    deleted_count: usize,
    recreated_count: usize,
    skipped_conflicts: usize,
    backup_checkpoint_id: i64,
    extra_status: &str,
) -> String {
    let mut text = format!(
        "{prefix}\nRestored {restored_count}  Deleted {deleted_count}  Recreated {recreated_count}  Skipped conflicts {skipped_conflicts}\nBackup checkpoint {backup_checkpoint_id}"
    );
    if !extra_status.trim().is_empty() {
        text.push('\n');
        text.push_str(extra_status.trim());
    }
    text
}

fn format_apply_confirmation_message(
    touched_count: usize,
    conflict_count: usize,
    touched_list: &str,
    git_risk_note: &str,
) -> String {
    let mut message = format!(
        "This will restore {touched_count} file(s).\nConflicts {conflict_count}\nA backup checkpoint will be created first.\n\nFiles:\n{touched_list}"
    );
    if !git_risk_note.trim().is_empty() {
        message.push_str("\n\nWarning:\n");
        message.push_str(git_risk_note.trim());
    }
    message
}

fn format_undo_confirmation_message(
    touched_count: usize,
    conflict_count: usize,
    touched_list: &str,
) -> String {
    format!(
        "This will undo the last restore and touch {touched_count} file(s).\nConflicts {conflict_count}\nA new backup checkpoint will be created first.\n\nFiles:\n{touched_list}"
    )
}

fn snippet(text: &str, max_chars: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        one_line
    } else {
        let mut out = String::new();
        for ch in one_line.chars().take(max_chars) {
            out.push(ch);
        }
        out.push('…');
        out
    }
}

fn relative_time(unix_ts: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now.saturating_sub(unix_ts.max(0));

    if diff < 60 {
        "now".to_string()
    } else if diff < 3_600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3_600)
    } else if diff < 604_800 {
        format!("{}d ago", diff / 86_400)
    } else {
        format!("{}w ago", diff / 604_800)
    }
}

fn parse_turn_timestamp_opt(turn: &Value) -> Option<i64> {
    for key in ["completedAt", "createdAt"] {
        let Some(raw) = turn.get(key) else {
            continue;
        };
        if let Some(ts) = raw.as_i64() {
            return Some(ts);
        }
        if let Some(s) = raw.as_str() {
            if let Ok(dt) = gtk::glib::DateTime::from_iso8601(s, None) {
                return Some(dt.to_unix());
            }
        }
    }
    None
}

fn classify_git_reflog_summary(summary: &str) -> Option<GitRiskLevel> {
    let lower = summary.to_ascii_lowercase();
    if lower.contains("pull")
        || lower.contains("rebase")
        || lower.contains("merge")
        || lower.contains("checkout")
        || lower.contains("switch")
        || lower.contains("reset")
    {
        return Some(GitRiskLevel::High);
    }
    if lower.contains("fetch") {
        return Some(GitRiskLevel::Info);
    }
    None
}

fn git_actions_since_checkpoint(
    workspace_path: &str,
    checkpoint_created_at: i64,
) -> Option<RestoreGitRiskSummary> {
    let workspace_root = std::path::Path::new(workspace_path);
    let reflog = crate::services::ops::git::run_git_text(
        workspace_root,
        &["reflog", "--date=unix", "--format=%gs|%ct", "-n", "120"],
    )
    .ok()?;
    let mut high_risk_count = 0usize;
    let mut info_count = 0usize;
    let mut recent_actions = Vec::new();

    for line in reflog.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((summary, ts_raw)) = trimmed.rsplit_once('|') else {
            continue;
        };
        let Ok(ts) = ts_raw.trim().parse::<i64>() else {
            continue;
        };
        if ts <= checkpoint_created_at {
            continue;
        }

        let summary = summary.trim();
        let Some(level) = classify_git_reflog_summary(summary) else {
            continue;
        };

        match level {
            GitRiskLevel::High => high_risk_count += 1,
            GitRiskLevel::Info => info_count += 1,
        }
        if recent_actions.len() < 3 {
            recent_actions.push(summary.to_string());
        }
    }

    if high_risk_count == 0 && info_count == 0 {
        None
    } else {
        Some(RestoreGitRiskSummary {
            high_risk_count,
            info_count,
            recent_actions,
        })
    }
}

fn restore_git_risk_message(summary: &RestoreGitRiskSummary) -> String {
    let mut message = format!(
        "Git changed since this checkpoint: {} high-risk action(s), {} fetch action(s).",
        summary.high_risk_count, summary.info_count
    );
    if !summary.recent_actions.is_empty() {
        message.push_str(" Recent: ");
        message.push_str(&summary.recent_actions.join(" • "));
    }
    message
}

fn resolve_checkpoint_turn_record(
    local_turns: &[LocalChatTurnRecord],
    checkpoint_turn_id: &str,
    checkpoint_created_at: i64,
) -> Option<LocalChatTurnRecord> {
    if let Some(turn) = local_turns
        .iter()
        .find(|turn| turn.external_turn_id == checkpoint_turn_id)
    {
        return Some(turn.clone());
    }

    if !checkpoint_turn_id.starts_with("opencode-turn-") {
        return None;
    }

    let checkpoint_ts_ms = if checkpoint_created_at < 1_000_000_000_000 {
        checkpoint_created_at.saturating_mul(1_000)
    } else {
        checkpoint_created_at
    };

    local_turns
        .iter()
        .filter_map(|turn| {
            let ts = turn.completed_at.unwrap_or(turn.created_at);
            let delta = (ts - checkpoint_ts_ms).abs();
            // OpenCode checkpoints are captured immediately after turn completion.
            // Keep the fallback tight so rolled-back checkpoints do not get rebound
            // to some other nearby surviving turn after restore.
            if delta > 30 * 1_000 {
                return None;
            }
            Some((delta, turn.clone()))
        })
        .min_by_key(|(delta, _)| *delta)
        .map(|(_, turn)| turn)
}

fn resolve_visible_restore_checkpoints(
    checkpoints: &[RestoreCheckpoint],
    local_turns: &[LocalChatTurnRecord],
) -> Vec<(RestoreCheckpoint, LocalChatTurnRecord)> {
    let mut ordered_checkpoints = checkpoints.to_vec();
    ordered_checkpoints.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut ordered_turns = local_turns.to_vec();
    ordered_turns.sort_by(|a, b| {
        a.completed_at
            .unwrap_or(a.created_at)
            .cmp(&b.completed_at.unwrap_or(b.created_at))
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.external_turn_id.cmp(&b.external_turn_id))
    });

    let mut turn_cursor = 0usize;
    let mut out = Vec::new();
    for checkpoint in ordered_checkpoints {
        if checkpoint.turn_id.starts_with("opencode-turn-") {
            let checkpoint_ts_ms = if checkpoint.created_at < 1_000_000_000_000 {
                checkpoint.created_at.saturating_mul(1_000)
            } else {
                checkpoint.created_at
            };
            let mut best_match: Option<(usize, i64)> = None;
            for (idx, turn) in ordered_turns.iter().enumerate().skip(turn_cursor) {
                let ts = turn.completed_at.unwrap_or(turn.created_at);
                let delta = (ts - checkpoint_ts_ms).abs();
                if delta > 30 * 1_000 {
                    continue;
                }
                match best_match {
                    Some((_, best_delta)) if delta >= best_delta => {}
                    _ => best_match = Some((idx, delta)),
                }
            }
            if let Some((idx, _)) = best_match {
                out.push((checkpoint, ordered_turns[idx].clone()));
                turn_cursor = idx.saturating_add(1);
            }
            continue;
        }

        if let Some((idx, turn)) = ordered_turns
            .iter()
            .enumerate()
            .skip(turn_cursor)
            .find(|(_, turn)| turn.external_turn_id == checkpoint.turn_id)
        {
            out.push((checkpoint, turn.clone()));
            turn_cursor = idx.saturating_add(1);
        }
    }

    out
}

fn refresh_checkpoint_dropdown_model(
    db: &AppDb,
    client: Option<&Arc<RuntimeClient>>,
    remote_thread_id: &str,
    checkpoint_model: &gtk::StringList,
    checkpoint_map: &Rc<RefCell<Vec<(String, i64, String, i64)>>>,
    checkpoint_turn_records: &Rc<RefCell<HashMap<i64, LocalChatTurnRecord>>>,
    local_turns: &Rc<RefCell<Vec<LocalChatTurnRecord>>>,
    turn_texts: &Rc<RefCell<HashMap<String, (String, String)>>>,
) {
    let latest_local_turns = db
        .list_local_chat_turns_for_remote_thread(remote_thread_id)
        .unwrap_or_default();
    let total_checkpoints =
        crate::services::app::restore::list_checkpoints_for_remote_thread(db, remote_thread_id);
    let visible_pairs =
        resolve_visible_restore_checkpoints(&total_checkpoints, &latest_local_turns);
    let hidden_count = total_checkpoints.len().saturating_sub(visible_pairs.len());

    local_turns.replace(latest_local_turns);
    turn_texts.replace(
        client
            .and_then(|runtime| runtime.thread_read(remote_thread_id, true).ok())
            .map(|thread| extract_turn_texts(&thread))
            .unwrap_or_default(),
    );

    while checkpoint_model.n_items() > 0 {
        checkpoint_model.remove(0);
    }

    let mut checkpoint_rows = Vec::with_capacity(visible_pairs.len());
    let mut checkpoint_turn_map = HashMap::with_capacity(visible_pairs.len());
    let mut visible_pairs = visible_pairs;
    visible_pairs.sort_by(|(a, _), (b, _)| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    for (checkpoint, turn) in visible_pairs {
        let label = format!(
            "Turn {} • {}",
            checkpoint.turn_id,
            relative_time(checkpoint.created_at)
        );
        checkpoint_model.append(&label);
        checkpoint_turn_map.insert(checkpoint.id, turn);
        checkpoint_rows.push((
            label,
            checkpoint.id,
            checkpoint.turn_id.clone(),
            checkpoint.created_at,
        ));
    }
    checkpoint_turn_records.replace(checkpoint_turn_map);
    checkpoint_map.replace(checkpoint_rows);

    if hidden_count > 0 {
        eprintln!(
            "[restore] hid {} stale checkpoint(s) for thread_id={}",
            hidden_count, remote_thread_id
        );
    }
}

fn extract_turn_texts(thread: &Value) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
        return out;
    };

    for turn in turns {
        let Some(turn_id) = turn
            .get("id")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
        else {
            continue;
        };
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };

        let mut user_text = String::new();
        let mut assistant_text = String::new();

        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("userMessage") => {
                    if let Some(content) = item.get("content").and_then(Value::as_array) {
                        for part in content {
                            if part.get("type").and_then(Value::as_str) == Some("text") {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    if !user_text.is_empty() {
                                        user_text.push('\n');
                                    }
                                    user_text.push_str(text);
                                }
                            }
                        }
                    }
                }
                Some("agentMessage") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        if !assistant_text.is_empty() {
                            assistant_text.push('\n');
                        }
                        assistant_text.push_str(text);
                    }
                }
                _ => {}
            }
        }

        out.insert(turn_id, (user_text, assistant_text));
    }

    out
}

fn set_composer_input_text(parent_window: &gtk::Window, text: &str) {
    let root_widget: gtk::Widget = parent_window.clone().upcast();
    let Some(widget) = widget_tree::find_widget_by_name(&root_widget, "composer-input-view") else {
        return;
    };
    let Ok(input_view) = widget.downcast::<gtk::TextView>() else {
        return;
    };

    input_view.buffer().set_text(text);
    input_view.grab_focus();
}

fn apply_synced_thread_view(
    db: &AppDb,
    parent_window: Option<&gtk::Window>,
    thread_id: &str,
    thread: &Value,
) -> Result<(), String> {
    if let Some(parent_window) = parent_window {
        if super::chat::refresh_visible_history_for_thread(db, parent_window, thread_id, thread) {
            return Ok(());
        }
        return Err("failed to refresh visible chat UI.".to_string());
    }

    if super::chat::sync_local_history_for_thread(db, thread_id, thread) {
        return Ok(());
    }

    Err("failed to persist local chat history.".to_string())
}

fn apply_thread_sync_outcome(
    db: &AppDb,
    parent_window: Option<&gtk::Window>,
    active_thread_id: Option<Rc<RefCell<Option<String>>>>,
    thread_id: &str,
    outcome: Option<&ThreadSyncOutcome>,
) -> Result<(), String> {
    let Some(outcome) = outcome else {
        return Ok(());
    };
    if let Some(active) = active_thread_id.as_ref() {
        if active.borrow().as_deref() != Some(thread_id) {
            active.replace(Some(thread_id.to_string()));
        }
    }
    apply_synced_thread_view(db, parent_window, thread_id, &outcome.thread)?;
    if outcome.request_runtime_history_reload {
        super::chat::request_runtime_history_reload(thread_id);
    }
    Ok(())
}

fn set_snapshot_summary_text(
    summary: &gtk::Box,
    restore_count: usize,
    delete_count: usize,
    recreate_count: usize,
    conflict_count: usize,
    selected_count: usize,
    selectable_count: usize,
) {
    set_metric_pills(
        summary,
        &[
            ("Selected", selected_count),
            ("Files", selectable_count),
            ("Restore", restore_count),
            ("Delete", delete_count),
            ("Recreate", recreate_count),
            ("Conflicts", conflict_count),
        ],
    );
}

fn refresh_preview_list(
    preview: Option<RestorePreview>,
    listbox: &gtk::ListBox,
    summary: &gtk::Box,
    _forced_paths: &Rc<RefCell<HashSet<String>>>,
    selected_paths: &Rc<RefCell<HashSet<String>>>,
    workspace_path: &str,
) {
    while let Some(child) = listbox.first_child() {
        listbox.remove(&child);
    }
    selected_paths.borrow_mut().clear();

    let Some(preview) = preview else {
        crate::ui::widget_tree::clear_box_children(summary);
        return;
    };
    let checkpoint_id = preview.target_checkpoint_id;

    let mut restore_count = 0usize;
    let mut delete_count = 0usize;
    let mut recreate_count = 0usize;
    let mut conflict_count = 0usize;
    let mut selectable_count = 0usize;

    for item in preview.items {
        match item.action {
            RestoreAction::Write => restore_count += 1,
            RestoreAction::Delete => delete_count += 1,
            RestoreAction::Recreate => recreate_count += 1,
            RestoreAction::Noop => {}
        }
        if item.conflict {
            conflict_count += 1;
        }
        let selectable = !matches!(item.action, RestoreAction::Noop);
        if selectable {
            selectable_count += 1;
            selected_paths.borrow_mut().insert(item.path.clone());
        }

        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        row.add_css_class("restore-preview-row");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_margin_top(6);
        content.set_margin_bottom(6);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if selectable {
            let select_toggle = gtk::CheckButton::new();
            select_toggle.set_active(true);
            let path = item.path.clone();
            let selected_paths = selected_paths.clone();
            let summary = summary.clone();
            select_toggle.connect_toggled(move |toggle| {
                if toggle.is_active() {
                    selected_paths.borrow_mut().insert(path.clone());
                } else {
                    selected_paths.borrow_mut().remove(path.as_str());
                }
                set_snapshot_summary_text(
                    &summary,
                    restore_count,
                    delete_count,
                    recreate_count,
                    conflict_count,
                    selected_paths.borrow().len(),
                    selectable_count,
                );
            });
            top.append(&select_toggle);
        }

        let path_label = gtk::Label::new(Some(&item.path));
        path_label.set_xalign(0.0);
        path_label.set_hexpand(true);
        path_label.add_css_class("restore-preview-path");
        top.append(&path_label);

        let kind = gtk::Label::new(Some(action_label(&item.action)));
        kind.add_css_class("restore-preview-kind");
        top.append(&kind);

        if selectable {
            let diff_btn = gtk::Button::new();
            diff_btn.set_has_frame(false);
            diff_btn.add_css_class("app-flat-button");
            diff_btn.add_css_class("restore-preview-diff-button");
            diff_btn.set_tooltip_text(Some("Open diff preview"));
            let diff_icon = gtk::Image::from_icon_name("view-dual-symbolic");
            diff_icon.set_pixel_size(14);
            diff_btn.set_child(Some(&diff_icon));
            let preview_path = to_workspace_file_path(workspace_path, &item.path);
            diff_btn.connect_clicked(move |_| {
                crate::ui::components::file_preview::open_checkpoint_diff_preview(
                    &preview_path,
                    checkpoint_id,
                );
            });
            top.append(&diff_btn);
        }

        let reason = gtk::Label::new(Some(&item.reason));
        reason.set_xalign(0.0);
        reason.add_css_class(if item.conflict {
            "restore-preview-reason-conflict"
        } else {
            "restore-preview-reason"
        });

        content.append(&top);
        content.append(&reason);
        row.set_child(Some(&content));
        listbox.append(&row);
    }

    set_snapshot_summary_text(
        summary,
        restore_count,
        delete_count,
        recreate_count,
        conflict_count,
        selected_paths.borrow().len(),
        selectable_count,
    );
}

fn refresh_native_restore_list(
    files: &[String],
    listbox: &gtk::ListBox,
    summary: &gtk::Box,
    has_native_file_restore: bool,
) {
    while let Some(child) = listbox.first_child() {
        listbox.remove(&child);
    }

    if !has_native_file_restore {
        set_metric_pills(summary, &[("Files", 0)]);
        return;
    }

    for path in files {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        row.add_css_class("restore-preview-row");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_margin_top(6);
        content.set_margin_bottom(6);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let path_label = gtk::Label::new(Some(path));
        path_label.set_xalign(0.0);
        path_label.set_hexpand(true);
        path_label.add_css_class("restore-preview-path");
        top.append(&path_label);

        let kind = gtk::Label::new(Some("Restore"));
        kind.add_css_class("restore-preview-kind");
        top.append(&kind);

        let reason = gtk::Label::new(Some("Recorded by OpenCode native patch data"));
        reason.set_xalign(0.0);
        reason.add_css_class("restore-preview-reason");

        content.append(&top);
        content.append(&reason);
        row.set_child(Some(&content));
        listbox.append(&row);
    }

    set_metric_pills(summary, &[("Files", files.len())]);
}

pub fn open_restore_preview_dialog(
    parent: Option<gtk::Window>,
    db: Rc<AppDb>,
    codex: Option<Arc<RuntimeClient>>,
    codex_thread_id: String,
    active_thread_id: Rc<RefCell<Option<String>>>,
    workspace_path: String,
    initial_checkpoint_id: Option<i64>,
) {
    let parent_window = parent.clone();
    let dialog = gtk::Window::builder()
        .title("Restore Preview")
        .default_width(720)
        .default_height(480)
        .modal(true)
        .build();
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(10);
    root.set_margin_end(10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_vexpand(true);

    let heading = gtk::Label::new(Some("Choose a checkpoint to restore"));
    heading.set_xalign(0.0);
    heading.add_css_class("restore-preview-heading");
    root.append(&heading);

    let backend_kind = backend_kind_for_restore_thread(&db, &codex_thread_id, codex.as_ref());
    let native_opencode_available =
        backend_kind.eq_ignore_ascii_case("opencode") && workspace_is_git_backed(&workspace_path);

    let turn_texts: Rc<RefCell<HashMap<String, (String, String)>>> = Rc::new(RefCell::new(
        codex
            .as_ref()
            .and_then(|client| client.thread_read(&codex_thread_id, true).ok())
            .map(|thread| extract_turn_texts(&thread))
            .unwrap_or_default(),
    ));
    let local_turns: Rc<RefCell<Vec<LocalChatTurnRecord>>> = Rc::new(RefCell::new(
        db.list_local_chat_turns_for_remote_thread(&codex_thread_id)
            .unwrap_or_default(),
    ));

    let checkpoint_map: Rc<RefCell<Vec<(String, i64, String, i64)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let checkpoint_turn_records: Rc<RefCell<HashMap<i64, LocalChatTurnRecord>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let checkpoint_model = gtk::StringList::new(&[]);
    refresh_checkpoint_dropdown_model(
        &db,
        codex.as_ref(),
        &codex_thread_id,
        &checkpoint_model,
        &checkpoint_map,
        &checkpoint_turn_records,
        &local_turns,
        &turn_texts,
    );

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let checkpoint_dropdown =
        gtk::DropDown::new(Some(checkpoint_model.clone()), None::<&gtk::Expression>);
    checkpoint_dropdown.add_css_class("restore-preview-checkpoints");
    checkpoint_dropdown.set_hexpand(true);
    controls.append(&checkpoint_dropdown);
    root.append(&controls);

    let context_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    context_box.add_css_class("restore-preview-context");

    let user_card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    user_card.add_css_class("restore-preview-card");
    user_card.set_hexpand(true);
    let user_title = gtk::Label::new(Some("User Prompt"));
    user_title.set_xalign(0.0);
    user_title.add_css_class("restore-preview-card-title");
    let selected_user_preview = gtk::Label::new(Some(""));
    selected_user_preview.set_xalign(0.0);
    selected_user_preview.set_wrap(true);
    selected_user_preview.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    selected_user_preview.add_css_class("restore-preview-card-body");
    user_card.append(&user_title);
    user_card.append(&selected_user_preview);

    let assistant_card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    assistant_card.add_css_class("restore-preview-card");
    assistant_card.set_hexpand(true);
    let assistant_title = gtk::Label::new(Some("Assistant Response"));
    assistant_title.set_xalign(0.0);
    assistant_title.add_css_class("restore-preview-card-title");
    let selected_assistant_preview = gtk::Label::new(Some(""));
    selected_assistant_preview.set_xalign(0.0);
    selected_assistant_preview.set_wrap(true);
    selected_assistant_preview.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    selected_assistant_preview.add_css_class("restore-preview-card-body");
    assistant_card.append(&assistant_title);
    assistant_card.append(&selected_assistant_preview);

    context_box.append(&user_card);
    context_box.append(&assistant_card);
    root.append(&context_box);

    let method_switcher = gtk::StackSwitcher::new();
    let method_stack = gtk::Stack::new();
    method_stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    method_stack.set_transition_duration(160);
    method_switcher.set_stack(Some(&method_stack));
    method_switcher.set_halign(gtk::Align::Start);

    if native_opencode_available {
        root.append(&method_switcher);
    }

    let snapshot_page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    snapshot_page.set_vexpand(true);

    let snapshot_notice = create_info_box(if backend_kind.eq_ignore_ascii_case("opencode") {
        "In-app restore uses Enzim snapshots. It can restore bash and manual edits, but it may also overwrite newer manual changes. For OpenCode threads, chat/tool state is reverted first, then the file snapshot is applied."
    } else {
        "In-app restore uses Enzim snapshots. It can restore full workspace state, but it may also overwrite newer manual changes after the checkpoint."
    });
    snapshot_page.append(&snapshot_notice);

    let summary = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    summary.set_halign(gtk::Align::Start);
    summary.set_hexpand(true);
    summary.add_css_class("restore-preview-summary");
    snapshot_page.append(&summary);

    let git_warning = gtk::Label::new(Some(""));
    git_warning.set_xalign(0.0);
    git_warning.set_wrap(true);
    git_warning.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    git_warning.add_css_class("restore-preview-git-warning");
    git_warning.set_visible(false);
    snapshot_page.append(&git_warning);

    let status = gtk::Label::new(Some(""));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status.add_css_class("dim-label");
    snapshot_page.append(&status);

    let listbox = gtk::ListBox::new();
    listbox.add_css_class("navigation-sidebar");
    listbox.set_selection_mode(gtk::SelectionMode::None);
    listbox.set_activate_on_single_click(false);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&listbox)
        .min_content_height(220)
        .build();
    scroll.set_has_frame(false);
    scroll.set_vexpand(true);
    snapshot_page.append(&scroll);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let conflicts_help = gtk::Label::new(Some(
        "Conflicting files can be force-restored during confirmation.",
    ));
    conflicts_help.set_xalign(0.0);
    conflicts_help.set_hexpand(true);
    conflicts_help.set_wrap(true);
    conflicts_help.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    conflicts_help.add_css_class("dim-label");
    footer.append(&conflicts_help);

    let undo_btn = gtk::Button::with_label("Undo Last Restore");
    undo_btn.set_sensitive(false);
    footer.append(&undo_btn);

    let apply_btn = gtk::Button::with_label("Restore");
    apply_btn.set_sensitive(false);
    footer.append(&apply_btn);
    snapshot_page.append(&footer);

    method_stack.add_titled(&snapshot_page, Some("snapshot"), "In-App Restore");

    let native_status = gtk::Label::new(Some(""));
    let native_restore_btn = gtk::Button::with_label("Restore With OpenCode");
    let native_restore_active: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let (native_summary_box, native_summary) =
        create_info_box_with_label("Pick a checkpoint to inspect OpenCode restore.");
    let native_files_summary = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let native_files_listbox = gtk::ListBox::new();
    native_files_summary.set_halign(gtk::Align::Start);
    native_files_summary.add_css_class("restore-preview-summary");
    native_files_listbox.add_css_class("navigation-sidebar");
    native_files_listbox.set_selection_mode(gtk::SelectionMode::None);
    native_files_listbox.set_activate_on_single_click(false);
    native_restore_btn.set_sensitive(false);

    if native_opencode_available {
        let native_page = gtk::Box::new(gtk::Orientation::Vertical, 10);
        native_page.set_vexpand(true);
        let native_notice = create_info_box(
            "OpenCode restore uses OpenCode's own revert flow and keeps tool/chat state aligned. It restores only patch-recorded file edits, not bash-made edits.",
        );
        native_page.append(&native_notice);
        native_page.append(&native_files_summary);
        native_page.append(&native_summary_box);

        native_status.set_xalign(0.0);
        native_status.set_wrap(true);
        native_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        native_status.add_css_class("dim-label");
        native_page.append(&native_status);

        let native_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&native_files_listbox)
            .min_content_height(220)
            .build();
        native_scroll.set_has_frame(false);
        native_scroll.set_vexpand(true);
        native_page.append(&native_scroll);

        let native_footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let native_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        native_spacer.set_hexpand(true);
        native_footer.append(&native_spacer);
        native_footer.append(&native_restore_btn);
        native_page.append(&native_footer);

        method_stack.add_titled(&native_page, Some("opencode"), "OpenCode Restore");
    }

    root.append(&method_stack);

    let forced_paths: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
    let selected_paths: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
    let selected_checkpoint_id: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(None));
    let selected_turn_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let selected_user_prompt: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let selected_git_risk: Rc<RefCell<Option<RestoreGitRiskSummary>>> = Rc::new(RefCell::new(None));
    let last_backup_checkpoint_id: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(
        crate::services::app::restore::last_backup_checkpoint_for_remote_thread(&db, &codex_thread_id),
    ));
    let reload_checkpoint_choices: Rc<RefCell<Option<Rc<dyn Fn(Option<i64>)>>>> =
        Rc::new(RefCell::new(None));
    if last_backup_checkpoint_id.borrow().is_some() {
        undo_btn.set_sensitive(true);
    }

    {
        let db = db.clone();
        let codex_thread_id = codex_thread_id.clone();
        let checkpoint_map = checkpoint_map.clone();
        let checkpoint_turn_records = checkpoint_turn_records.clone();
        let turn_texts = turn_texts.clone();
        let local_turns = local_turns.clone();
        let listbox = listbox.clone();
        let summary = summary.clone();
        let git_warning = git_warning.clone();
        let status = status.clone();
        let selected_user_preview = selected_user_preview.clone();
        let selected_assistant_preview = selected_assistant_preview.clone();
        let apply_btn = apply_btn.clone();
        let undo_btn = undo_btn.clone();
        let forced_paths = forced_paths.clone();
        let selected_paths = selected_paths.clone();
        let selected_checkpoint_id = selected_checkpoint_id.clone();
        let selected_turn_id = selected_turn_id.clone();
        let selected_user_prompt = selected_user_prompt.clone();
        let selected_git_risk = selected_git_risk.clone();
        let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
        let workspace_path = workspace_path.clone();
        let native_summary = native_summary.clone();
        let native_files_summary = native_files_summary.clone();
        let native_files_listbox = native_files_listbox.clone();
        let native_status = native_status.clone();
        let native_restore_btn = native_restore_btn.clone();
        let native_restore_active = native_restore_active.clone();
        let native_opencode_available = native_opencode_available;
        let reload_checkpoint_choices_cell = reload_checkpoint_choices.clone();

        let db_for_loader = db.clone();
        let codex_for_loader = codex.clone();
        let codex_thread_id_for_loader = codex_thread_id.clone();
        let local_turns_for_loader = local_turns.clone();
        let checkpoint_turn_records_for_loader = checkpoint_turn_records.clone();
        let turn_texts_for_loader = turn_texts.clone();
        let selected_checkpoint_id_for_loader = selected_checkpoint_id.clone();
        let checkpoint_map_for_loader = checkpoint_map.clone();
        let load_selected_preview: Rc<dyn Fn(&gtk::DropDown)> = Rc::new(move |dropdown| {
            let idx = dropdown.selected() as usize;
            let selected_checkpoint_entry = checkpoint_map_for_loader.borrow().get(idx).cloned();
            let Some((_, checkpoint_id, turn_id, created_at)) = selected_checkpoint_entry else {
                selected_checkpoint_id_for_loader.replace(None);
                selected_turn_id.replace(None);
                selected_user_prompt.replace(None);
                selected_git_risk.replace(None);
                apply_btn.set_sensitive(false);
                status.set_text("");
                native_status.set_text("");
                if !*native_restore_active.borrow() {
                    native_restore_btn.set_label("Restore With OpenCode");
                }
                native_summary.set_text("Pick a checkpoint to inspect OpenCode restore.");
                set_metric_pills(&native_files_summary, &[("Files", 0)]);
                refresh_native_restore_list(
                    &[],
                    &native_files_listbox,
                    &native_files_summary,
                    false,
                );
                native_restore_btn.set_sensitive(*native_restore_active.borrow());
                selected_user_preview.set_text("");
                selected_assistant_preview.set_text("");
                git_warning.set_visible(false);
                undo_btn.set_sensitive(last_backup_checkpoint_id.borrow().is_some());
                refresh_preview_list(
                    None,
                    &listbox,
                    &summary,
                    &forced_paths,
                    &selected_paths,
                    &workspace_path,
                );
                return;
            };
            selected_checkpoint_id_for_loader.replace(Some(checkpoint_id));
            status.set_text("");
            native_status.set_text("");
            let risk = git_actions_since_checkpoint(&workspace_path, created_at);
            if let Some(risk_summary) = risk.clone() {
                git_warning.set_text(&restore_git_risk_message(&risk_summary));
                git_warning.set_visible(true);
            } else {
                git_warning.set_visible(false);
            }
            selected_git_risk.replace(risk);
            let stored_turn = checkpoint_turn_records_for_loader
                .borrow()
                .get(&checkpoint_id)
                .cloned();
            let resolved_turn = stored_turn.or_else(|| {
                let local_turns = local_turns_for_loader.borrow();
                resolve_checkpoint_turn_record(&local_turns, &turn_id, created_at)
            });
            if let Some(turn) = resolved_turn.as_ref() {
                eprintln!(
                    "[restore] checkpoint.resolve checkpoint_id={} checkpoint_turn_id={} matched_turn_id={} matched_completed_at={:?}",
                    checkpoint_id, turn_id, turn.external_turn_id, turn.completed_at
                );
                selected_turn_id.replace(Some(turn.external_turn_id.clone()));
                selected_user_prompt.replace(Some(turn.user_text.clone()));
                let user_text = if turn.user_text.trim().is_empty() {
                    "(no prompt text captured)".to_string()
                } else {
                    snippet(&turn.user_text, 360)
                };
                let assistant_text = if turn.assistant_text.trim().is_empty() {
                    "(no response text captured)".to_string()
                } else {
                    snippet(&turn.assistant_text, 360)
                };
                selected_user_preview.set_text(&user_text);
                selected_assistant_preview.set_text(&assistant_text);
            } else if let Some((user, assistant)) =
                turn_texts_for_loader.borrow().get(&turn_id).cloned()
            {
                eprintln!(
                    "[restore] checkpoint.resolve checkpoint_id={} checkpoint_turn_id={} matched_remote_turn_id={} fallback=thread_read",
                    checkpoint_id, turn_id, turn_id
                );
                selected_turn_id.replace(Some(turn_id.clone()));
                selected_user_prompt.replace(Some(user.clone()));
                let user_text = if user.trim().is_empty() {
                    "(no prompt text captured)".to_string()
                } else {
                    snippet(&user, 360)
                };
                let assistant_text = if assistant.trim().is_empty() {
                    "(no response text captured)".to_string()
                } else {
                    snippet(&assistant, 360)
                };
                selected_user_preview.set_text(&user_text);
                selected_assistant_preview.set_text(&assistant_text);
            } else {
                eprintln!(
                    "[restore] checkpoint.resolve checkpoint_id={} checkpoint_turn_id={} matched_turn_id=<none>",
                    checkpoint_id, turn_id
                );
                selected_turn_id.replace(None);
                selected_user_prompt.replace(None);
                selected_user_preview.set_text("(turn text not available)");
                selected_assistant_preview.set_text("(turn text not available)");
            }
            let preview = crate::services::app::restore::preview_restore_to_checkpoint_by_remote_id(
                &db_for_loader,
                &codex_thread_id_for_loader,
                checkpoint_id,
            );
            apply_btn.set_sensitive(preview.is_some());
            let target_turn_id_for_native = selected_turn_id.borrow().clone();
            let native_restore_info = if native_opencode_available {
                target_turn_id_for_native
                    .as_deref()
                    .and_then(|target_turn_id| {
                        codex_for_loader.as_ref().and_then(|client| {
                            client
                                .thread_native_restore_info(
                                    &codex_thread_id_for_loader,
                                    target_turn_id,
                                )
                                .ok()
                        })
                    })
            } else {
                None
            };
            let native_has_file_restore = native_restore_info
                .as_ref()
                .and_then(|info| info.get("hasNativeFileRestore"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let native_patch_files = native_restore_info
                .as_ref()
                .and_then(|info| info.get("patchFiles"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let native_tool_file_change_count = native_restore_info
                .as_ref()
                .and_then(|info| info.get("toolFileChangeCount"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let can_native_restore = target_turn_id_for_native.is_some();
            if !*native_restore_active.borrow() {
                native_restore_btn.set_label("Restore With OpenCode");
                native_restore_btn.set_sensitive(
                    native_opencode_available && preview.is_some() && can_native_restore,
                );
            } else {
                native_restore_btn.set_sensitive(true);
            }
            if preview.is_some() {
                if native_tool_file_change_count > 0 && !native_has_file_restore {
                    if !*native_restore_active.borrow() {
                        native_restore_btn.set_sensitive(false);
                    }
                    native_summary.set_text(
                        "OpenCode did not record native patch data for this checkpoint and later turns in this session. Its native restore would only trim chat/messages, not restore files. Use In-App Restore for file changes.",
                    );
                    refresh_native_restore_list(
                        &[],
                        &native_files_listbox,
                        &native_files_summary,
                        false,
                    );
                } else {
                    native_summary.set_text(&format!(
                        "OpenCode will revert this checkpoint turn and later turns. File changes restored by OpenCode: native patch-backed edits only.",
                    ));
                    refresh_native_restore_list(
                        &native_patch_files,
                        &native_files_listbox,
                        &native_files_summary,
                        native_has_file_restore,
                    );
                }
            } else if native_opencode_available {
                native_summary.set_text(
                    "OpenCode restore is unavailable for this checkpoint because no preview could be loaded.",
                );
                refresh_native_restore_list(
                    &[],
                    &native_files_listbox,
                    &native_files_summary,
                    false,
                );
            }
            undo_btn.set_sensitive(last_backup_checkpoint_id.borrow().is_some());
            refresh_preview_list(
                preview,
                &listbox,
                &summary,
                &forced_paths,
                &selected_paths,
                &workspace_path,
            );
        });

        let load_for_signal = load_selected_preview.clone();
        checkpoint_dropdown.connect_selected_notify(move |dropdown| {
            load_for_signal(dropdown);
        });

        let reload_checkpoint_choices: Rc<dyn Fn(Option<i64>)> = {
            let db = db.clone();
            let codex = codex.clone();
            let codex_thread_id = codex_thread_id.clone();
            let checkpoint_model = checkpoint_model.clone();
            let checkpoint_map = checkpoint_map.clone();
            let checkpoint_turn_records = checkpoint_turn_records.clone();
            let checkpoint_dropdown = checkpoint_dropdown.clone();
            let local_turns = local_turns.clone();
            let turn_texts = turn_texts.clone();
            let load_selected_preview = load_selected_preview.clone();
            let selected_checkpoint_id = selected_checkpoint_id.clone();
            Rc::new(move |preferred_checkpoint_id| {
                refresh_checkpoint_dropdown_model(
                    &db,
                    codex.as_ref(),
                    &codex_thread_id,
                    &checkpoint_model,
                    &checkpoint_map,
                    &checkpoint_turn_records,
                    &local_turns,
                    &turn_texts,
                );

                let preferred_checkpoint_id =
                    preferred_checkpoint_id.or(*selected_checkpoint_id.borrow());
                let next_selected = preferred_checkpoint_id.and_then(|checkpoint_id| {
                    checkpoint_map
                        .borrow()
                        .iter()
                        .enumerate()
                        .find(|(_, (_, current_id, _, _))| *current_id == checkpoint_id)
                        .map(|(idx, _)| idx as u32)
                });
                let selected = next_selected.unwrap_or_else(|| {
                    if checkpoint_map.borrow().is_empty() {
                        gtk::INVALID_LIST_POSITION
                    } else {
                        0
                    }
                });

                if checkpoint_dropdown.selected() != selected {
                    checkpoint_dropdown.set_selected(selected);
                } else {
                    load_selected_preview(&checkpoint_dropdown);
                }
            })
        };

        reload_checkpoint_choices_cell.replace(Some(reload_checkpoint_choices.clone()));
        reload_checkpoint_choices(initial_checkpoint_id);
    }

    {
        let db = db.clone();
        let codex_thread_id = codex_thread_id.clone();
        let dialog = dialog.clone();
        let listbox = listbox.clone();
        let summary = summary.clone();
        let status = status.clone();
        let apply_btn = apply_btn.clone();
        let undo_btn = undo_btn.clone();
        let forced_paths = forced_paths.clone();
        let selected_paths = selected_paths.clone();
        let selected_checkpoint_id = selected_checkpoint_id.clone();
        let selected_turn_id = selected_turn_id.clone();
        let selected_user_prompt = selected_user_prompt.clone();
        let selected_git_risk = selected_git_risk.clone();
        let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
        let codex = codex.clone();
        let active_thread_id = active_thread_id.clone();
        let workspace_path = workspace_path.clone();
        let parent_window = parent_window.clone();
        let reload_checkpoint_choices = reload_checkpoint_choices.clone();
        apply_btn.connect_clicked(move |_| {
            let Some(checkpoint_id) = *selected_checkpoint_id.borrow() else {
                status.set_text("Pick a checkpoint before applying restore.");
                return;
            };
            let selected_turn_id_value = selected_turn_id.borrow().clone();
            let selected_user_prompt_value = selected_user_prompt.borrow().clone();

            let preview = crate::services::app::restore::preview_restore_to_checkpoint_by_remote_id(
                &db,
                &codex_thread_id,
                checkpoint_id,
            );
            let Some(preview) = preview else {
                status.set_text("Unable to load restore preview for confirmation.");
                return;
            };
            let actionable_count = preview
                .items
                .iter()
                .filter(|item| !matches!(item.action, RestoreAction::Noop))
                .count();
            let selected_path_list: Vec<String> =
                selected_paths.borrow().iter().cloned().collect();
            if actionable_count > 0 && selected_path_list.is_empty() {
                status.set_text("Select at least one file to restore.");
                return;
            }
            let touched_count = preview
                .items
                .iter()
                .filter(|item| !matches!(item.action, RestoreAction::Noop))
                .count();
            let conflict_count = preview.items.iter().filter(|item| item.conflict).count();
            let conflict_paths: Vec<String> = preview
                .items
                .iter()
                .filter(|item| item.conflict)
                .map(|item| item.path.clone())
                .collect();
            let touched_list = touched_paths_preview(&preview, 8);
            let git_risk_note = selected_git_risk
                .borrow()
                .as_ref()
                .map(restore_git_risk_message)
                .unwrap_or_default();

            let confirm_message = format_apply_confirmation_message(
                touched_count,
                conflict_count,
                &touched_list,
                &git_risk_note,
            );

            let on_confirm: Rc<dyn Fn(Vec<String>)> = {
                let db = db.clone();
                let codex_thread_id = codex_thread_id.clone();
                let listbox = listbox.clone();
                let summary = summary.clone();
                let status = status.clone();
                let undo_btn = undo_btn.clone();
                let forced_paths_for_refresh = forced_paths.clone();
                let selected_path_list = selected_path_list.clone();
                let selected_paths_for_refresh = selected_paths.clone();
                let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
                let codex = codex.clone();
                let active_thread_id = active_thread_id.clone();
                let workspace_path = workspace_path.clone();
                let selected_turn_id_value = selected_turn_id_value.clone();
                let selected_user_prompt_value = selected_user_prompt_value.clone();
                let parent_window = parent_window.clone();
                let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                Rc::new(move |forced_paths_from_confirm| {
                    eprintln!(
                        "[restore] apply start: codex_thread_id={} checkpoint_id={} selected_turn_id={:?} active_thread={:?}",
                        codex_thread_id,
                        checkpoint_id,
                        selected_turn_id_value,
                        active_thread_id.borrow().clone()
                    );
                    status.set_text("Applying restore...");
                    let (tx, rx) =
                        mpsc::channel::<Result<ApplyRestoreWorkerOutcome, String>>();
                    let codex_thread_id_for_worker = codex_thread_id.clone();
                    let workspace_path_for_worker = workspace_path.clone();
                    let selected_turn_id_for_worker = selected_turn_id_value.clone();
                    let selected_path_list_for_worker = selected_path_list.clone();
                    let codex_for_worker = codex.clone();
                    thread::spawn(move || {
                        let background_db = AppDb::open_default();
                        let result = apply_restore_worker(
                            background_db.as_ref(),
                            codex_for_worker,
                            Some(workspace_path_for_worker.as_str()),
                            &codex_thread_id_for_worker,
                            checkpoint_id,
                            selected_turn_id_for_worker.as_deref(),
                            &selected_path_list_for_worker,
                            &forced_paths_from_confirm,
                        );
                        let _ = tx.send(result);
                    });
                    let db = db.clone();
                    let codex_thread_id = codex_thread_id.clone();
                    let status = status.clone();
                    let undo_btn = undo_btn.clone();
                    let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
                    let listbox = listbox.clone();
                    let summary = summary.clone();
                    let forced_paths_for_refresh = forced_paths_for_refresh.clone();
                    let selected_paths_for_refresh = selected_paths_for_refresh.clone();
                    let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                    let active_thread_id = active_thread_id.clone();
                    let parent_window = parent_window.clone();
                    let workspace_path = workspace_path.clone();
                    let selected_user_prompt_value = selected_user_prompt_value.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                        if status.root().is_none() {
                            return gtk::glib::ControlFlow::Break;
                        }
                        match rx.try_recv() {
                            Ok(Ok(outcome)) => {
                                if let Err(err) = apply_thread_sync_outcome(
                                    &db,
                                    parent_window.as_ref(),
                                    Some(active_thread_id.clone()),
                                    &codex_thread_id,
                                    outcome.thread_sync.as_ref(),
                                ) {
                                    status.set_text(&format!("Restore applied, but {err}"));
                                    return gtk::glib::ControlFlow::Break;
                                }
                                if let (Some(parent_window), Some(prompt)) = (
                                    parent_window.as_ref(),
                                    selected_user_prompt_value.as_deref(),
                                ) {
                                    if !prompt.trim().is_empty() {
                                        set_composer_input_text(parent_window, prompt);
                                    }
                                }
                                let result = outcome.result;
                                last_backup_checkpoint_id.replace(Some(result.backup_checkpoint_id));
                                undo_btn.set_sensitive(true);
                                status.set_text(&format_restore_result_text(
                                    &format!(
                                        "Restore applied to checkpoint {}",
                                        result.target_checkpoint_id
                                    ),
                                    result.restored_count,
                                    result.deleted_count,
                                    result.recreated_count,
                                    result.skipped_conflicts,
                                    result.backup_checkpoint_id,
                                    &outcome.rollback_status,
                                ));
                                if let Some(reload) = reload_checkpoint_choices.borrow().as_ref() {
                                    reload(Some(checkpoint_id));
                                } else {
                                    let preview = crate::services::app::restore::preview_restore_to_checkpoint_by_remote_id(
                                        &db,
                                        &codex_thread_id,
                                        checkpoint_id,
                                    );
                                    refresh_preview_list(
                                        preview,
                                        &listbox,
                                        &summary,
                                        &forced_paths_for_refresh,
                                        &selected_paths_for_refresh,
                                        &workspace_path,
                                    );
                                }
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                status.set_text(&err);
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                status.set_text("Restore apply stopped unexpectedly.");
                                gtk::glib::ControlFlow::Break
                            }
                        }
                    });
                })
            };

            open_confirmation_dialog(
                &dialog,
                "Confirm Restore",
                &confirm_message,
                &conflict_paths,
                on_confirm,
            );
        });
    }

    if native_opencode_available {
        let db = db.clone();
        let codex_thread_id = codex_thread_id.clone();
        let dialog = dialog.clone();
        let native_status = native_status.clone();
        let selected_turn_id = selected_turn_id.clone();
        let selected_user_prompt = selected_user_prompt.clone();
        let native_restore_active = native_restore_active.clone();
        let codex = codex.clone();
        let active_thread_id = active_thread_id.clone();
        let workspace_path = workspace_path.clone();
        let parent_window = parent_window.clone();
        let reload_checkpoint_choices = reload_checkpoint_choices.clone();
        native_restore_btn.clone().connect_clicked(move |_| {
            if *native_restore_active.borrow() {
                let confirm_message = "OpenCode will undo the last native restore for this thread and restore the reverted chat/file state that OpenCode still tracks.";
                let on_confirm: Rc<dyn Fn(Vec<String>)> = {
                    let db = db.clone();
                    let codex_thread_id = codex_thread_id.clone();
                    let native_status = native_status.clone();
                    let native_restore_btn = native_restore_btn.clone();
                    let native_restore_active = native_restore_active.clone();
                    let codex = codex.clone();
                    let workspace_path = workspace_path.clone();
                    let parent_window = parent_window.clone();
                    let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                    Rc::new(move |_| {
                        native_status.set_text("Undoing OpenCode restore...");
                        let (tx, rx) =
                            mpsc::channel::<Result<ChatRestoreWorkerOutcome, String>>();
                        let codex_thread_id_for_worker = codex_thread_id.clone();
                        let workspace_path_for_worker = workspace_path.clone();
                        let codex_for_worker = codex.clone();
                        thread::spawn(move || {
                            let result = undo_opencode_restore_worker(
                                codex_for_worker,
                                Some(workspace_path_for_worker.as_str()),
                                &codex_thread_id_for_worker,
                            );
                            let _ = tx.send(result);
                        });
                        let db = db.clone();
                        let codex_thread_id = codex_thread_id.clone();
                        let native_status = native_status.clone();
                        let native_restore_btn = native_restore_btn.clone();
                        let native_restore_active = native_restore_active.clone();
                        let parent_window = parent_window.clone();
                        let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                        gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                            if native_status.root().is_none() {
                                return gtk::glib::ControlFlow::Break;
                            }
                            match rx.try_recv() {
                                Ok(Ok(outcome)) => {
                                    if let Err(err) = apply_thread_sync_outcome(
                                        &db,
                                        parent_window.as_ref(),
                                        None,
                                        &codex_thread_id,
                                        outcome.thread_sync.as_ref(),
                                    ) {
                                        native_status
                                            .set_text(&format!("OpenCode undo completed, but {err}"));
                                        return gtk::glib::ControlFlow::Break;
                                    }
                                    native_restore_active.replace(false);
                                    native_restore_btn.set_label("Restore With OpenCode");
                                    let detail = outcome
                                        .status_text
                                        .trim()
                                        .trim_start_matches('•')
                                        .trim()
                                        .to_string();
                                    native_status.set_text(if detail.is_empty() {
                                        "OpenCode restore undone."
                                    } else {
                                        &detail
                                    });
                                    if let Some(reload) = reload_checkpoint_choices.borrow().as_ref()
                                    {
                                        reload(None);
                                    }
                                    gtk::glib::ControlFlow::Break
                                }
                                Ok(Err(err)) => {
                                    native_status.set_text(&err);
                                    gtk::glib::ControlFlow::Break
                                }
                                Err(mpsc::TryRecvError::Empty) => {
                                    gtk::glib::ControlFlow::Continue
                                }
                                Err(mpsc::TryRecvError::Disconnected) => {
                                    native_status.set_text(
                                        "OpenCode undo request stopped unexpectedly.",
                                    );
                                    gtk::glib::ControlFlow::Break
                                }
                            }
                        });
                    })
                };

                open_confirmation_dialog(
                    &dialog,
                    "Confirm Undo OpenCode Restore",
                    confirm_message,
                    &[],
                    on_confirm,
                );
                return;
            }

            let Some(selected_turn_id_value) = selected_turn_id.borrow().clone() else {
                native_status.set_text("Pick a checkpoint before applying OpenCode restore.");
                return;
            };
            let selected_user_prompt_value = selected_user_prompt.borrow().clone();
            let confirm_message = "OpenCode will revert the selected checkpoint turn and all later turns.\n\nThis keeps OpenCode's chat/tool state aligned and restores tool-based file edits in git-backed workspaces.\n\nLimitation: file edits made through bash commands will not be restored by OpenCode.";
            let on_confirm: Rc<dyn Fn(Vec<String>)> = {
                let db = db.clone();
                let codex_thread_id = codex_thread_id.clone();
                let native_status = native_status.clone();
                let codex = codex.clone();
                let active_thread_id = active_thread_id.clone();
                let workspace_path = workspace_path.clone();
                let selected_turn_id_value = selected_turn_id_value.clone();
                let selected_user_prompt_value = selected_user_prompt_value.clone();
                let parent_window = parent_window.clone();
                let native_restore_btn = native_restore_btn.clone();
                let native_restore_active = native_restore_active.clone();
                let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                Rc::new(move |_| {
                    native_status.set_text("Applying OpenCode restore...");
                    let (tx, rx) =
                        mpsc::channel::<Result<ChatRestoreWorkerOutcome, String>>();
                    let codex_thread_id_for_worker = codex_thread_id.clone();
                    let workspace_path_for_worker = workspace_path.clone();
                    let selected_turn_id_for_worker = selected_turn_id_value.clone();
                    let codex_for_worker = codex.clone();
                    thread::spawn(move || {
                        let result = apply_opencode_restore_worker(
                            codex_for_worker,
                            Some(workspace_path_for_worker.as_str()),
                            &codex_thread_id_for_worker,
                            Some(selected_turn_id_for_worker.as_str()),
                        );
                        let _ = tx.send(result);
                    });
                    let db = db.clone();
                    let codex_thread_id = codex_thread_id.clone();
                    let active_thread_id = active_thread_id.clone();
                    let native_status = native_status.clone();
                    let native_restore_btn = native_restore_btn.clone();
                    let native_restore_active = native_restore_active.clone();
                    let parent_window = parent_window.clone();
                    let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                    let selected_user_prompt_value = selected_user_prompt_value.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                        if native_status.root().is_none() {
                            return gtk::glib::ControlFlow::Break;
                        }
                        match rx.try_recv() {
                            Ok(Ok(outcome)) => {
                                if let Err(err) = apply_thread_sync_outcome(
                                    &db,
                                    parent_window.as_ref(),
                                    Some(active_thread_id.clone()),
                                    &codex_thread_id,
                                    outcome.thread_sync.as_ref(),
                                ) {
                                    native_status.set_text(&format!(
                                        "OpenCode restore completed, but {err}"
                                    ));
                                    return gtk::glib::ControlFlow::Break;
                                }
                                if let (Some(parent_window), Some(prompt)) = (
                                    parent_window.as_ref(),
                                    selected_user_prompt_value.as_deref(),
                                ) {
                                    if !prompt.trim().is_empty() {
                                        set_composer_input_text(parent_window, prompt);
                                    }
                                }
                                native_restore_active.replace(true);
                                native_restore_btn.set_label("Undo OpenCode Restore");
                                let detail = outcome
                                    .status_text
                                    .trim()
                                    .trim_start_matches('•')
                                    .trim()
                                    .to_string();
                                if detail.is_empty() {
                                    native_status.set_text("OpenCode restore completed.");
                                } else {
                                    native_status.set_text(&detail);
                                }
                                if let Some(reload) = reload_checkpoint_choices.borrow().as_ref() {
                                    reload(None);
                                }
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                native_status.set_text(&err);
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => {
                                gtk::glib::ControlFlow::Continue
                            }
                            Err(mpsc::TryRecvError::Disconnected) => {
                                native_status.set_text(
                                    "OpenCode restore request stopped unexpectedly.",
                                );
                                gtk::glib::ControlFlow::Break
                            }
                        }
                    });
                })
            };

            open_confirmation_dialog(
                &dialog,
                "Confirm OpenCode Restore",
                confirm_message,
                &[],
                on_confirm,
            );
        });
    }

    {
        let db = db.clone();
        let codex_thread_id = codex_thread_id.clone();
        let dialog = dialog.clone();
        let listbox = listbox.clone();
        let summary = summary.clone();
        let status = status.clone();
        let forced_paths = forced_paths.clone();
        let selected_paths = selected_paths.clone();
        let selected_checkpoint_id = selected_checkpoint_id.clone();
        let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
        let checkpoint_dropdown = checkpoint_dropdown.clone();
        let reload_checkpoint_choices = reload_checkpoint_choices.clone();
        let codex = codex.clone();
        let workspace_path = workspace_path.clone();
        let parent_window = parent_window.clone();
        undo_btn.connect_clicked(move |_| {
            let backup_checkpoint_id = last_backup_checkpoint_id.borrow().or_else(|| {
                crate::services::app::restore::last_backup_checkpoint_for_remote_thread(&db, &codex_thread_id)
            });
            let Some(backup_checkpoint_id) = backup_checkpoint_id else {
                status.set_text("No backup checkpoint found for undo.");
                return;
            };

            let preview = crate::services::app::restore::preview_restore_to_checkpoint_by_remote_id(
                &db,
                &codex_thread_id,
                backup_checkpoint_id,
            );
            let Some(preview) = preview else {
                status.set_text("Unable to load backup preview for confirmation.");
                return;
            };
            let touched_count = preview
                .items
                .iter()
                .filter(|item| !matches!(item.action, RestoreAction::Noop))
                .count();
            let conflict_count = preview.items.iter().filter(|item| item.conflict).count();
            let conflict_paths: Vec<String> = preview
                .items
                .iter()
                .filter(|item| item.conflict)
                .map(|item| item.path.clone())
                .collect();
            let touched_list = touched_paths_preview(&preview, 8);
            let confirm_message =
                format_undo_confirmation_message(touched_count, conflict_count, &touched_list);

            let on_confirm: Rc<dyn Fn(Vec<String>)> = {
                let db = db.clone();
                let codex_thread_id = codex_thread_id.clone();
                let listbox = listbox.clone();
                let summary = summary.clone();
                let status = status.clone();
                let forced_paths = forced_paths.clone();
                let selected_paths = selected_paths.clone();
                let selected_checkpoint_id = selected_checkpoint_id.clone();
                let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
                let checkpoint_dropdown = checkpoint_dropdown.clone();
                let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                let codex = codex.clone();
                let workspace_path = workspace_path.clone();
                let parent_window = parent_window.clone();
                Rc::new(move |undo_forced_paths| {
                    status.set_text("Undoing restore...");
                    let (tx, rx) =
                        mpsc::channel::<Result<Option<UndoRestoreWorkerOutcome>, String>>();
                    let codex_thread_id_for_worker = codex_thread_id.clone();
                    let workspace_path_for_worker = workspace_path.clone();
                    let codex_for_worker = codex.clone();
                    thread::spawn(move || {
                        let background_db = AppDb::open_default();
                        let result = undo_restore_worker(
                            background_db.as_ref(),
                            codex_for_worker,
                            Some(workspace_path_for_worker.as_str()),
                            &codex_thread_id_for_worker,
                            backup_checkpoint_id,
                            &undo_forced_paths,
                        );
                        let _ = tx.send(result);
                    });
                    let db = db.clone();
                    let codex_thread_id = codex_thread_id.clone();
                    let status = status.clone();
                    let listbox = listbox.clone();
                    let summary = summary.clone();
                    let forced_paths = forced_paths.clone();
                    let selected_paths = selected_paths.clone();
                    let selected_checkpoint_id = selected_checkpoint_id.clone();
                    let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
                    let checkpoint_dropdown = checkpoint_dropdown.clone();
                    let parent_window = parent_window.clone();
                    let reload_checkpoint_choices = reload_checkpoint_choices.clone();
                    let workspace_path = workspace_path.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                        if status.root().is_none() {
                            return gtk::glib::ControlFlow::Break;
                        }
                        match rx.try_recv() {
                            Ok(Ok(Some(outcome))) => {
                                if let Err(err) = apply_thread_sync_outcome(
                                    &db,
                                    parent_window.as_ref(),
                                    None,
                                    &codex_thread_id,
                                    outcome.thread_sync.as_ref(),
                                ) {
                                    status.set_text(&format!("Undo restore applied, but {err}"));
                                    return gtk::glib::ControlFlow::Break;
                                }
                                let result = outcome.result;
                                last_backup_checkpoint_id.replace(Some(result.backup_checkpoint_id));
                                status.set_text(&format_restore_result_text(
                                    &format!(
                                        "Undo restore applied from backup checkpoint {}",
                                        backup_checkpoint_id
                                    ),
                                    result.restored_count,
                                    result.deleted_count,
                                    result.recreated_count,
                                    result.skipped_conflicts,
                                    result.backup_checkpoint_id,
                                    &outcome.chat_restore_status,
                                ));

                                if let Some(reload) = reload_checkpoint_choices.borrow().as_ref() {
                                    reload(*selected_checkpoint_id.borrow());
                                } else if let Some(current_id) = *selected_checkpoint_id.borrow() {
                                    let preview =
                                        crate::services::app::restore::preview_restore_to_checkpoint_by_remote_id(
                                            &db,
                                            &codex_thread_id,
                                            current_id,
                                        );
                                    refresh_preview_list(
                                        preview,
                                        &listbox,
                                        &summary,
                                        &forced_paths,
                                        &selected_paths,
                                        &workspace_path,
                                    );
                                } else {
                                    let idx = checkpoint_dropdown.selected() as usize;
                                    if let Some(id) =
                                        crate::services::app::restore::list_checkpoints_for_remote_thread(
                                            &db,
                                            &codex_thread_id,
                                        )
                                        .get(idx)
                                        .map(|cp| cp.id)
                                    {
                                        let preview =
                                            crate::services::app::restore::preview_restore_to_checkpoint_by_remote_id(
                                                &db,
                                                &codex_thread_id,
                                                id,
                                            );
                                        refresh_preview_list(
                                            preview,
                                            &listbox,
                                            &summary,
                                            &forced_paths,
                                            &selected_paths,
                                            &workspace_path,
                                        );
                                    }
                                }
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Ok(None)) => {
                                status.set_text("Undo restore unavailable.");
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                status.set_text(&format!("Undo restore failed: {err}"));
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                status.set_text("Undo restore request stopped unexpectedly.");
                                gtk::glib::ControlFlow::Break
                            }
                        }
                    });
                })
            };

            open_confirmation_dialog(
                &dialog,
                "Confirm Undo Restore",
                &confirm_message,
                &conflict_paths,
                on_confirm,
            );
        });
    }

    if checkpoint_map.borrow().is_empty() {
        crate::ui::widget_tree::clear_box_children(&summary);
        set_metric_pills(&native_files_summary, &[("Files", 0)]);
        native_summary.set_text("No restore checkpoints found for this thread yet.");
    }

    dialog.set_child(Some(&root));
    dialog.present();
}
