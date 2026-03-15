use crate::backend::RuntimeClient;
use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use crate::restore::RestoreAction;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

fn action_label(action: &RestoreAction) -> &'static str {
    match action {
        RestoreAction::Noop => "No change",
        RestoreAction::Write => "Restore",
        RestoreAction::Delete => "Delete",
        RestoreAction::Recreate => "Recreate",
    }
}

fn connect_runtime_for_thread(
    db: &AppDb,
    remote_thread_id: &str,
) -> Result<Arc<RuntimeClient>, String> {
    let profile = db
        .get_thread_profile_id_by_remote_thread_id(remote_thread_id)
        .ok()
        .flatten()
        .and_then(|profile_id| db.get_codex_profile(profile_id).ok().flatten());

    RuntimeClient::connect_for_profile(profile.as_ref(), "checkpoint-restore")
}

fn resolve_runtime_for_thread(
    db: &AppDb,
    manager: Option<&Rc<CodexProfileManager>>,
    remote_thread_id: &str,
) -> Result<Arc<RuntimeClient>, String> {
    if let Some(manager) = manager {
        if let Some(client) = manager.resolve_running_client_for_thread_id(remote_thread_id) {
            return Ok(client);
        }
        if let Some(client) = manager.resolve_client_for_thread_id(remote_thread_id) {
            return Ok(client);
        }
    }
    connect_runtime_for_thread(db, remote_thread_id)
}

fn update_selection_summary(
    summary: &gtk::Label,
    restore: &gtk::Button,
    selected_count: usize,
    total_count: usize,
) {
    if total_count == 0 {
        summary
            .set_text("No file changes at this checkpoint. Restore will only trim chat history.");
        restore.set_sensitive(true);
        return;
    }

    summary.set_text(&format!(
        "Selected {selected_count} of {total_count} file(s) for restore."
    ));
    restore.set_sensitive(selected_count > 0);
}

pub(super) fn open_checkpoint_restore_popup(
    parent: Option<gtk::Window>,
    db: Rc<AppDb>,
    manager: Option<Rc<CodexProfileManager>>,
    codex_thread_id: String,
    checkpoint_id: i64,
    turn_id: String,
    user_prompt: Option<String>,
) {
    let dialog = gtk::Window::builder()
        .title("Restore Checkpoint")
        .default_width(560)
        .default_height(420)
        .modal(true)
        .build();
    dialog.set_resizable(false);
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let heading = gtk::Label::new(Some("Restore this checkpoint?"));
    heading.set_xalign(0.0);
    heading.add_css_class("chat-restore-popup-heading");
    root.append(&heading);

    let summary = gtk::Label::new(Some(""));
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    summary.add_css_class("dim-label");
    root.append(&summary);

    let selection_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    selection_actions.set_halign(gtk::Align::End);

    let select_all = gtk::Button::with_label("Select all");
    let clear_all = gtk::Button::with_label("Clear all");
    selection_actions.append(&select_all);
    selection_actions.append(&clear_all);
    root.append(&selection_actions);

    let listbox = gtk::ListBox::new();
    listbox.add_css_class("navigation-sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&listbox)
        .min_content_height(220)
        .build();
    scroll.set_has_frame(false);
    root.append(&scroll);

    let status_card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    status_card.add_css_class("chat-restore-status-card");
    status_card.set_size_request(-1, 84);

    let stats_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    stats_row.add_css_class("chat-restore-stats-row");

    let make_stat = |title: &str| -> (gtk::Box, gtk::Label) {
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 1);
        cell.add_css_class("chat-restore-stat");
        cell.set_hexpand(true);

        let key = gtk::Label::new(Some(title));
        key.set_xalign(0.0);
        key.add_css_class("chat-restore-stat-key");
        cell.append(&key);

        let value = gtk::Label::new(Some("0"));
        value.set_xalign(0.0);
        value.add_css_class("chat-restore-stat-value");
        cell.append(&value);
        (cell, value)
    };

    let (restored_cell, restored_value) = make_stat("Restored");
    let (deleted_cell, deleted_value) = make_stat("Deleted");
    let (recreated_cell, recreated_value) = make_stat("Recreated");
    let (conflict_cell, conflict_value) = make_stat("Conflicts");

    stats_row.append(&restored_cell);
    stats_row.append(&deleted_cell);
    stats_row.append(&recreated_cell);
    stats_row.append(&conflict_cell);
    status_card.append(&stats_row);

    let status_detail = gtk::Label::new(Some("Review changes and press Restore."));
    status_detail.set_xalign(0.0);
    status_detail.set_wrap(true);
    status_detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_detail.add_css_class("chat-restore-status-detail");
    status_card.append(&status_detail);
    root.append(&status_card);

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

    let restore = gtk::Button::with_label("Restore");
    restore.add_css_class("suggested-action");
    actions.append(&restore);
    root.append(&actions);

    let preview = crate::restore::preview_restore_to_checkpoint_by_remote_id(
        &db,
        &codex_thread_id,
        checkpoint_id,
    );
    if let Some(preview) = preview {
        let selected_paths: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
        let toggles: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let mut touched_count = 0usize;
        for item in preview.items {
            if matches!(item.action, RestoreAction::Noop) {
                continue;
            }
            touched_count += 1;
            selected_paths.borrow_mut().insert(item.path.clone());
            let row = gtk::ListBoxRow::new();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            line.set_margin_start(8);
            line.set_margin_end(8);
            line.set_margin_top(6);
            line.set_margin_bottom(6);

            let include_toggle = gtk::CheckButton::new();
            include_toggle.set_active(true);
            line.append(&include_toggle);

            let path = gtk::Label::new(Some(&item.path));
            path.set_xalign(0.0);
            path.set_hexpand(true);
            path.add_css_class("restore-preview-path");
            line.append(&path);

            let action = gtk::Label::new(Some(action_label(&item.action)));
            action.add_css_class("restore-preview-kind");
            line.append(&action);

            {
                let selected_paths = selected_paths.clone();
                let summary = summary.clone();
                let restore = restore.clone();
                let path_value = item.path.clone();
                include_toggle.connect_toggled(move |toggle| {
                    if toggle.is_active() {
                        selected_paths.borrow_mut().insert(path_value.clone());
                    } else {
                        selected_paths.borrow_mut().remove(path_value.as_str());
                    }
                    let selected_count = selected_paths.borrow().len();
                    update_selection_summary(&summary, &restore, selected_count, touched_count);
                });
            }

            toggles
                .borrow_mut()
                .push((item.path.clone(), include_toggle.clone()));

            row.set_child(Some(&line));
            listbox.append(&row);
        }

        update_selection_summary(&summary, &restore, touched_count, touched_count);
        selection_actions.set_visible(touched_count > 0);

        {
            let toggles = toggles.clone();
            select_all.connect_clicked(move |_| {
                for (_, toggle) in toggles.borrow().iter() {
                    toggle.set_active(true);
                }
            });
        }

        {
            let toggles = toggles.clone();
            clear_all.connect_clicked(move |_| {
                for (_, toggle) in toggles.borrow().iter() {
                    toggle.set_active(false);
                }
            });
        }

        let status_detail_label = status_detail.clone();
        let restored_value_label = restored_value.clone();
        let deleted_value_label = deleted_value.clone();
        let recreated_value_label = recreated_value.clone();
        let conflict_value_label = conflict_value.clone();
        let db = db.clone();
        let manager = manager.clone();
        let codex_thread_id = codex_thread_id.clone();
        let turn_id = turn_id.clone();
        let user_prompt = user_prompt.clone();
        let parent_window = parent.clone();
        let selected_paths = selected_paths.clone();
        restore.connect_clicked(move |_| {
            let selected: Vec<String> = selected_paths.borrow().iter().cloned().collect();
            let codex = match resolve_runtime_for_thread(&db, manager.as_ref(), &codex_thread_id) {
                Ok(client) => Some(client),
                Err(err) => {
                    status_detail_label.set_text(&format!(
                        "Restore applied, but chat trim failed: unable to connect the runtime ({err})"
                    ));
                    return;
                }
            };
            let active_thread_id = Rc::new(RefCell::new(Some(codex_thread_id.clone())));
            let workspace_path = db
                .workspace_path_for_remote_thread(&codex_thread_id)
                .ok()
                .flatten()
                .or_else(|| db.get_setting("last_active_workspace_path").ok().flatten());
            match crate::ui::components::restore_preview::apply_restore_with_chat_sync(
                &db,
                codex,
                Some(active_thread_id),
                workspace_path.as_deref(),
                &codex_thread_id,
                checkpoint_id,
                Some(turn_id.as_str()),
                &selected,
                &[],
                parent_window.as_ref(),
                user_prompt.as_deref(),
            ) {
                Ok((result, rollback_status)) => {
                    restored_value_label.set_text(&result.restored_count.to_string());
                    deleted_value_label.set_text(&result.deleted_count.to_string());
                    recreated_value_label.set_text(&result.recreated_count.to_string());
                    conflict_value_label.set_text(&result.skipped_conflicts.to_string());
                    let rollback_detail = rollback_status
                        .trim()
                        .trim_start_matches('•')
                        .trim()
                        .to_string();
                    if rollback_detail.is_empty() {
                        status_detail_label.set_text("Restore completed.");
                    } else {
                        status_detail_label.set_text(&rollback_detail);
                    }
                }
                Err(err) => {
                    status_detail_label.set_text(&err);
                }
            }
        });
    } else {
        summary.set_text("No restore preview available for this checkpoint.");
        selection_actions.set_visible(false);
        restore.set_sensitive(false);
    }

    dialog.set_child(Some(&root));
    dialog.present();
}
