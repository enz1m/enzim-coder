use adw::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use crate::services::app::chat::AppDb;
use crate::ui::components::file_preview;

mod dialogs;
mod model;
mod operations;
mod runtime;

use dialogs::{
    open_branch_manager_popover, open_git_feedback_dialog, open_init_repository_dialog,
    open_upstream_dialog,
};
use model::{GitFileEntry, GitSnapshot, WorkerEvent};
use operations::{
    install_refresh_on_map, install_workspace_observer, list_local_branches, load_git_snapshot,
    resolve_workspace_root, run_branch_action, run_commit_selected, run_configure_upstream,
    run_fetch, run_initialize_repository, run_pull_ff_only, run_push_with_optional_credentials,
    selected_line_delta,
};
use runtime::install_worker_event_pump;

const MAX_RENDERED_GIT_FILES: usize = 400;

fn build_action_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(13);
    icon.add_css_class("git-tab-action-icon");
    let text = gtk::Label::new(Some(label));
    text.add_css_class("git-tab-action-label");
    row.append(&icon);
    row.append(&text);
    button.set_child(Some(&row));
    button
}

pub fn build_git_tab(
    db: Rc<AppDb>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content_box.set_margin_start(0);
    content_box.set_margin_end(14);
    content_box.set_margin_top(0);
    content_box.set_margin_bottom(0);
    content_box.set_vexpand(true);

    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("chat-frame");
    frame.set_vexpand(true);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("git-tab-root");
    root.set_margin_start(10);
    root.set_margin_end(10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_vexpand(true);
    frame.append(&root);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("git-tab-header");

    let repo_meta_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    repo_meta_box.set_hexpand(true);

    let workspace_label = gtk::Label::new(Some("Workspace: —"));
    workspace_label.add_css_class("git-tab-meta");
    workspace_label.set_xalign(0.0);
    workspace_label.set_hexpand(true);
    workspace_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let repository_label = gtk::Label::new(Some("Repository: —"));
    repository_label.add_css_class("git-tab-meta");
    repository_label.add_css_class("git-tab-muted");
    repository_label.add_css_class("dim-label");
    repository_label.set_xalign(0.0);
    repository_label.set_hexpand(true);
    repository_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

    repo_meta_box.append(&workspace_label);
    repo_meta_box.append(&repository_label);

    let branch_button = gtk::Button::with_label("—");
    branch_button.set_has_frame(false);
    branch_button.add_css_class("app-flat-button");
    branch_button.add_css_class("git-tab-branch");
    branch_button.set_tooltip_text(Some("Manage branches"));

    let selected_delta_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    selected_delta_box.add_css_class("git-tab-selected-delta");
    selected_delta_box.set_valign(gtk::Align::Center);
    selected_delta_box.set_visible(false);

    let selected_added_label = gtk::Label::new(Some("+0"));
    selected_added_label.add_css_class("git-tab-delta-add");
    let selected_removed_label = gtk::Label::new(Some("-0"));
    selected_removed_label.add_css_class("git-tab-delta-remove");
    selected_delta_box.append(&selected_added_label);
    selected_delta_box.append(&selected_removed_label);

    let refresh_button = gtk::Button::new();
    refresh_button.set_has_frame(false);
    refresh_button.set_icon_name("view-refresh-symbolic");
    refresh_button.set_tooltip_text(Some("Refresh Git status"));
    refresh_button.add_css_class("app-flat-button");
    refresh_button.add_css_class("git-tab-refresh");

    header.append(&repo_meta_box);
    header.append(&selected_delta_box);
    header.append(&branch_button);
    header.append(&refresh_button);

    let outgoing_card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    outgoing_card.add_css_class("git-tab-outgoing-card");

    let outgoing_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let outgoing_title = gtk::Label::new(Some("Ready to Push"));
    outgoing_title.add_css_class("git-tab-outgoing-title");
    outgoing_title.set_xalign(0.0);
    outgoing_title.set_hexpand(true);

    let outgoing_count = gtk::Label::new(Some("0"));
    outgoing_count.add_css_class("git-tab-outgoing-count");
    outgoing_header.append(&outgoing_title);
    outgoing_header.append(&outgoing_count);

    let outgoing_hint = gtk::Label::new(Some("All commits are pushed."));
    outgoing_hint.add_css_class("git-tab-outgoing-hint");
    outgoing_hint.set_xalign(0.0);

    let outgoing_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    outgoing_list.add_css_class("git-tab-outgoing-list");

    outgoing_card.append(&outgoing_header);
    outgoing_card.append(&outgoing_hint);
    outgoing_card.append(&outgoing_list);

    let listbox = gtk::ListBox::new();
    listbox.add_css_class("git-tab-list");
    listbox.set_selection_mode(gtk::SelectionMode::None);
    listbox.set_margin_start(1);
    listbox.set_margin_end(1);
    listbox.set_margin_top(1);
    listbox.set_margin_bottom(1);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&listbox)
        .build();
    scroll.add_css_class("git-tab-scroll");
    scroll.set_overflow(gtk::Overflow::Hidden);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("git-tab-footer");

    let init_button = gtk::Button::with_label("Initialize Git");
    init_button.add_css_class("git-tab-init");

    let commit_message = gtk::Entry::new();
    commit_message.set_hexpand(true);
    commit_message.set_placeholder_text(Some("Commit message"));
    commit_message.add_css_class("git-tab-commit-message");

    let commit_button = build_action_button("commit-symbolic", "Commit Selected");
    commit_button.add_css_class("suggested-action");
    commit_button.add_css_class("git-tab-action-button");

    let fetch_button = build_action_button("import-symbolic", "Fetch");
    fetch_button.add_css_class("git-tab-action-button");
    let pull_button = build_action_button("pull-request-symbolic", "Pull");
    pull_button.add_css_class("git-tab-action-button");
    let push_button = build_action_button("cloud-deploy-symbolic", "Push");
    push_button.add_css_class("git-tab-action-button");
    let upstream_button = gtk::Button::with_label("Configure Upstream");

    footer.append(&init_button);
    footer.append(&commit_message);
    footer.append(&commit_button);
    footer.append(&fetch_button);
    footer.append(&pull_button);
    footer.append(&push_button);
    footer.append(&upstream_button);

    let status_label = gtk::Label::new(Some(""));
    status_label.add_css_class("git-tab-status");
    status_label.add_css_class("git-tab-muted");
    status_label.add_css_class("dim-label");
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);

    root.append(&header);
    root.append(&outgoing_card);
    root.append(&scroll);
    root.append(&footer);
    root.append(&status_label);
    content_box.append(&frame);

    let entries_state = Rc::new(RefCell::new(Vec::<GitFileEntry>::new()));
    let snapshot_state = Rc::new(RefCell::new(None::<GitSnapshot>));
    let operation_busy = Rc::new(RefCell::new(false));
    let no_repo_state = Rc::new(RefCell::new(false));
    let observed_workspace_root = Rc::new(RefCell::new(resolve_workspace_root(
        &db,
        &active_workspace_path,
    )));

    let (worker_tx, worker_rx) = mpsc::channel::<WorkerEvent>();

    let update_actions: Rc<dyn Fn()> = {
        let entries_state = entries_state.clone();
        let snapshot_state = snapshot_state.clone();
        let operation_busy = operation_busy.clone();
        let no_repo_state = no_repo_state.clone();
        let commit_message = commit_message.clone();
        let init_button = init_button.clone();
        let commit_button = commit_button.clone();
        let fetch_button = fetch_button.clone();
        let pull_button = pull_button.clone();
        let branch_button = branch_button.clone();
        let push_button = push_button.clone();
        let upstream_button = upstream_button.clone();
        Rc::new(move || {
            let is_busy = *operation_busy.borrow();
            let snapshot = snapshot_state.borrow().clone();
            let has_repo = snapshot.is_some();
            let no_repo = *no_repo_state.borrow();
            let has_selected = entries_state.borrow().iter().any(|entry| entry.selected);
            let has_message = !commit_message.text().trim().is_empty();
            let can_push = snapshot
                .as_ref()
                .map(|value| value.has_upstream && value.ahead_count > 0)
                .unwrap_or(false);
            let needs_upstream = snapshot
                .as_ref()
                .map(|value| !value.has_upstream)
                .unwrap_or(false);
            let is_detached = snapshot
                .as_ref()
                .map(|value| value.branch_label.starts_with("detached@"))
                .unwrap_or(false);

            init_button.set_visible(no_repo);
            init_button.set_sensitive(!is_busy && no_repo);
            commit_button.set_sensitive(!is_busy && has_repo && has_selected && has_message);
            fetch_button.set_visible(has_repo);
            fetch_button.set_sensitive(!is_busy && has_repo);
            pull_button.set_visible(has_repo && !needs_upstream);
            pull_button.set_sensitive(!is_busy && has_repo && !needs_upstream && !is_detached);
            branch_button.set_sensitive(!is_busy && has_repo);
            push_button.set_visible(can_push);
            push_button.set_sensitive(!is_busy && has_repo && can_push);
            upstream_button.set_visible(has_repo && needs_upstream);
            upstream_button.set_sensitive(!is_busy && has_repo && needs_upstream);
        })
    };

    let refresh_selected_delta: Rc<dyn Fn()> = {
        let snapshot_state = snapshot_state.clone();
        let entries_state = entries_state.clone();
        let selected_delta_box = selected_delta_box.clone();
        let selected_added_label = selected_added_label.clone();
        let selected_removed_label = selected_removed_label.clone();
        let status_label = status_label.clone();
        Rc::new(move || {
            let Some(snapshot) = snapshot_state.borrow().clone() else {
                selected_delta_box.set_visible(false);
                selected_added_label.set_text("+0");
                selected_removed_label.set_text("-0");
                return;
            };
            let entries = entries_state.borrow().clone();
            match selected_line_delta(&snapshot.workspace_root, &entries) {
                Ok((added, removed)) => {
                    selected_delta_box.set_visible(true);
                    selected_added_label.set_text(&format!("+{}", added));
                    selected_removed_label.set_text(&format!("-{}", removed));
                }
                Err(err) => {
                    selected_delta_box.set_visible(false);
                    status_label
                        .set_text(&format!("Unable to calculate selected-file diff: {}", err));
                }
            }
        })
    };

    let render_entries: Rc<dyn Fn()> = {
        let listbox = listbox.clone();
        let entries_state = entries_state.clone();
        let snapshot_state = snapshot_state.clone();
        let no_repo_state = no_repo_state.clone();
        let outgoing_card = outgoing_card.clone();
        let outgoing_count = outgoing_count.clone();
        let outgoing_hint = outgoing_hint.clone();
        let outgoing_list = outgoing_list.clone();
        let status_label = status_label.clone();
        let update_actions = update_actions.clone();
        let refresh_selected_delta = refresh_selected_delta.clone();
        Rc::new(move || {
            while let Some(child) = listbox.first_child() {
                listbox.remove(&child);
            }
            while let Some(child) = outgoing_list.first_child() {
                outgoing_list.remove(&child);
            }

            let snapshot = snapshot_state.borrow().clone();
            if snapshot.is_none() {
                outgoing_card.set_visible(false);
                let empty_text = if *no_repo_state.borrow() {
                    "No Git repository in this workspace. Use Initialize Git to get started."
                } else {
                    "Open a Git workspace and click Refresh."
                };
                let empty = gtk::Label::new(Some(empty_text));
                empty.add_css_class("git-tab-muted");
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                empty.set_margin_start(8);
                empty.set_margin_end(8);
                empty.set_margin_top(8);
                empty.set_margin_bottom(8);
                listbox.append(&empty);
                refresh_selected_delta();
                update_actions();
                return;
            }

            outgoing_card.set_visible(true);
            let workspace_root = snapshot
                .as_ref()
                .map(|value| value.workspace_root.clone())
                .unwrap_or_default();
            let outgoing = snapshot
                .as_ref()
                .map(|value| value.unpushed_commits.clone())
                .unwrap_or_default();
            let ahead_count = snapshot
                .as_ref()
                .map(|value| value.ahead_count)
                .unwrap_or(0);
            let push_hint = snapshot
                .as_ref()
                .map(|value| value.push_hint.clone())
                .unwrap_or_else(|| "All commits are pushed.".to_string());

            outgoing_count.set_text(&ahead_count.to_string());
            outgoing_hint.set_text(&push_hint);
            for commit in outgoing {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.add_css_class("git-tab-outgoing-item");

                let hash = gtk::Label::new(Some(&commit.short_hash));
                hash.add_css_class("git-tab-outgoing-hash");
                hash.set_xalign(0.0);

                let summary = gtk::Label::new(Some(&commit.summary));
                summary.add_css_class("git-tab-outgoing-summary");
                summary.set_xalign(0.0);
                summary.set_hexpand(true);
                summary.set_ellipsize(gtk::pango::EllipsizeMode::End);

                row.append(&hash);
                row.append(&summary);
                outgoing_list.append(&row);
            }

            let entries = entries_state.borrow().clone();

            if entries.is_empty() {
                let empty = gtk::Label::new(Some("Working tree is clean."));
                empty.add_css_class("git-tab-muted");
                empty.add_css_class("dim-label");
                empty.set_xalign(0.0);
                empty.set_margin_start(8);
                empty.set_margin_end(8);
                empty.set_margin_top(8);
                empty.set_margin_bottom(8);
                listbox.append(&empty);
                refresh_selected_delta();
                update_actions();
                return;
            }

            let total_entries = entries.len();
            let visible_count = total_entries.min(MAX_RENDERED_GIT_FILES);
            if total_entries > MAX_RENDERED_GIT_FILES {
                let overflow = gtk::Label::new(Some(&format!(
                    "Showing the first {} of {} changed paths. This workspace is very large; use Refresh after narrowing the repo state or use Git in the terminal for the full list.",
                    visible_count, total_entries
                )));
                overflow.add_css_class("git-tab-muted");
                overflow.add_css_class("dim-label");
                overflow.set_wrap(true);
                overflow.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                overflow.set_xalign(0.0);
                overflow.set_margin_start(8);
                overflow.set_margin_end(8);
                overflow.set_margin_top(8);
                overflow.set_margin_bottom(8);
                listbox.append(&overflow);
            }

            for (idx, entry) in entries.iter().take(MAX_RENDERED_GIT_FILES).enumerate() {
                let row = gtk::ListBoxRow::new();
                row.add_css_class("git-tab-item");
                row.set_selectable(false);
                row.set_activatable(false);

                let row_content = gtk::Box::new(gtk::Orientation::Horizontal, 1);
                row_content.set_margin_start(4);
                row_content.set_margin_end(4);
                row_content.set_margin_top(0);
                row_content.set_margin_bottom(0);

                let checkbox = gtk::CheckButton::new();
                checkbox.set_active(entry.selected);
                checkbox.add_css_class("git-tab-file-check");

                let status = gtk::Label::new(Some(&entry.status));
                status.add_css_class("git-tab-file-status");
                status.set_width_chars(2);

                let path_button = gtk::Button::new();
                path_button.set_has_frame(false);
                path_button.add_css_class("git-tab-file-button");
                path_button.set_hexpand(true);
                path_button.set_halign(gtk::Align::Fill);
                let path_label = gtk::Label::new(Some(&entry.path));
                path_label.add_css_class("git-tab-file-path");
                path_label.set_xalign(0.0);
                path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                path_label.set_hexpand(true);
                path_button.set_child(Some(&path_label));
                {
                    let entries_state = entries_state.clone();
                    let update_actions = update_actions.clone();
                    let refresh_selected_delta = refresh_selected_delta.clone();
                    checkbox.connect_toggled(move |check| {
                        if let Some(value) = entries_state.borrow_mut().get_mut(idx) {
                            value.selected = check.is_active();
                        }
                        refresh_selected_delta();
                        update_actions();
                    });
                }

                {
                    let status_label = status_label.clone();
                    let repo_root = PathBuf::from(&workspace_root);
                    let file_path = PathBuf::from(&workspace_root).join(&entry.path);
                    let file_status = entry.status.clone();
                    path_button.connect_clicked(move |_| {
                        if file_path.exists() || file_status == "D" || file_status == "??" {
                            file_preview::open_git_diff_preview(
                                &repo_root,
                                &file_path,
                                &file_status,
                            );
                        } else {
                            status_label
                                .set_text("File preview unavailable: file no longer exists.");
                        }
                    });
                }

                row_content.append(&checkbox);
                row_content.append(&status);
                row_content.append(&path_button);
                row.set_child(Some(&row_content));
                listbox.append(&row);
            }

            refresh_selected_delta();
            update_actions();
        })
    };

    let trigger_refresh: Rc<dyn Fn()> = {
        let db = db.clone();
        let active_workspace_path = active_workspace_path.clone();
        let content_box = content_box.clone();
        let operation_busy = operation_busy.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        Rc::new(move || {
            if !content_box.is_mapped() || !content_box.is_visible() {
                return;
            }
            if *operation_busy.borrow() {
                return;
            }
            operation_busy.replace(true);
            status_label.set_text("Refreshing Git status...");
            update_actions();

            let workspace_root = resolve_workspace_root(&db, &active_workspace_path)
                .to_string_lossy()
                .to_string();
            let worker_tx = worker_tx.clone();
            thread::spawn(move || {
                let result = load_git_snapshot(&workspace_root);
                let _ = worker_tx.send(WorkerEvent::Loaded(result));
            });
        })
    };

    {
        let trigger_refresh = trigger_refresh.clone();
        refresh_button.connect_clicked(move |_| trigger_refresh());
    }

    {
        let update_actions = update_actions.clone();
        commit_message.connect_changed(move |_| update_actions());
    }

    {
        let commit_button = commit_button.clone();
        commit_message.connect_activate(move |_| {
            if commit_button.is_sensitive() {
                commit_button.emit_clicked();
                commit_button.grab_focus();
            }
        });
    }

    {
        let commit_message = commit_message.clone();
        let push_button = push_button.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        key_controller.connect_key_pressed(move |_, key, _, _| {
            let is_enter = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
            if !is_enter {
                return gtk::glib::Propagation::Proceed;
            }

            if commit_message.has_focus() {
                return gtk::glib::Propagation::Proceed;
            }

            if push_button.is_visible() && push_button.is_sensitive() {
                push_button.emit_clicked();
                return gtk::glib::Propagation::Stop;
            }

            gtk::glib::Propagation::Proceed
        });
        root.add_controller(key_controller);
    }

    {
        let db = db.clone();
        let active_workspace_path = active_workspace_path.clone();
        let operation_busy = operation_busy.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        init_button.connect_clicked(move |_| {
            if *operation_busy.borrow() {
                return;
            }

            let workspace_root = resolve_workspace_root(&db, &active_workspace_path)
                .to_string_lossy()
                .to_string();
            let workspace_root_for_dialog = workspace_root.clone();
            let parent = gtk::Application::default()
                .active_window()
                .and_then(|window| window.downcast::<gtk::Window>().ok());

            let operation_busy = operation_busy.clone();
            let status_label = status_label.clone();
            let worker_tx = worker_tx.clone();
            let update_actions = update_actions.clone();
            open_init_repository_dialog(
                parent,
                &workspace_root_for_dialog,
                Rc::new(move |options| {
                    if *operation_busy.borrow() {
                        return;
                    }
                    operation_busy.replace(true);
                    status_label.set_text("Initializing Git repository...");
                    update_actions();

                    let worker_tx = worker_tx.clone();
                    let workspace_root = workspace_root.clone();
                    thread::spawn(move || {
                        let result = run_initialize_repository(&workspace_root, &options);
                        let _ = worker_tx.send(WorkerEvent::InitDone(result));
                    });
                }),
            );
        });
    }

    {
        let operation_busy = operation_busy.clone();
        let snapshot_state = snapshot_state.clone();
        let entries_state = entries_state.clone();
        let commit_message = commit_message.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        commit_button.connect_clicked(move |_| {
            if *operation_busy.borrow() {
                return;
            }

            let Some(snapshot) = snapshot_state.borrow().clone() else {
                status_label.set_text("No Git repository loaded.");
                let parent = gtk::Application::default()
                    .active_window()
                    .and_then(|window| window.downcast::<gtk::Window>().ok());
                open_git_feedback_dialog(parent, "Commit Unavailable", "No Git repository loaded.");
                return;
            };

            let message = commit_message.text().trim().to_string();
            if message.is_empty() {
                status_label.set_text("Commit message is required.");
                let parent = gtk::Application::default()
                    .active_window()
                    .and_then(|window| window.downcast::<gtk::Window>().ok());
                open_git_feedback_dialog(
                    parent,
                    "Commit Message Required",
                    "Enter a commit message before committing.",
                );
                return;
            }

            let selected_paths: Vec<String> = entries_state
                .borrow()
                .iter()
                .filter(|entry| entry.selected)
                .map(|entry| entry.path.clone())
                .collect();

            if selected_paths.is_empty() {
                status_label.set_text("Select at least one file to commit.");
                let parent = gtk::Application::default()
                    .active_window()
                    .and_then(|window| window.downcast::<gtk::Window>().ok());
                open_git_feedback_dialog(
                    parent,
                    "No Files Selected",
                    "Select at least one file to commit.",
                );
                return;
            }

            operation_busy.replace(true);
            status_label.set_text("Creating commit...");
            update_actions();

            let worker_tx = worker_tx.clone();
            thread::spawn(move || {
                let result =
                    run_commit_selected(&snapshot.workspace_root, &selected_paths, &message);
                let _ = worker_tx.send(WorkerEvent::CommitDone(result));
            });
        });
    }

    {
        let operation_busy = operation_busy.clone();
        let snapshot_state = snapshot_state.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        fetch_button.connect_clicked(move |_| {
            if *operation_busy.borrow() {
                return;
            }
            let Some(snapshot) = snapshot_state.borrow().clone() else {
                status_label.set_text("No Git repository loaded.");
                let parent = gtk::Application::default()
                    .active_window()
                    .and_then(|window| window.downcast::<gtk::Window>().ok());
                open_git_feedback_dialog(parent, "Fetch Unavailable", "No Git repository loaded.");
                return;
            };

            operation_busy.replace(true);
            status_label.set_text("Fetching from remote...");
            update_actions();

            let worker_tx = worker_tx.clone();
            let workspace_root = snapshot.workspace_root.clone();
            thread::spawn(move || {
                let result = run_fetch(&workspace_root);
                let _ = worker_tx.send(WorkerEvent::FetchDone(result));
            });
        });
    }

    {
        let operation_busy = operation_busy.clone();
        let snapshot_state = snapshot_state.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        pull_button.connect_clicked(move |_| {
            if *operation_busy.borrow() {
                return;
            }
            let Some(snapshot) = snapshot_state.borrow().clone() else {
                status_label.set_text("No Git repository loaded.");
                let parent = gtk::Application::default()
                    .active_window()
                    .and_then(|window| window.downcast::<gtk::Window>().ok());
                open_git_feedback_dialog(parent, "Pull Unavailable", "No Git repository loaded.");
                return;
            };
            if snapshot.branch_label.starts_with("detached@") {
                status_label
                    .set_text("Pull is unavailable in detached HEAD. Switch to a branch first.");
                let parent = gtk::Application::default()
                    .active_window()
                    .and_then(|window| window.downcast::<gtk::Window>().ok());
                open_git_feedback_dialog(
                    parent,
                    "Pull Unavailable",
                    "Pull is unavailable in detached HEAD. Switch to a branch first.",
                );
                return;
            }

            operation_busy.replace(true);
            status_label.set_text("Pulling latest changes...");
            update_actions();

            let worker_tx = worker_tx.clone();
            let workspace_root = snapshot.workspace_root.clone();
            thread::spawn(move || {
                let result = run_pull_ff_only(&workspace_root);
                let _ = worker_tx.send(WorkerEvent::PullDone(result));
            });
        });
    }

    {
        let operation_busy = operation_busy.clone();
        let snapshot_state = snapshot_state.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        let branch_anchor = branch_button.clone();
        branch_button.connect_clicked(move |_| {
            if *operation_busy.borrow() {
                return;
            }
            let Some(snapshot) = snapshot_state.borrow().clone() else {
                status_label.set_text("No Git repository loaded.");
                return;
            };

            let branches = list_local_branches(&snapshot.workspace_root);
            let current_branch = snapshot.branch_label.clone();
            let operation_busy = operation_busy.clone();
            let status_label = status_label.clone();
            let worker_tx = worker_tx.clone();
            let update_actions = update_actions.clone();
            let workspace_root = snapshot.workspace_root.clone();
            open_branch_manager_popover(
                &branch_anchor,
                &current_branch,
                &branches,
                Rc::new(move |action| {
                    if *operation_busy.borrow() {
                        return;
                    }
                    operation_busy.replace(true);
                    status_label.set_text("Updating branch...");
                    update_actions();

                    let worker_tx = worker_tx.clone();
                    let workspace_root = workspace_root.clone();
                    thread::spawn(move || {
                        let result = run_branch_action(&workspace_root, action);
                        let _ = worker_tx.send(WorkerEvent::BranchDone(result));
                    });
                }),
            );
        });
    }

    {
        let operation_busy = operation_busy.clone();
        let snapshot_state = snapshot_state.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        upstream_button.connect_clicked(move |_| {
            if *operation_busy.borrow() {
                return;
            }

            let Some(snapshot) = snapshot_state.borrow().clone() else {
                status_label.set_text("No Git repository loaded.");
                return;
            };

            let default_branch = snapshot
                .branch_label
                .strip_prefix("detached@")
                .map(|_| "main".to_string())
                .unwrap_or_else(|| snapshot.branch_label.clone());
            let default_remote = if snapshot.remotes.iter().any(|remote| remote == "origin") {
                "origin".to_string()
            } else {
                snapshot
                    .remotes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "origin".to_string())
            };
            let workspace_root_for_dialog = snapshot.workspace_root.clone();
            let default_remote_url = if snapshot.repository_url == "—" {
                String::new()
            } else {
                snapshot.repository_url.clone()
            };

            let parent = gtk::Application::default()
                .active_window()
                .and_then(|window| window.downcast::<gtk::Window>().ok());

            let operation_busy = operation_busy.clone();
            let status_label = status_label.clone();
            let worker_tx = worker_tx.clone();
            let update_actions = update_actions.clone();
            open_upstream_dialog(
                parent,
                &workspace_root_for_dialog,
                &default_remote,
                &default_remote_url,
                &default_branch,
                Rc::new(move |options| {
                    if *operation_busy.borrow() {
                        return;
                    }

                    operation_busy.replace(true);
                    status_label.set_text("Configuring upstream...");
                    update_actions();

                    let worker_tx = worker_tx.clone();
                    let workspace_root = snapshot.workspace_root.clone();
                    thread::spawn(move || {
                        let result = run_configure_upstream(&workspace_root, &options, None);
                        let _ = worker_tx.send(WorkerEvent::UpstreamDone(result));
                    });
                }),
            );
        });
    }

    {
        let operation_busy = operation_busy.clone();
        let snapshot_state = snapshot_state.clone();
        let status_label = status_label.clone();
        let worker_tx = worker_tx.clone();
        let update_actions = update_actions.clone();
        push_button.connect_clicked(move |_| {
            if *operation_busy.borrow() {
                return;
            }

            let Some(snapshot) = snapshot_state.borrow().clone() else {
                status_label.set_text("No Git repository loaded.");
                return;
            };

            operation_busy.replace(true);
            status_label.set_text("Pushing...");
            update_actions();

            let worker_tx = worker_tx.clone();
            thread::spawn(move || {
                let outcome = run_push_with_optional_credentials(&snapshot.workspace_root, None);
                let _ = worker_tx.send(WorkerEvent::PushDone(outcome));
            });
        });
    }

    install_worker_event_pump(
        worker_rx,
        worker_tx.clone(),
        operation_busy.clone(),
        snapshot_state.clone(),
        entries_state.clone(),
        no_repo_state.clone(),
        workspace_label.clone(),
        repository_label.clone(),
        branch_button.clone(),
        status_label.clone(),
        render_entries.clone(),
        trigger_refresh.clone(),
        commit_message.clone(),
        update_actions.clone(),
    );

    install_workspace_observer(
        db.clone(),
        active_workspace_path.clone(),
        observed_workspace_root.clone(),
        trigger_refresh.clone(),
    );
    install_refresh_on_map(&content_box, trigger_refresh.clone());
    {
        let commit_message = commit_message.clone();
        content_box.connect_map(move |_| {
            commit_message.grab_focus();
        });
    }

    content_box
}
