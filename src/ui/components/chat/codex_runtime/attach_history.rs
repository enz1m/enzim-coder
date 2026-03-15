#[derive(Clone, Copy)]
enum HistorySyncRenderPolicy {
    AllowRerender,
    DbOnly,
}

type HistorySyncMessage = (
    String,
    String,
    HistorySyncRenderPolicy,
    Result<Value, String>,
);

type PendingRequestSyncMessage = (String, Result<Vec<Value>, String>);

#[allow(clippy::too_many_arguments)]
fn replace_pending_requests_for_thread(
    db: &Rc<AppDb>,
    manager: &Rc<CodexProfileManager>,
    thread_id: &str,
    entries: Vec<Value>,
    turn_uis: &Rc<RefCell<HashMap<String, super::TurnUi>>>,
    pending_server_requests_by_id: &Rc<RefCell<HashMap<i64, PendingServerRequestUi>>>,
    pending_request_thread_by_id: &Rc<RefCell<HashMap<i64, String>>>,
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    cached_pending_requests_for_thread: &Rc<RefCell<Vec<Value>>>,
) {
    let request_ids_for_thread = pending_request_thread_by_id
        .borrow()
        .iter()
        .filter_map(|(request_id, owner_thread_id)| {
            if owner_thread_id == thread_id {
                Some(*request_id)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for request_id in request_ids_for_thread {
        pending_request_thread_by_id.borrow_mut().remove(&request_id);
        if let Some(pending) = pending_server_requests_by_id
            .borrow_mut()
            .remove(&request_id)
        {
            remove_request_card(turn_uis, &pending.turn_id, &pending.card);
        }
    }

    cached_pending_requests_for_thread.replace(entries.clone());
    super::codex_history::save_cached_pending_requests(db, thread_id, &entries);

    let Some(client) = manager.resolve_running_client_for_thread_id(thread_id) else {
        return;
    };
    for pending in &entries {
        let Some(request_id) = pending.get("requestId").and_then(Value::as_i64) else {
            continue;
        };
        let Some(turn_id) = pending.get("turnId").and_then(Value::as_str) else {
            continue;
        };
        let method = pending
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("item/tool/requestUserInput");
        let params = pending
            .get("params")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        pending_request_thread_by_id
            .borrow_mut()
            .insert(request_id, thread_id.to_string());
        show_pending_request_card(
            &client,
            turn_uis,
            pending_server_requests_by_id,
            messages_box,
            messages_scroll,
            conversation_stack,
            thread_id,
            turn_id,
            request_id,
            method,
            &params,
            false,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_active_thread_transition(
    db: &Rc<AppDb>,
    manager: &Rc<CodexProfileManager>,
    active_thread_now: Option<String>,
    turn_uis: &Rc<RefCell<HashMap<String, super::TurnUi>>>,
    turn_threads: &Rc<RefCell<HashMap<String, String>>>,
    active_turn: &Rc<RefCell<Option<String>>>,
    active_turn_thread: &Rc<RefCell<Option<String>>>,
    pending_server_requests_by_id: &Rc<RefCell<HashMap<i64, PendingServerRequestUi>>>,
    pending_request_thread_by_id: &Rc<RefCell<HashMap<i64, String>>>,
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    suggestion_row: &gtk::Box,
    cached_commands_for_thread: &Rc<RefCell<Vec<Value>>>,
    cached_file_changes_for_thread: &Rc<RefCell<Vec<Value>>>,
    cached_tool_items_for_thread: &Rc<RefCell<Vec<Value>>>,
    cached_pending_requests_for_thread: &Rc<RefCell<Vec<Value>>>,
    cached_turn_errors_for_thread: &Rc<RefCell<Vec<Value>>>,
    pending_history_snapshots: &Rc<
        RefCell<HashMap<String, super::codex_history::ThreadHistoryRenderSnapshot>>,
    >,
    cached_history_snapshots: &Rc<
        RefCell<HashMap<String, super::codex_history::ThreadHistoryRenderSnapshot>>,
    >,
    loaded_history_thread_id: &Rc<RefCell<Option<String>>>,
    loading_history_thread_id: &Rc<RefCell<Option<String>>>,
    history_snapshot_tx:
        &mpsc::Sender<(String, Result<super::codex_history::ThreadHistoryRenderSnapshot, String>)>,
    history_tx: &mpsc::Sender<HistorySyncMessage>,
    pending_request_tx: &mpsc::Sender<PendingRequestSyncMessage>,
) {
    super::clear_messages(messages_box);

    if let Some(thread_id) = active_thread_now.clone() {
        mark_history_load_started(&thread_id, "thread switch");
        super::message_render::set_messages_box_thread_context(messages_box, Some(&thread_id));
        let thread_locked = db.is_remote_thread_locked(&thread_id).unwrap_or(false);

        let mut preloaded_snapshot = pending_history_snapshots
            .borrow_mut()
            .remove(&thread_id)
            .or_else(|| cached_history_snapshots.borrow().get(&thread_id).cloned());
        if preloaded_snapshot.is_none() {
            conversation_stack.set_visible_child_name("loading");
            suggestion_row.set_visible(false);
            cached_commands_for_thread.replace(Vec::new());
            cached_file_changes_for_thread.replace(Vec::new());
            cached_tool_items_for_thread.replace(Vec::new());
            cached_pending_requests_for_thread.replace(Vec::new());
            cached_turn_errors_for_thread.replace(Vec::new());
            loaded_history_thread_id.replace(None);
            loading_history_thread_id.replace(Some(thread_id.clone()));
            let thread_id_for_worker = thread_id.clone();
            let history_snapshot_tx = history_snapshot_tx.clone();
            thread::spawn(move || {
                let result = super::codex_history::load_thread_history_render_snapshot_detached(
                    &thread_id_for_worker,
                );
                let _ = history_snapshot_tx.send((thread_id_for_worker, result));
            });
            return;
        }

        if let Some(snapshot) = preloaded_snapshot.as_ref() {
            cached_history_snapshots
                .borrow_mut()
                .insert(thread_id.clone(), snapshot.clone());
            cached_commands_for_thread.replace(snapshot.caches.commands.clone());
            cached_file_changes_for_thread.replace(snapshot.caches.file_changes.clone());
            cached_tool_items_for_thread.replace(snapshot.caches.tool_items.clone());
            cached_pending_requests_for_thread.replace(snapshot.caches.pending_requests.clone());
            cached_turn_errors_for_thread.replace(snapshot.caches.turn_errors.clone());
        } else {
            cached_commands_for_thread
                .replace(super::codex_history::load_cached_commands(db, &thread_id));
            cached_file_changes_for_thread.replace(super::codex_history::load_cached_file_changes(
                db, &thread_id,
            ));
            cached_tool_items_for_thread
                .replace(super::codex_history::load_cached_tool_items(db, &thread_id));
            cached_pending_requests_for_thread.replace(
                super::codex_history::load_cached_pending_requests(db, &thread_id),
            );
            cached_turn_errors_for_thread.replace(super::codex_history::load_cached_turn_errors(
                db, &thread_id,
            ));
        }

        let has_local_history = super::codex_history::render_local_thread_history_from_db(
            db,
            Some(manager.clone()),
            messages_box,
            messages_scroll,
            conversation_stack,
            suggestion_row,
            &thread_id,
            preloaded_snapshot.take(),
        );
        log_history_load_step(
            &thread_id,
            if has_local_history {
                "scheduled local sqlite render"
            } else {
                "no local sqlite history found"
            },
        );
        if let Some(shared_turn_id) = active_turn_for_thread(&thread_id) {
            turn_threads
                .borrow_mut()
                .insert(shared_turn_id.clone(), thread_id.clone());
            active_turn.replace(Some(shared_turn_id.clone()));
            active_turn_thread.replace(Some(thread_id.clone()));
            let mut turns = turn_uis.borrow_mut();
            let turn_ui = turns.entry(shared_turn_id).or_insert_with(|| {
                super::create_turn_ui(messages_box, messages_scroll, conversation_stack)
            });
            turn_ui.in_progress = true;
            if turn_ui.status_label.text().trim().is_empty() {
                turn_ui.status_label.set_text("Working...");
            }
            super::refresh_turn_status(turn_ui);
        }
        let thread_history_dirty = history_is_dirty_for_thread(&thread_id);

        for pending in cached_pending_requests_for_thread.borrow().iter() {
            let Some(request_id) = pending.get("requestId").and_then(Value::as_i64) else {
                continue;
            };
            let Some(turn_id) = pending.get("turnId").and_then(Value::as_str) else {
                continue;
            };
            let method = pending
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("item/tool/requestUserInput");
            let params = pending
                .get("params")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            pending_request_thread_by_id
                .borrow_mut()
                .insert(request_id, thread_id.clone());
            let Some(client_for_thread) = manager.resolve_running_client_for_thread_id(&thread_id)
            else {
                continue;
            };
            show_pending_request_card(
                &client_for_thread,
                turn_uis,
                pending_server_requests_by_id,
                messages_box,
                messages_scroll,
                conversation_stack,
                &thread_id,
                turn_id,
                request_id,
                method,
                &params,
                true,
            );
        }

        if let Some(client) = manager.resolve_running_client_for_thread_id(&thread_id) {
            if client.backend_kind() == "opencode" {
                let pending_request_tx = pending_request_tx.clone();
                let thread_id_for_worker = thread_id.clone();
                thread::spawn(move || {
                    let result = client.pending_server_requests_for_thread(&thread_id_for_worker);
                    let _ = pending_request_tx.send((thread_id_for_worker, result));
                });
            }
        }

        if thread_locked {
            loading_history_thread_id.replace(None);
            loaded_history_thread_id.replace(Some(thread_id.clone()));
            log_history_load_step(&thread_id, "thread locked; skipping appserver history sync");
        } else if history_is_unavailable_for_thread(&thread_id) {
            loading_history_thread_id.replace(None);
            loaded_history_thread_id.replace(Some(thread_id.clone()));
            log_history_load_step(
                &thread_id,
                "history unavailable for thread; skipping appserver history sync",
            );
        } else if !has_local_history || thread_history_dirty {
            let Some(client) = manager.resolve_running_client_for_thread_id(&thread_id) else {
                mark_history_unavailable_for_thread(&thread_id);
                loading_history_thread_id.replace(None);
                loaded_history_thread_id.replace(Some(thread_id.clone()));
                return;
            };
            loading_history_thread_id.replace(Some(thread_id.clone()));
            if has_local_history {
                loaded_history_thread_id.replace(Some(thread_id.clone()));
            } else {
                loaded_history_thread_id.replace(None);
            }
            let history_tx = history_tx.clone();
            log_history_load_step(&thread_id, "dispatching appserver thread/read");
            let log_source = db
                .get_thread_record_by_remote_thread_id(&thread_id)
                .ok()
                .flatten()
                .and_then(|thread| {
                    db.get_codex_profile(thread.profile_id)
                        .ok()
                        .flatten()
                        .map(|profile| format!("{}#{}", profile.name, profile.id))
                })
                .unwrap_or_else(|| "unknown".to_string());
            let thread_id_for_read = thread_id.clone();
            thread::spawn(move || {
                let result = client.thread_read(&thread_id_for_read, true);
                let _ = history_tx.send((
                    thread_id_for_read,
                    log_source,
                    HistorySyncRenderPolicy::AllowRerender,
                    result,
                ));
            });
        } else {
            loading_history_thread_id.replace(None);
            loaded_history_thread_id.replace(Some(thread_id.clone()));
        }
    } else {
        super::message_render::set_messages_box_thread_context(messages_box, None);
        super::clear_messages(messages_box);
        suggestion_row.set_visible(true);
        cached_commands_for_thread.replace(Vec::new());
        cached_file_changes_for_thread.replace(Vec::new());
        cached_tool_items_for_thread.replace(Vec::new());
        cached_pending_requests_for_thread.replace(Vec::new());
        cached_turn_errors_for_thread.replace(Vec::new());
        loading_history_thread_id.replace(None);
        loaded_history_thread_id.replace(None);
        conversation_stack.set_visible_child_name("empty");
    }
}
