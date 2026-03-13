use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use super::dialogs::{open_git_feedback_dialog, open_push_credentials_dialog};
use super::model::{GitFileEntry, GitSnapshot, PushOutcome, UpstreamOptions, WorkerEvent};
use super::operations::{run_configure_upstream, run_push_with_optional_credentials};

pub(super) fn install_worker_event_pump(
    worker_rx: mpsc::Receiver<WorkerEvent>,
    worker_tx: mpsc::Sender<WorkerEvent>,
    operation_busy: Rc<RefCell<bool>>,
    snapshot_state: Rc<RefCell<Option<GitSnapshot>>>,
    entries_state: Rc<RefCell<Vec<GitFileEntry>>>,
    no_repo_state: Rc<RefCell<bool>>,
    workspace_label: gtk::Label,
    repository_label: gtk::Label,
    branch_button: gtk::Button,
    status_label: gtk::Label,
    render_entries: Rc<dyn Fn()>,
    trigger_refresh: Rc<dyn Fn()>,
    commit_message: gtk::Entry,
    update_actions: Rc<dyn Fn()>,
) {
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
        while let Ok(event) = worker_rx.try_recv() {
            match event {
                WorkerEvent::Loaded(result) => {
                    operation_busy.replace(false);
                    match result {
                        Ok(snapshot) => {
                            no_repo_state.replace(false);
                            workspace_label.set_text(&format!(
                                "Workspace: {} ({})",
                                snapshot.repo_name, snapshot.workspace_root
                            ));
                            repository_label
                                .set_text(&format!("Repository: {}", snapshot.repository_url));
                            branch_button.set_label(&snapshot.branch_label);
                            status_label.set_text(&format!(
                                "Loaded {} changed file(s).",
                                snapshot.files.len()
                            ));
                            entries_state.replace(snapshot.files.clone());
                            snapshot_state.replace(Some(snapshot));
                            render_entries();
                        }
                        Err(err) => {
                            let is_no_repo =
                                err == "No Git repository found for the active workspace.";
                            no_repo_state.replace(is_no_repo);
                            workspace_label.set_text("Workspace: —");
                            repository_label.set_text("Repository: —");
                            branch_button.set_label("—");
                            status_label.set_text(&err);
                            entries_state.replace(Vec::new());
                            snapshot_state.replace(None);
                            render_entries();
                        }
                    }
                }
                WorkerEvent::CommitDone(result) => {
                    operation_busy.replace(false);
                    match result {
                        Ok(message) => {
                            status_label.set_text(&message);
                            commit_message.set_text("");
                        }
                        Err(err) => {
                            status_label.set_text(&err);
                            let parent = gtk::Application::default()
                                .active_window()
                                .and_then(|window| window.downcast::<gtk::Window>().ok());
                            open_git_feedback_dialog(parent, "Commit Failed", &err);
                        }
                    }
                    update_actions();
                    trigger_refresh();
                }
                WorkerEvent::PushDone(outcome) => {
                    operation_busy.replace(false);
                    match outcome {
                        PushOutcome::Success(message) => {
                            status_label.set_text(&message);
                            update_actions();
                            trigger_refresh();
                        }
                        PushOutcome::Failure(err) => {
                            status_label.set_text(&err);
                            let parent = gtk::Application::default()
                                .active_window()
                                .and_then(|window| window.downcast::<gtk::Window>().ok());
                            open_git_feedback_dialog(parent, "Push Failed", &err);
                            update_actions();
                        }
                        PushOutcome::AuthRequired(initial_error) => {
                            status_label.set_text(
                                "Authentication required. Enter HTTPS username and token/password.",
                            );
                            update_actions();

                            let operation_busy = operation_busy.clone();
                            let update_actions = update_actions.clone();
                            let worker_tx = worker_tx.clone();
                            let snapshot = snapshot_state.borrow().clone();
                            if let Some(snapshot) = snapshot {
                                let parent = gtk::Application::default()
                                    .active_window()
                                    .and_then(|window| window.downcast::<gtk::Window>().ok());
                                let needs_upstream = !snapshot.has_upstream;
                                let branch = snapshot
                                    .branch_label
                                    .strip_prefix("detached@")
                                    .map(|_| "main".to_string())
                                    .unwrap_or_else(|| snapshot.branch_label.clone());
                                let remote = snapshot
                                    .remotes
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| "origin".to_string());
                                open_push_credentials_dialog(
                                    parent,
                                    &initial_error,
                                    Rc::new(move |credentials| {
                                        if *operation_busy.borrow() {
                                            return;
                                        }
                                        operation_busy.replace(true);
                                        update_actions();

                                        let worker_tx = worker_tx.clone();
                                        let repo_root = snapshot.workspace_root.clone();
                                        let remote_for_thread = remote.clone();
                                        let branch_for_thread = branch.clone();
                                        thread::spawn(move || {
                                            if needs_upstream {
                                                let options = UpstreamOptions {
                                                    remote_name: remote_for_thread,
                                                    remote_url: String::new(),
                                                    branch_name: branch_for_thread,
                                                };
                                                let result = run_configure_upstream(
                                                    &repo_root,
                                                    &options,
                                                    Some(credentials),
                                                );
                                                let _ = worker_tx
                                                    .send(WorkerEvent::UpstreamDone(result));
                                            } else {
                                                let outcome = run_push_with_optional_credentials(
                                                    &repo_root,
                                                    Some(credentials),
                                                );
                                                let _ =
                                                    worker_tx.send(WorkerEvent::PushDone(outcome));
                                            }
                                        });
                                    }),
                                );
                            }
                        }
                    }
                }
                WorkerEvent::FetchDone(result) => {
                    operation_busy.replace(false);
                    match result {
                        Ok(message) => {
                            status_label.set_text(&message);
                            update_actions();
                            trigger_refresh();
                        }
                        Err(err) => {
                            status_label.set_text(&err);
                            let parent = gtk::Application::default()
                                .active_window()
                                .and_then(|window| window.downcast::<gtk::Window>().ok());
                            open_git_feedback_dialog(parent, "Fetch Failed", &err);
                            update_actions();
                        }
                    }
                }
                WorkerEvent::PullDone(result) => {
                    operation_busy.replace(false);
                    match result {
                        Ok(message) => {
                            status_label.set_text(&message);
                            update_actions();
                            trigger_refresh();
                        }
                        Err(err) => {
                            status_label.set_text(&err);
                            let parent = gtk::Application::default()
                                .active_window()
                                .and_then(|window| window.downcast::<gtk::Window>().ok());
                            open_git_feedback_dialog(parent, "Pull Failed", &err);
                            update_actions();
                        }
                    }
                }
                WorkerEvent::BranchDone(result) => {
                    operation_busy.replace(false);
                    match result {
                        Ok(message) => {
                            status_label.set_text(&message);
                            update_actions();
                            trigger_refresh();
                        }
                        Err(err) => {
                            status_label.set_text(&err);
                            let parent = gtk::Application::default()
                                .active_window()
                                .and_then(|window| window.downcast::<gtk::Window>().ok());
                            open_git_feedback_dialog(parent, "Branch Action Failed", &err);
                            update_actions();
                        }
                    }
                }
                WorkerEvent::InitDone(result) => {
                    operation_busy.replace(false);
                    match result {
                        Ok(message) => {
                            no_repo_state.replace(false);
                            status_label.set_text(&message);
                            update_actions();
                            trigger_refresh();
                        }
                        Err(err) => {
                            status_label.set_text(&err);
                            update_actions();
                        }
                    }
                }
                WorkerEvent::UpstreamDone(result) => {
                    operation_busy.replace(false);
                    match result {
                        Ok(message) => {
                            status_label.set_text(&message);
                            update_actions();
                            trigger_refresh();
                        }
                        Err(err) => {
                            status_label.set_text(&err);
                            update_actions();
                        }
                    }
                }
            }
        }
        gtk::glib::ControlFlow::Continue
    });
}
