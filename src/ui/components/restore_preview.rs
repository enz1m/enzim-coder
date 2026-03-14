use crate::codex_appserver::CodexAppServer;
use crate::data::AppDb;
use crate::restore::{RestoreAction, RestorePreview};
use crate::ui::widget_tree;
use gtk::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn open_confirmation_dialog(
    parent: &gtk::Window,
    title: &str,
    message: &str,
    on_confirm: Rc<dyn Fn()>,
) {
    let dialog = gtk::Window::builder()
        .title(title)
        .default_width(460)
        .modal(true)
        .transient_for(parent)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let label = gtk::Label::new(Some(message));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&label);

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
        confirm.connect_clicked(move |_| {
            on_confirm();
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
    let reflog = crate::git_exec::run_git_text(
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

include!("restore_preview/apply_with_chat_sync.rs");
fn refresh_preview_list(
    preview: Option<RestorePreview>,
    listbox: &gtk::ListBox,
    summary: &gtk::Label,
    forced_paths: &Rc<RefCell<HashSet<String>>>,
) {
    while let Some(child) = listbox.first_child() {
        listbox.remove(&child);
    }
    forced_paths.borrow_mut().clear();

    let Some(preview) = preview else {
        summary.set_text("No restore preview available for this checkpoint.");
        return;
    };

    let mut restore_count = 0usize;
    let mut delete_count = 0usize;
    let mut recreate_count = 0usize;
    let mut conflict_count = 0usize;

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

        let row = gtk::ListBoxRow::new();
        row.add_css_class("restore-preview-row");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_margin_top(6);
        content.set_margin_bottom(6);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let path_label = gtk::Label::new(Some(&item.path));
        path_label.set_xalign(0.0);
        path_label.set_hexpand(true);
        path_label.add_css_class("restore-preview-path");
        top.append(&path_label);

        let kind = gtk::Label::new(Some(action_label(&item.action)));
        kind.add_css_class("restore-preview-kind");
        top.append(&kind);

        if item.conflict {
            let force_toggle = gtk::CheckButton::with_label("Force");
            force_toggle.add_css_class("restore-preview-force");
            let path = item.path.clone();
            let forced_paths = forced_paths.clone();
            force_toggle.connect_toggled(move |toggle| {
                if toggle.is_active() {
                    forced_paths.borrow_mut().insert(path.clone());
                } else {
                    forced_paths.borrow_mut().remove(path.as_str());
                }
            });
            top.append(&force_toggle);
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

    summary.set_text(&format!(
        "Restore: {restore_count} • Delete: {delete_count} • Recreate: {recreate_count} • Conflicts: {conflict_count}"
    ));
}

pub fn open_restore_preview_dialog(
    parent: Option<gtk::Window>,
    db: Rc<AppDb>,
    codex: Option<Arc<CodexAppServer>>,
    codex_thread_id: String,
    active_codex_thread_id: Rc<RefCell<Option<String>>>,
    workspace_path: String,
) {
    let parent_window = parent.clone();
    let dialog = gtk::Window::builder()
        .title("Restore Preview")
        .default_width(860)
        .default_height(560)
        .modal(true)
        .build();
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let heading = gtk::Label::new(Some("Select a checkpoint to preview restore changes"));
    heading.set_xalign(0.0);
    heading.add_css_class("restore-preview-heading");
    root.append(&heading);

    let turn_texts: Rc<RefCell<HashMap<String, (String, String)>>> = Rc::new(RefCell::new(
        codex
            .as_ref()
            .and_then(|client| client.thread_read(&codex_thread_id, true).ok())
            .map(|thread| extract_turn_texts(&thread))
            .unwrap_or_default(),
    ));

    let checkpoints = crate::restore::list_checkpoints_for_thread(&db, &codex_thread_id);
    let checkpoint_map: Rc<RefCell<Vec<(String, i64, String, i64)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let checkpoint_model = gtk::StringList::new(&[]);

    for cp in checkpoints {
        let label = format!("Turn {} • {}", cp.turn_id, relative_time(cp.created_at));
        checkpoint_model.append(&label);
        checkpoint_map
            .borrow_mut()
            .push((label, cp.id, cp.turn_id.clone(), cp.created_at));
    }

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let checkpoint_dropdown =
        gtk::DropDown::new(Some(checkpoint_model.clone()), None::<&gtk::Expression>);
    checkpoint_dropdown.add_css_class("restore-preview-checkpoints");
    checkpoint_dropdown.set_hexpand(true);
    controls.append(&checkpoint_dropdown);

    let close_btn = gtk::Button::with_label("Close");
    {
        let dialog = dialog.clone();
        close_btn.connect_clicked(move |_| {
            dialog.close();
        });
    }
    controls.append(&close_btn);
    root.append(&controls);

    let summary = gtk::Label::new(Some(""));
    summary.set_xalign(0.0);
    summary.add_css_class("restore-preview-summary");
    root.append(&summary);

    let git_warning = gtk::Label::new(Some(""));
    git_warning.set_xalign(0.0);
    git_warning.set_wrap(true);
    git_warning.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    git_warning.add_css_class("restore-preview-git-warning");
    git_warning.set_visible(false);
    root.append(&git_warning);

    let status = gtk::Label::new(Some(""));
    status.set_xalign(0.0);
    status.add_css_class("dim-label");
    root.append(&status);

    let context_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    context_box.add_css_class("restore-preview-context");

    let user_card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    user_card.add_css_class("restore-preview-card");
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

    let listbox = gtk::ListBox::new();
    listbox.add_css_class("navigation-sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&listbox)
        .min_content_height(320)
        .build();
    scroll.set_has_frame(false);
    root.append(&scroll);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let conflicts_help = gtk::Label::new(Some(
        "Conflicts are blocked by default. Toggle Force per file for future apply step.",
    ));
    conflicts_help.set_xalign(0.0);
    conflicts_help.set_hexpand(true);
    conflicts_help.add_css_class("dim-label");
    footer.append(&conflicts_help);

    let undo_btn = gtk::Button::with_label("Undo Last Restore");
    undo_btn.set_sensitive(false);
    footer.append(&undo_btn);

    let apply_btn = gtk::Button::with_label("Restore");
    apply_btn.set_sensitive(false);
    footer.append(&apply_btn);
    root.append(&footer);

    let forced_paths: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
    let selected_checkpoint_id: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(None));
    let selected_turn_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let selected_user_prompt: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let selected_git_risk: Rc<RefCell<Option<RestoreGitRiskSummary>>> = Rc::new(RefCell::new(None));
    let last_backup_checkpoint_id: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(
        crate::restore::last_backup_checkpoint_for_thread(&db, &codex_thread_id),
    ));
    if last_backup_checkpoint_id.borrow().is_some() {
        undo_btn.set_sensitive(true);
    }

    {
        let db = db.clone();
        let codex_thread_id = codex_thread_id.clone();
        let checkpoint_map = checkpoint_map.clone();
        let turn_texts = turn_texts.clone();
        let listbox = listbox.clone();
        let summary = summary.clone();
        let git_warning = git_warning.clone();
        let status = status.clone();
        let selected_user_preview = selected_user_preview.clone();
        let selected_assistant_preview = selected_assistant_preview.clone();
        let apply_btn = apply_btn.clone();
        let undo_btn = undo_btn.clone();
        let forced_paths = forced_paths.clone();
        let selected_checkpoint_id = selected_checkpoint_id.clone();
        let selected_turn_id = selected_turn_id.clone();
        let selected_user_prompt = selected_user_prompt.clone();
        let selected_git_risk = selected_git_risk.clone();
        let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
        let workspace_path = workspace_path.clone();

        let load_selected_preview: Rc<dyn Fn(&gtk::DropDown)> = Rc::new(move |dropdown| {
            let idx = dropdown.selected() as usize;
            let Some((_, checkpoint_id, turn_id, created_at)) =
                checkpoint_map.borrow().get(idx).cloned()
            else {
                selected_checkpoint_id.replace(None);
                selected_turn_id.replace(None);
                selected_user_prompt.replace(None);
                selected_git_risk.replace(None);
                apply_btn.set_sensitive(false);
                status.set_text("");
                selected_user_preview.set_text("");
                selected_assistant_preview.set_text("");
                git_warning.set_visible(false);
                undo_btn.set_sensitive(last_backup_checkpoint_id.borrow().is_some());
                refresh_preview_list(None, &listbox, &summary, &forced_paths);
                return;
            };
            selected_checkpoint_id.replace(Some(checkpoint_id));
            selected_turn_id.replace(Some(turn_id.clone()));
            status.set_text("");
            let risk = git_actions_since_checkpoint(&workspace_path, created_at);
            if let Some(risk_summary) = risk.clone() {
                git_warning.set_text(&restore_git_risk_message(&risk_summary));
                git_warning.set_visible(true);
            } else {
                git_warning.set_visible(false);
            }
            selected_git_risk.replace(risk);
            if let Some((user, assistant)) = turn_texts.borrow().get(&turn_id) {
                selected_user_prompt.replace(Some(user.clone()));
                let user_text = if user.trim().is_empty() {
                    "(no prompt text captured)".to_string()
                } else {
                    snippet(user, 360)
                };
                let assistant_text = if assistant.trim().is_empty() {
                    "(no response text captured)".to_string()
                } else {
                    snippet(assistant, 360)
                };
                selected_user_preview.set_text(&user_text);
                selected_assistant_preview.set_text(&assistant_text);
            } else {
                selected_user_prompt.replace(None);
                selected_user_preview.set_text("(turn text not available)");
                selected_assistant_preview.set_text("(turn text not available)");
            }
            let preview =
                crate::restore::preview_restore_to_checkpoint(&db, &codex_thread_id, checkpoint_id);
            apply_btn.set_sensitive(preview.is_some());
            undo_btn.set_sensitive(last_backup_checkpoint_id.borrow().is_some());
            refresh_preview_list(preview, &listbox, &summary, &forced_paths);
        });

        let load_for_signal = load_selected_preview.clone();
        checkpoint_dropdown.connect_selected_notify(move |dropdown| {
            load_for_signal(dropdown);
        });

        load_selected_preview(&checkpoint_dropdown);
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
        let selected_checkpoint_id = selected_checkpoint_id.clone();
        let selected_turn_id = selected_turn_id.clone();
        let selected_user_prompt = selected_user_prompt.clone();
        let selected_git_risk = selected_git_risk.clone();
        let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
        let codex = codex.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let workspace_path = workspace_path.clone();
        let parent_window = parent_window.clone();
        apply_btn.connect_clicked(move |_| {
            let Some(checkpoint_id) = *selected_checkpoint_id.borrow() else {
                status.set_text("Pick a checkpoint before applying restore.");
                return;
            };
            let selected_turn_id_value = selected_turn_id.borrow().clone();
            let selected_user_prompt_value = selected_user_prompt.borrow().clone();

            let preview = crate::restore::preview_restore_to_checkpoint(
                &db,
                &codex_thread_id,
                checkpoint_id,
            );
            let Some(preview) = preview else {
                status.set_text("Unable to load restore preview for confirmation.");
                return;
            };
            let touched_count = preview
                .items
                .iter()
                .filter(|item| !matches!(item.action, RestoreAction::Noop))
                .count();
            let conflict_count = preview.items.iter().filter(|item| item.conflict).count();
            let forced_count = forced_paths.borrow().len();
            let touched_list = touched_paths_preview(&preview, 8);
            let git_risk_note = selected_git_risk
                .borrow()
                .as_ref()
                .map(restore_git_risk_message)
                .unwrap_or_default();

            let mut confirm_message = format!(
                "You are about to restore {touched_count} file(s).\nConflicts detected: {conflict_count}. Forced conflicts selected: {forced_count}.\n\nA backup checkpoint will be created before applying.")
            + &format!("\n\nTouched files:\n{touched_list}");
            if !git_risk_note.is_empty() {
                confirm_message.push_str("\n\nWarning: ");
                confirm_message.push_str(&git_risk_note);
            }

            let on_confirm: Rc<dyn Fn()> = {
                let db = db.clone();
                let codex_thread_id = codex_thread_id.clone();
                let listbox = listbox.clone();
                let summary = summary.clone();
                let status = status.clone();
                let undo_btn = undo_btn.clone();
                let forced_paths = forced_paths.clone();
                let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
                let codex = codex.clone();
                let active_codex_thread_id = active_codex_thread_id.clone();
                let workspace_path = workspace_path.clone();
                let selected_turn_id_value = selected_turn_id_value.clone();
                let selected_user_prompt_value = selected_user_prompt_value.clone();
                let parent_window = parent_window.clone();
                Rc::new(move || {
                    eprintln!(
                        "[restore] apply start: codex_thread_id={} checkpoint_id={} selected_turn_id={:?} active_thread={:?}",
                        codex_thread_id,
                        checkpoint_id,
                        selected_turn_id_value,
                        active_codex_thread_id.borrow().clone()
                    );
                    let forced: Vec<String> = forced_paths.borrow().iter().cloned().collect();
                    match apply_restore_with_chat_sync(
                        &db,
                        codex.clone(),
                        Some(active_codex_thread_id.clone()),
                        Some(&workspace_path),
                        &codex_thread_id,
                        checkpoint_id,
                        selected_turn_id_value.as_deref(),
                        &[],
                        &forced,
                        parent_window.as_ref(),
                        selected_user_prompt_value.as_deref(),
                    ) {
                        Ok((result, rollback_status)) => {
                            last_backup_checkpoint_id.replace(Some(result.backup_checkpoint_id));
                            undo_btn.set_sensitive(true);
                            status.set_text(&format!(
                                "Applied restore to checkpoint {}. Restored: {} • Deleted: {} • Recreated: {} • Skipped conflicts: {} • Backup checkpoint: {}{}",
                                result.target_checkpoint_id,
                                result.restored_count,
                                result.deleted_count,
                                result.recreated_count,
                                result.skipped_conflicts,
                                result.backup_checkpoint_id,
                                rollback_status
                            ));

                            let preview = crate::restore::preview_restore_to_checkpoint(
                                &db,
                                &codex_thread_id,
                                checkpoint_id,
                            );
                            refresh_preview_list(preview, &listbox, &summary, &forced_paths);
                        }
                        Err(err) => {
                            status.set_text(&err);
                        }
                    }
                })
            };

            open_confirmation_dialog(&dialog, "Confirm Restore", &confirm_message, on_confirm);
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
        let selected_checkpoint_id = selected_checkpoint_id.clone();
        let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
        let checkpoint_dropdown = checkpoint_dropdown.clone();
        undo_btn.connect_clicked(move |_| {
            let backup_checkpoint_id = last_backup_checkpoint_id
                .borrow()
                .or_else(|| crate::restore::last_backup_checkpoint_for_thread(&db, &codex_thread_id));
            let Some(backup_checkpoint_id) = backup_checkpoint_id else {
                status.set_text("No backup checkpoint found for undo.");
                return;
            };

            let preview = crate::restore::preview_restore_to_checkpoint(
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
            let undo_forced_paths: Vec<String> = preview
                .items
                .iter()
                .filter(|item| item.conflict)
                .map(|item| item.path.clone())
                .collect();
            let touched_list = touched_paths_preview(&preview, 8);
            let confirm_message = format!(
                "You are about to undo the last restore and touch {touched_count} file(s).\nConflicts detected: {conflict_count}.\n\nA new backup checkpoint will be created before applying.");
            let confirm_message = format!("{confirm_message}\n\nTouched files:\n{touched_list}");

            let on_confirm: Rc<dyn Fn()> = {
                let db = db.clone();
                let codex_thread_id = codex_thread_id.clone();
                let listbox = listbox.clone();
                let summary = summary.clone();
                let status = status.clone();
                let forced_paths = forced_paths.clone();
                let selected_checkpoint_id = selected_checkpoint_id.clone();
                let last_backup_checkpoint_id = last_backup_checkpoint_id.clone();
                let checkpoint_dropdown = checkpoint_dropdown.clone();
                let undo_forced_paths = undo_forced_paths.clone();
                Rc::new(move || {
                    match crate::restore::apply_restore_to_checkpoint(
                        &db,
                        &codex_thread_id,
                        backup_checkpoint_id,
                        &[],
                        &undo_forced_paths,
                    ) {
                        Ok(Some(result)) => {
                            last_backup_checkpoint_id.replace(Some(result.backup_checkpoint_id));
                            status.set_text(&format!(
                                "Undo restore applied from backup checkpoint {}. Restored: {} • Deleted: {} • Recreated: {} • Skipped conflicts: {} • New backup: {}",
                                backup_checkpoint_id,
                                result.restored_count,
                                result.deleted_count,
                                result.recreated_count,
                                result.skipped_conflicts,
                                result.backup_checkpoint_id
                            ));

                            if let Some(current_id) = *selected_checkpoint_id.borrow() {
                                let preview = crate::restore::preview_restore_to_checkpoint(
                                    &db,
                                    &codex_thread_id,
                                    current_id,
                                );
                                refresh_preview_list(preview, &listbox, &summary, &forced_paths);
                            } else {
                                let idx = checkpoint_dropdown.selected() as usize;
                                if let Some(id) =
                                    crate::restore::list_checkpoints_for_thread(&db, &codex_thread_id)
                                        .get(idx)
                                        .map(|cp| cp.id)
                                {
                                    let preview = crate::restore::preview_restore_to_checkpoint(
                                        &db,
                                        &codex_thread_id,
                                        id,
                                    );
                                    refresh_preview_list(preview, &listbox, &summary, &forced_paths);
                                }
                            }
                        }
                        Ok(None) => {
                            status.set_text("Undo restore unavailable.");
                        }
                        Err(err) => {
                            status.set_text(&format!("Undo restore failed: {err}"));
                        }
                    }
                })
            };

            open_confirmation_dialog(&dialog, "Confirm Undo Restore", &confirm_message, on_confirm);
        });
    }

    if checkpoint_map.borrow().is_empty() {
        summary.set_text("No restore checkpoints found for this thread yet.");
    }

    dialog.set_child(Some(&root));
    dialog.present();
}
