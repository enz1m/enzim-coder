#[allow(clippy::too_many_arguments)]
fn attach_inner(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    messages_box: gtk::Box,
    messages_scroll: gtk::ScrolledWindow,
    conversation_stack: gtk::Stack,
    suggestion_row: gtk::Box,
    track_background_completion: bool,
    active_codex_thread_id: Rc<RefCell<Option<String>>>,
    turn_uis: Rc<RefCell<HashMap<String, super::TurnUi>>>,
    item_turns: Rc<RefCell<HashMap<String, String>>>,
    item_kinds: Rc<RefCell<HashMap<String, String>>>,
    item_threads: Rc<RefCell<HashMap<String, String>>>,
    turn_threads: Rc<RefCell<HashMap<String, String>>>,
    active_turn: Rc<RefCell<Option<String>>>,
    active_turn_thread: Rc<RefCell<Option<String>>>,
    cached_commands_for_thread: Rc<RefCell<Vec<Value>>>,
    cached_file_changes_for_thread: Rc<RefCell<Vec<Value>>>,
    cached_tool_items_for_thread: Rc<RefCell<Vec<Value>>>,
    cached_pending_requests_for_thread: Rc<RefCell<Vec<Value>>>,
    cached_turn_errors_for_thread: Rc<RefCell<Vec<Value>>>,
    loaded_history_thread_id: Rc<RefCell<Option<String>>>,
    loading_history_thread_id: Rc<RefCell<Option<String>>>,
) {
    let (history_tx, history_rx) = mpsc::channel::<HistorySyncMessage>();
    let (history_snapshot_tx, history_snapshot_rx) = mpsc::channel::<(
        String,
        Result<super::codex_history::ThreadHistoryRenderSnapshot, String>,
    )>();
    let (event_tx, event_rx) = mpsc::channel::<(i64, AppServerNotification)>();
    let (checkpoint_tx, checkpoint_rx) = mpsc::channel::<(String, String, i64)>();
    register_shared_event_sink(event_tx);
    let staged_preimages_by_item: Rc<RefCell<HashMap<String, Value>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let staged_command_preimages_by_item: Rc<RefCell<HashMap<String, Value>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let staged_command_paths_by_item: Rc<RefCell<HashMap<String, Vec<String>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let pending_server_requests_by_id: Rc<RefCell<HashMap<i64, PendingServerRequestUi>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let pending_request_thread_by_id: Rc<RefCell<HashMap<i64, String>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let pending_history_snapshots: Rc<
        RefCell<HashMap<String, super::codex_history::ThreadHistoryRenderSnapshot>>,
    > = Rc::new(RefCell::new(HashMap::new()));
    let cached_history_snapshots: Rc<
        RefCell<HashMap<String, super::codex_history::ThreadHistoryRenderSnapshot>>,
    > = Rc::new(RefCell::new(HashMap::new()));
    let event_note_counter: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));
    let turns_with_write_like_commands: Rc<RefCell<HashSet<String>>> =
        Rc::new(RefCell::new(HashSet::new()));
    crate::ui::scheduler::every(Duration::from_millis(30), move || {
        if messages_box.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        ensure_shared_event_workers(&manager);

        let active_thread_now = active_codex_thread_id.borrow().clone();
        if let Some(thread_id) = active_thread_now.as_deref() {
            if take_history_reload_request(thread_id) {
                mark_history_dirty_for_thread(thread_id);
                pending_history_snapshots.borrow_mut().remove(thread_id);
                cached_history_snapshots.borrow_mut().remove(thread_id);
                loaded_history_thread_id.replace(None);
                loading_history_thread_id.replace(None);
            }
        }
        let loaded_thread = loaded_history_thread_id.borrow().clone();
        let loading_thread = loading_history_thread_id.borrow().clone();

        if active_thread_now != loaded_thread && active_thread_now != loading_thread {
            handle_active_thread_transition(
                &db,
                &manager,
                active_thread_now.clone(),
                &turn_uis,
                &turn_threads,
                &active_turn,
                &active_turn_thread,
                &pending_server_requests_by_id,
                &pending_request_thread_by_id,
                &messages_box,
                &messages_scroll,
                &conversation_stack,
                &suggestion_row,
                &cached_commands_for_thread,
                &cached_file_changes_for_thread,
                &cached_tool_items_for_thread,
                &cached_pending_requests_for_thread,
                &cached_turn_errors_for_thread,
                &pending_history_snapshots,
                &cached_history_snapshots,
                &loaded_history_thread_id,
                &loading_history_thread_id,
                &history_snapshot_tx,
                &history_tx,
            );
        }

        while let Ok((thread_id, snapshot_result)) = history_snapshot_rx.try_recv() {
            if loading_history_thread_id.borrow().as_deref() == Some(thread_id.as_str()) {
                loading_history_thread_id.replace(None);
            }
            match snapshot_result {
                Ok(snapshot) => {
                    pending_history_snapshots
                        .borrow_mut()
                        .insert(thread_id.clone(), snapshot.clone());
                    cached_history_snapshots
                        .borrow_mut()
                        .insert(thread_id, snapshot);
                }
                Err(err) => {
                    pending_history_snapshots.borrow_mut().remove(&thread_id);
                    eprintln!(
                        "[chat-load:{thread_id}] detached sqlite snapshot load failed: {err}"
                    );
                    if active_codex_thread_id.borrow().as_deref() == Some(thread_id.as_str()) {
                        super::clear_messages(&messages_box);
                        conversation_stack.set_visible_child_name("empty");
                        suggestion_row.set_visible(true);
                        loaded_history_thread_id.replace(Some(thread_id.clone()));
                        mark_history_load_finished(&thread_id, "detached sqlite snapshot failed");
                    }
                }
            }
        }

        include!("attach_history_poll_section.rs");

        while let Ok((thread_id, turn_id, checkpoint_id)) = checkpoint_rx.try_recv() {
            let active_thread = active_codex_thread_id.borrow().clone();
            append_checkpoint_strip_for_turn(
                &manager,
                &messages_box,
                &messages_scroll,
                &conversation_stack,
                active_thread.as_deref(),
                &thread_id,
                &turn_id,
                checkpoint_id,
            );
        }

        include!("attach_event_loop_section.rs");

        gtk::glib::ControlFlow::Continue
    });
}
