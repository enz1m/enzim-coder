use crate::backend::RuntimeClient;
use crate::codex_profiles::CodexProfileManager;
use crate::data::{AppDb, LocalChatTurnInput, LocalChatTurnRecord};
use crate::restore::RestoreCheckpoint;
use crate::ui::components::restore_preview;
use gtk::prelude::*;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn user_row_marker(turn_id: &str) -> String {
    format!("turn-user-row:{turn_id}")
}

thread_local! {
    static LOCAL_HISTORY_RENDER_GENERATION: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
    static LOCAL_HISTORY_LAZY_STATE: RefCell<HashMap<usize, Rc<RefCell<LocalHistoryLazyState>>>> =
        RefCell::new(HashMap::new());
    static LOCAL_HISTORY_SCROLL_WATCHERS: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

const INITIAL_VISIBLE_TURNS: usize = 5;
const OLDER_TURNS_CHUNK: usize = 5;
const OLDER_PREFETCH_PX: f64 = 260.0;
const BOTTOM_PRUNE_THRESHOLD_PX: f64 = 18.0;

fn trim_process_heap_after_prune() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

struct LocalHistoryLazyState {
    thread_id: String,
    generation: u64,
    manager: Option<Rc<CodexProfileManager>>,
    turns: Rc<Vec<LocalChatTurnRecord>>,
    checkpoint_by_turn: Rc<HashMap<String, i64>>,
    cached_commands_by_turn: Rc<HashMap<String, Vec<Value>>>,
    cached_file_changes_by_turn: Rc<HashMap<String, Vec<Value>>>,
    cached_tool_items_by_turn: Rc<HashMap<String, Vec<Value>>>,
    cached_turn_errors_by_turn: Rc<HashMap<String, String>>,
    initial_start: usize,
    rendered_start: usize,
    lazy_loaded_row_count: usize,
    loading_older: bool,
    pruning_older: bool,
}

#[derive(Clone)]
pub(super) struct ThreadHistoryRenderCaches {
    pub commands: Vec<Value>,
    pub file_changes: Vec<Value>,
    pub tool_items: Vec<Value>,
    pub pending_requests: Vec<Value>,
    pub turn_errors: Vec<Value>,
}

#[derive(Clone)]
pub(super) struct ThreadHistoryRenderSnapshot {
    pub turns: Vec<LocalChatTurnRecord>,
    pub checkpoint_by_turn: HashMap<String, i64>,
    pub caches: ThreadHistoryRenderCaches,
}

fn local_history_render_key(messages_box: &gtk::Box) -> usize {
    messages_box.as_ptr() as usize
}

fn begin_local_history_render(messages_box: &gtk::Box) -> u64 {
    let key = local_history_render_key(messages_box);
    LOCAL_HISTORY_RENDER_GENERATION.with(|generations| {
        let mut generations = generations.borrow_mut();
        let next = generations.get(&key).copied().unwrap_or(0).wrapping_add(1);
        generations.insert(key, next);
        next
    })
}

fn local_history_render_is_current(messages_box: &gtk::Box, generation: u64) -> bool {
    let key = local_history_render_key(messages_box);
    LOCAL_HISTORY_RENDER_GENERATION
        .with(|generations| generations.borrow().get(&key).copied().unwrap_or(0) == generation)
}

fn group_cached_entries_by_turn_id(entries: Vec<Value>) -> HashMap<String, Vec<Value>> {
    let mut grouped = HashMap::new();
    for entry in entries {
        if let Some(turn_id) = entry.get("turnId").and_then(Value::as_str) {
            grouped
                .entry(turn_id.to_string())
                .or_insert_with(Vec::new)
                .push(entry);
        }
    }
    grouped
}

fn turn_sort_timestamp(turn: &LocalChatTurnRecord) -> i64 {
    turn.completed_at.unwrap_or(turn.created_at)
}

fn checkpoint_sort_timestamp(checkpoint: &RestoreCheckpoint) -> i64 {
    checkpoint.created_at
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

fn resolve_checkpoint_map_for_turns(
    turns: &[LocalChatTurnRecord],
    checkpoints: Vec<RestoreCheckpoint>,
) -> HashMap<String, i64> {
    let turn_ids = turns
        .iter()
        .map(|turn| turn.external_turn_id.as_str())
        .collect::<HashSet<_>>();

    let mut resolved = HashMap::new();
    let mut unmatched_checkpoints = Vec::new();
    for checkpoint in checkpoints {
        if turn_ids.contains(checkpoint.turn_id.as_str()) {
            resolved.insert(checkpoint.turn_id.clone(), checkpoint.id);
        } else {
            unmatched_checkpoints.push(checkpoint);
        }
    }

    if unmatched_checkpoints.is_empty() {
        return resolved;
    }

    let mut unmatched_turns = turns
        .iter()
        .filter(|turn| !resolved.contains_key(turn.external_turn_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    unmatched_turns.sort_by(|a, b| {
        turn_sort_timestamp(a)
            .cmp(&turn_sort_timestamp(b))
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.external_turn_id.cmp(&b.external_turn_id))
    });
    unmatched_checkpoints.sort_by(|a, b| {
        checkpoint_sort_timestamp(a)
            .cmp(&checkpoint_sort_timestamp(b))
            .then_with(|| a.id.cmp(&b.id))
    });

    for (turn, checkpoint) in unmatched_turns.into_iter().zip(unmatched_checkpoints.into_iter()) {
        resolved.insert(turn.external_turn_id, checkpoint.id);
    }

    resolved
}

fn map_cached_turn_errors_by_turn_id(entries: Vec<Value>) -> HashMap<String, String> {
    let mut mapped = HashMap::new();
    for entry in entries {
        let Some(turn_id) = entry.get("turnId").and_then(Value::as_str) else {
            continue;
        };
        let Some(message) = entry.get("message").and_then(Value::as_str) else {
            continue;
        };
        mapped.insert(turn_id.to_string(), message.to_string());
    }
    mapped
}

fn set_local_history_lazy_state(
    messages_box: &gtk::Box,
    state: Rc<RefCell<LocalHistoryLazyState>>,
) {
    let key = local_history_render_key(messages_box);
    LOCAL_HISTORY_LAZY_STATE.with(|states| {
        states.borrow_mut().insert(key, state);
    });
}

fn get_local_history_lazy_state(
    messages_box: &gtk::Box,
) -> Option<Rc<RefCell<LocalHistoryLazyState>>> {
    let key = local_history_render_key(messages_box);
    LOCAL_HISTORY_LAZY_STATE.with(|states| states.borrow().get(&key).cloned())
}

fn clear_local_history_lazy_state(messages_box: &gtk::Box) {
    let key = local_history_render_key(messages_box);
    LOCAL_HISTORY_LAZY_STATE.with(|states| {
        states.borrow_mut().remove(&key);
    });
}

fn collect_children_in_order(container: &gtk::Box) -> Vec<gtk::Widget> {
    let mut children = Vec::new();
    let mut child = container.first_child();
    while let Some(node) = child {
        children.push(node.clone());
        child = node.next_sibling();
    }
    children
}

fn prepend_staged_children(messages_box: &gtk::Box, staging: &gtk::Box) -> usize {
    let children = collect_children_in_order(staging);
    let count = children.len();
    for child in children.into_iter().rev() {
        if let Ok(row) = child.clone().downcast::<gtk::Box>() {
            row.set_margin_top(0);
        }
        staging.remove(&child);
        messages_box.prepend(&child);
    }
    count
}

fn render_history_turn_range(
    target_box: &gtk::Box,
    conversation_stack: &gtk::Stack,
    thread_id: &str,
    manager: &Option<Rc<CodexProfileManager>>,
    turns: &[LocalChatTurnRecord],
    checkpoint_by_turn: &HashMap<String, i64>,
    cached_commands_by_turn: &HashMap<String, Vec<Value>>,
    cached_file_changes_by_turn: &HashMap<String, Vec<Value>>,
    cached_tool_items_by_turn: &HashMap<String, Vec<Value>>,
    cached_turn_errors_by_turn: &HashMap<String, String>,
) -> bool {
    let mut has_any = false;
    for turn in turns {
        if render_local_turn(
            turn,
            manager,
            target_box,
            conversation_stack,
            thread_id,
            checkpoint_by_turn,
            cached_commands_by_turn,
            cached_file_changes_by_turn,
            cached_tool_items_by_turn,
            cached_turn_errors_by_turn,
        ) {
            has_any = true;
        }
    }
    has_any
}

fn load_older_history_chunk(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    suggestion_row: &gtk::Box,
    state_rc: &Rc<RefCell<LocalHistoryLazyState>>,
) {
    let thread_id_current = super::message_render::chat_thread_id_for_messages_box(messages_box);
    let mut state = state_rc.borrow_mut();
    if thread_id_current.as_deref() != Some(state.thread_id.as_str())
        || !local_history_render_is_current(messages_box, state.generation)
    {
        state.loading_older = false;
        return;
    }
    if state.rendered_start == 0 {
        state.loading_older = false;
        return;
    }

    let old_start = state.rendered_start;
    let new_start = old_start.saturating_sub(OLDER_TURNS_CHUNK);
    let old_upper = messages_scroll.vadjustment().upper();
    let old_value = messages_scroll.vadjustment().value();

    let staging = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let chunk_turns = &state.turns[new_start..old_start];
    let _ = render_history_turn_range(
        &staging,
        conversation_stack,
        &state.thread_id,
        &state.manager,
        chunk_turns,
        &state.checkpoint_by_turn,
        &state.cached_commands_by_turn,
        &state.cached_file_changes_by_turn,
        &state.cached_tool_items_by_turn,
        &state.cached_turn_errors_by_turn,
    );
    let loaded_rows = prepend_staged_children(messages_box, &staging);
    state.lazy_loaded_row_count = state.lazy_loaded_row_count.saturating_add(loaded_rows);
    state.rendered_start = new_start;
    drop(state);
    super::message_render::set_chat_reasoning_visibility(
        messages_box,
        super::message_render::messages_reasoning_visible(messages_box),
    );

    let adj = messages_scroll.vadjustment();
    let state_for_finish = state_rc.clone();
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(16), move || {
        let new_upper = adj.upper();
        let delta = (new_upper - old_upper).max(0.0);
        let target = old_value + delta;
        let max = (adj.upper() - adj.page_size()).max(adj.lower());
        adj.set_value(target.clamp(adj.lower(), max));

        state_for_finish.borrow_mut().loading_older = false;
    });
    suggestion_row.set_visible(false);
}

fn prune_loaded_history_rows(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    state_rc: &Rc<RefCell<LocalHistoryLazyState>>,
) {
    let thread_id_current = super::message_render::chat_thread_id_for_messages_box(messages_box);
    let rows_to_remove = {
        let mut state = state_rc.borrow_mut();
        if thread_id_current.as_deref() != Some(state.thread_id.as_str())
            || !local_history_render_is_current(messages_box, state.generation)
        {
            state.pruning_older = false;
            return;
        }
        if state.lazy_loaded_row_count == 0 {
            state.pruning_older = false;
            return;
        }
        let rows_to_remove = state.lazy_loaded_row_count;
        state.lazy_loaded_row_count = 0;
        state.rendered_start = state.initial_start;
        state.loading_older = false;
        state.pruning_older = false;
        rows_to_remove
    };

    for _ in 0..rows_to_remove {
        let Some(first) = messages_box.first_child() else {
            break;
        };
        messages_box.remove(&first);
    }

    let adj = messages_scroll.vadjustment();
    gtk::glib::idle_add_local_once(move || {
        let lower = adj.lower();
        let max = (adj.upper() - adj.page_size()).max(lower);
        adj.set_value(max);
    });

    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(120), || {
        trim_process_heap_after_prune();
    });
}

fn maybe_schedule_load_older_history(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    suggestion_row: &gtk::Box,
) {
    let Some(state_rc) = get_local_history_lazy_state(messages_box) else {
        return;
    };

    {
        let mut state = state_rc.borrow_mut();
        if state.loading_older || state.rendered_start == 0 {
            return;
        }
        let adj = messages_scroll.vadjustment();
        if adj.value() > OLDER_PREFETCH_PX {
            return;
        }
        state.loading_older = true;
    }

    let messages_box_c = messages_box.clone();
    let messages_scroll_c = messages_scroll.clone();
    let conversation_stack_c = conversation_stack.clone();
    let suggestion_row_c = suggestion_row.clone();
    let state_for_idle = state_rc.clone();
    gtk::glib::idle_add_local_once(move || {
        load_older_history_chunk(
            &messages_box_c,
            &messages_scroll_c,
            &conversation_stack_c,
            &suggestion_row_c,
            &state_for_idle,
        );
    });
}

fn maybe_schedule_prune_loaded_history(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
) {
    let Some(state_rc) = get_local_history_lazy_state(messages_box) else {
        return;
    };

    {
        let mut state = state_rc.borrow_mut();
        if state.pruning_older || state.loading_older || state.lazy_loaded_row_count == 0 {
            return;
        }
        let adj = messages_scroll.vadjustment();
        let lower = adj.lower();
        let max = (adj.upper() - adj.page_size()).max(lower);
        let distance_from_bottom = (max - adj.value()).max(0.0);
        if distance_from_bottom > BOTTOM_PRUNE_THRESHOLD_PX {
            return;
        }
        state.pruning_older = true;
    }

    let messages_box_c = messages_box.clone();
    let messages_scroll_c = messages_scroll.clone();
    let state_for_idle = state_rc.clone();
    gtk::glib::idle_add_local_once(move || {
        prune_loaded_history_rows(&messages_box_c, &messages_scroll_c, &state_for_idle);
    });
}

fn ensure_local_history_scroll_watcher(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    suggestion_row: &gtk::Box,
) {
    let scroll_key = messages_scroll.as_ptr() as usize;
    let should_connect = LOCAL_HISTORY_SCROLL_WATCHERS.with(|watchers| {
        let mut watchers = watchers.borrow_mut();
        if watchers.contains(&scroll_key) {
            false
        } else {
            watchers.insert(scroll_key);
            true
        }
    });
    if !should_connect {
        return;
    }

    let messages_box_c = messages_box.clone();
    let messages_scroll_c = messages_scroll.clone();
    let conversation_stack_c = conversation_stack.clone();
    let suggestion_row_c = suggestion_row.clone();
    messages_scroll
        .vadjustment()
        .connect_value_changed(move |_| {
            maybe_schedule_load_older_history(
                &messages_box_c,
                &messages_scroll_c,
                &conversation_stack_c,
                &suggestion_row_c,
            );
            maybe_schedule_prune_loaded_history(&messages_box_c, &messages_scroll_c);
        });

    messages_scroll.connect_destroy(move |_| {
        LOCAL_HISTORY_SCROLL_WATCHERS.with(|watchers| {
            watchers.borrow_mut().remove(&scroll_key);
        });
    });
}

#[derive(Default, Clone)]
struct UserMessagePayload {
    text: String,
    local_image_paths: Vec<String>,
    unresolved_image_count: usize,
}

impl UserMessagePayload {
    fn has_content(&self) -> bool {
        !self.text.trim().is_empty()
            || !self.local_image_paths.is_empty()
            || self.unresolved_image_count > 0
    }

    fn render_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.text.trim().is_empty() {
            parts.push(self.text.clone());
        }
        for _ in 0..self.unresolved_image_count {
            parts.push("[image]".to_string());
        }
        parts.join("\n")
    }

    fn storage_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.text.trim().is_empty() {
            parts.push(self.text.clone());
        }
        for _ in 0..(self.local_image_paths.len() + self.unresolved_image_count) {
            parts.push("[image]".to_string());
        }
        parts.join("\n")
    }

    fn merge_from(&mut self, other: UserMessagePayload) {
        if !other.text.trim().is_empty() {
            if !self.text.trim().is_empty() {
                self.text.push('\n');
            }
            self.text.push_str(&other.text);
        }
        for path in other.local_image_paths {
            if !self
                .local_image_paths
                .iter()
                .any(|existing| existing == &path)
            {
                self.local_image_paths.push(path);
            }
        }
        self.unresolved_image_count += other.unresolved_image_count;
    }
}

fn extract_user_message_payload(item: &Value) -> Option<UserMessagePayload> {
    if item.get("type").and_then(Value::as_str) != Some("userMessage") {
        return None;
    }
    let content = item.get("content").and_then(Value::as_array)?;
    let mut payload = UserMessagePayload::default();
    for entry in content {
        match entry.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = entry.get("text").and_then(Value::as_str) {
                    if !payload.text.is_empty() {
                        payload.text.push('\n');
                    }
                    payload.text.push_str(text);
                }
            }
            Some("localImage") => {
                if let Some(path) = entry.get("path").and_then(Value::as_str) {
                    payload.local_image_paths.push(path.to_string());
                } else {
                    payload.unresolved_image_count += 1;
                }
            }
            Some("image") => {
                if let Some(url) = entry.get("url").and_then(Value::as_str) {
                    if let Ok((path, _)) = gtk::glib::filename_from_uri(url) {
                        payload
                            .local_image_paths
                            .push(path.to_string_lossy().to_string());
                    } else {
                        payload.unresolved_image_count += 1;
                    }
                } else {
                    payload.unresolved_image_count += 1;
                }
            }
            _ => {}
        }
    }
    if !payload.has_content() {
        None
    } else {
        Some(payload)
    }
}

fn extract_user_message_payload_from_items(items: &[Value]) -> Option<UserMessagePayload> {
    let mut merged = UserMessagePayload::default();
    for item in items {
        if let Some(payload) = extract_user_message_payload(item) {
            merged.merge_from(payload);
        }
    }
    merged.has_content().then_some(merged)
}

fn append_command_from_value(body_box: &gtk::Box, value: &Value) {
    let command = value
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("command");
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    let exit_code = value.get("exitCode").and_then(Value::as_i64);
    let duration_ms = value.get("durationMs").and_then(Value::as_i64);
    let (widget, mut command_ui) = super::message_render::create_command_widget(command);
    command_ui.set_command_headline(command);
    command_ui.set_command_status_label(&super::codex_events::format_command_status_label(
        status,
        exit_code,
        duration_ms,
    ));
    if let Some(output) = value
        .get("aggregatedOutput")
        .and_then(Value::as_str)
        .or_else(|| value.get("output").and_then(Value::as_str))
    {
        command_ui.set_command_output(output);
    }
    command_ui.revealer.set_reveal_child(false);
    super::message_render::append_action_widget(body_box, "commandExecution", &widget);
}

fn append_cached_commands_for_text_count<'a>(
    body_box: &gtk::Box,
    cached_by_text_count: &mut HashMap<usize, Vec<&'a Value>>,
    text_count: usize,
    has_content: &mut bool,
) {
    if let Some(cached_entries) = cached_by_text_count.remove(&text_count) {
        for cached in cached_entries {
            append_command_from_value(body_box, cached);
            *has_content = true;
        }
    }
}

fn append_tool_item_from_value(body_box: &gtk::Box, value: &Value) -> bool {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match kind {
        "dynamicToolCall" => {
            let (tool_name, arguments, status, output) =
                super::codex_events::extract_dynamic_tool_call_fields(value);
            let (widget, tool_ui) =
                super::message_render::create_tool_call_widget(&tool_name, &arguments);
            tool_ui.status_label.set_text(if status == "failed" {
                "Failed"
            } else {
                "Completed"
            });
            tool_ui.set_output(&output);
            super::message_render::append_action_widget(body_box, "dynamicToolCall", &widget);
            true
        }
        "webSearch" | "webFetch" | "fileRead" | "fileSearch" | "directoryList" | "codeSearch"
        | "skillCall" | "todoList" | "questionTool" | "mcpToolCall" | "collabToolCall"
        | "imageView" | "enteredReviewMode" | "exitedReviewMode" | "contextCompaction" => {
            let (section, title, summary, status, output) =
                super::codex_events::extract_generic_item_fields(value);
            let (widget, generic_ui) =
                super::message_render::create_generic_item_widget(&section, &title, &summary);
            generic_ui.set_title(&title);
            generic_ui.set_details(&summary, &output);
            generic_ui.set_running(status == "running");
            super::message_render::append_action_widget(body_box, kind, &widget);
            true
        }
        _ => false,
    }
}

fn append_cached_tool_items_for_text_count<'a>(
    body_box: &gtk::Box,
    cached_by_text_count: &mut HashMap<usize, Vec<&'a Value>>,
    text_count: usize,
    has_content: &mut bool,
) {
    if let Some(cached_entries) = cached_by_text_count.remove(&text_count) {
        for cached in cached_entries {
            if append_tool_item_from_value(body_box, cached) {
                *has_content = true;
            }
        }
    }
}

include!("codex_history/assistant_turn.rs");
#[allow(dead_code)]
pub(super) fn render_thread_history(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    thread: &Value,
    cached_commands: &[Value],
    cached_file_changes: &[Value],
    cached_tool_items: &[Value],
    cached_turn_errors: &[Value],
) -> bool {
    super::clear_messages(messages_box);
    let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
        conversation_stack.set_visible_child_name("empty");
        return false;
    };
    let cached_commands_by_turn = group_cached_entries_by_turn_id(cached_commands.to_vec());
    let cached_file_changes_by_turn = group_cached_entries_by_turn_id(cached_file_changes.to_vec());
    let cached_tool_items_by_turn = group_cached_entries_by_turn_id(cached_tool_items.to_vec());
    let cached_turn_errors_by_turn = map_cached_turn_errors_by_turn_id(cached_turn_errors.to_vec());

    let mut has_any = false;
    for turn in turns {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        let turn_id = turn.get("id").and_then(Value::as_str).unwrap_or("");
        let turn_cached_commands = cached_commands_by_turn
            .get(turn_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let turn_cached_file_changes = cached_file_changes_by_turn
            .get(turn_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let turn_cached_tool_items = cached_tool_items_by_turn
            .get(turn_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let turn_cached_error = cached_turn_errors_by_turn.get(turn_id).map(String::as_str);

        if let Some(payload) = extract_user_message_payload_from_items(items) {
            let user_content = super::message_render::append_user_message_with_images(
                messages_box,
                None,
                conversation_stack,
                &payload.render_text(),
                &payload.local_image_paths,
                to_system_time(parse_timestamp(turn.get("createdAt"))),
            );
            let _ = super::message_render::set_message_row_marker(
                &user_content,
                &user_row_marker(turn_id),
            );
            has_any = true;
        }

        let turn_error_from_server = turn
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str);
        let turn_error = turn_cached_error.or(turn_error_from_server);
        if append_history_assistant_turn(
            messages_box,
            conversation_stack,
            items,
            turn_cached_commands,
            turn_cached_file_changes,
            turn_cached_tool_items,
            turn_error,
            to_system_time(
                parse_timestamp_opt(turn.get("completedAt").filter(|v| !v.is_null()))
                    .unwrap_or_else(|| parse_timestamp(turn.get("createdAt"))),
            ),
        ) {
            has_any = true;
        }
    }

    if has_any {
        messages_box.set_opacity(0.0);
        conversation_stack.set_visible_child_name("messages");
        super::message_render::scroll_to_bottom(messages_scroll);

        let mb = messages_box.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(55), move || {
            mb.set_opacity(1.0);
        });
    } else {
        conversation_stack.set_visible_child_name("empty");
    }
    has_any
}

#[allow(dead_code)]
pub(super) fn thread_has_user_messages(thread: &Value) -> bool {
    thread
        .get("turns")
        .and_then(Value::as_array)
        .map(|turns| {
            turns.iter().any(|turn| {
                turn.get("items")
                    .and_then(Value::as_array)
                    .map(|items| extract_user_message_payload_from_items(items).is_some())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn to_system_time(ts: i64) -> SystemTime {
    if ts > 1_000_000_000_000 {
        UNIX_EPOCH + std::time::Duration::from_millis(ts as u64)
    } else {
        UNIX_EPOCH + std::time::Duration::from_secs(ts as u64)
    }
}

fn parse_timestamp_opt(value: Option<&Value>) -> Option<i64> {
    if let Some(num) = value.and_then(Value::as_i64) {
        return Some(num);
    }
    if let Some(s) = value.and_then(Value::as_str) {
        if let Ok(dt) = gtk::glib::DateTime::from_iso8601(s, None) {
            return Some(dt.to_unix());
        }
    }
    None
}

fn raw_items_may_include_uncached_actions(raw: &str) -> bool {
    raw.contains("\"commandExecution\"")
        || raw.contains("\"fileChange\"")
        || raw.contains("\"dynamicToolCall\"")
        || raw.contains("\"webSearch\"")
        || raw.contains("\"webFetch\"")
        || raw.contains("\"fileRead\"")
        || raw.contains("\"fileSearch\"")
        || raw.contains("\"directoryList\"")
        || raw.contains("\"codeSearch\"")
        || raw.contains("\"skillCall\"")
        || raw.contains("\"todoList\"")
        || raw.contains("\"questionTool\"")
        || raw.contains("\"mcpToolCall\"")
        || raw.contains("\"collabToolCall\"")
        || raw.contains("\"imageView\"")
        || raw.contains("\"enteredReviewMode\"")
        || raw.contains("\"exitedReviewMode\"")
        || raw.contains("\"contextCompaction\"")
}

fn raw_items_may_include_local_images(raw: &str) -> bool {
    raw.contains("\"localImage\"")
        || raw.contains("\"type\":\"image\"")
        || raw.contains("\"type\": \"image\"")
}

fn should_parse_raw_items_for_history_turn(
    turn: &LocalChatTurnRecord,
    raw_items_json: &str,
    has_cached_actions: bool,
    max_cached_after_text_count: usize,
) -> bool {
    if raw_items_json.trim().is_empty() {
        return false;
    }

    if turn.user_text.trim().is_empty() || turn.assistant_text.trim().is_empty() {
        return true;
    }

    if raw_items_may_include_local_images(raw_items_json) {
        return true;
    }

    if has_cached_actions && max_cached_after_text_count > 1 {
        return true;
    }

    !has_cached_actions && raw_items_may_include_uncached_actions(raw_items_json)
}

#[allow(dead_code)]
fn parse_timestamp(value: Option<&Value>) -> i64 {
    parse_timestamp_opt(value).unwrap_or_else(unix_now)
}

fn extract_assistant_turn_text(items: &[Value]) -> String {
    let mut parts = Vec::new();
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
            continue;
        }
        if let Some(text) = super::codex_events::extract_agent_message_text(item) {
            if !text.trim().is_empty() {
                parts.push(text);
            }
        }
    }
    parts.join("\n")
}

pub(super) fn create_checkpoint_strip_widget(
    thread_id: &str,
    checkpoint_id: i64,
    turn_id: &str,
    _user_prompt: Option<&str>,
    manager: Option<Rc<CodexProfileManager>>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_widget_name(&format!("checkpoint-strip:{turn_id}"));
    row.add_css_class("chat-checkpoint-row");
    row.set_margin_start(0);
    row.set_margin_end(0);
    row.set_margin_top(0);
    row.set_margin_bottom(0);

    let label = gtk::Label::new(Some("Checkpoint"));
    label.set_xalign(0.0);
    label.add_css_class("chat-checkpoint-label");
    row.append(&label);

    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.set_hexpand(true);
    line.set_height_request(1);
    line.set_valign(gtk::Align::Center);
    line.add_css_class("chat-checkpoint-separator");
    row.append(&line);

    let restore_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    restore_button.set_valign(gtk::Align::Center);
    restore_button.set_can_target(true);
    restore_button.add_css_class("chat-checkpoint-restore");
    let restore_label = gtk::Label::new(Some("Restore"));
    restore_label.add_css_class("chat-checkpoint-restore-label");
    restore_button.append(&restore_label);
    {
        let thread_id = thread_id.to_string();
        let manager = manager.clone();
        let restore_button_weak = restore_button.downgrade();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let db = AppDb::open_default();
            let parent_window = restore_button_weak.upgrade().and_then(|restore_button| {
                restore_button
                    .root()
                    .and_then(|root| root.downcast::<gtk::Window>().ok())
            });
            let runtime = resolve_runtime_for_thread(&db, manager.as_ref(), &thread_id).ok();
            let active_thread_id = Rc::new(RefCell::new(Some(thread_id.clone())));
            let workspace_path = db
                .workspace_path_for_remote_thread(&thread_id)
                .ok()
                .flatten()
                .or_else(|| db.get_setting("last_active_workspace_path").ok().flatten())
                .unwrap_or_default();
            restore_preview::open_restore_preview_dialog(
                parent_window,
                db,
                runtime,
                thread_id.clone(),
                active_thread_id,
                workspace_path,
                Some(checkpoint_id),
            );
        });
        restore_button.add_controller(click);
    }
    row.append(&restore_button);

    row
}

fn surviving_turn_ids(thread: &Value) -> HashSet<String> {
    thread
        .get("turns")
        .and_then(Value::as_array)
        .map(|turns| {
            turns
                .iter()
                .filter_map(|turn| turn.get("id").and_then(Value::as_str))
                .map(|turn_id| turn_id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn filter_cached_entries_by_turn_ids(entries: &[Value], turn_ids: &HashSet<String>) -> Vec<Value> {
    entries
        .iter()
        .filter(|entry| {
            entry
                .get("turnId")
                .and_then(Value::as_str)
                .map(|turn_id| turn_ids.contains(turn_id))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

fn filter_cached_pending_requests_by_turn_ids(
    entries: &[Value],
    turn_ids: &HashSet<String>,
) -> Vec<Value> {
    entries
        .iter()
        .filter(|entry| {
            entry
                .get("turnId")
                .and_then(Value::as_str)
                .map(|turn_id| {
                    turn_ids.contains(turn_id) || turn_id.starts_with("opencode-pending:")
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

pub(super) fn prune_cached_state_for_thread(db: &AppDb, thread_id: &str, thread: &Value) {
    let turn_ids = surviving_turn_ids(thread);
    save_cached_commands(
        db,
        thread_id,
        &filter_cached_entries_by_turn_ids(&load_cached_commands(db, thread_id), &turn_ids),
    );
    save_cached_file_changes(
        db,
        thread_id,
        &filter_cached_entries_by_turn_ids(&load_cached_file_changes(db, thread_id), &turn_ids),
    );
    save_cached_tool_items(
        db,
        thread_id,
        &filter_cached_entries_by_turn_ids(&load_cached_tool_items(db, thread_id), &turn_ids),
    );
    save_cached_pending_requests(
        db,
        thread_id,
        &filter_cached_pending_requests_by_turn_ids(
            &load_cached_pending_requests(db, thread_id),
            &turn_ids,
        ),
    );
    save_cached_turn_errors(
        db,
        thread_id,
        &filter_cached_entries_by_turn_ids(&load_cached_turn_errors(db, thread_id), &turn_ids),
    );
}

pub(super) fn sync_completed_turns_from_thread(
    db: &AppDb,
    thread_id: &str,
    thread: &Value,
) -> rusqlite::Result<usize> {
    let existing_turns = db.list_local_chat_turns_for_remote_thread(thread_id)?;
    let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
        eprintln!(
            "[history-sync] thread/read missing turns for thread_id={}, preserving {} local turn(s)",
            thread_id,
            existing_turns.len()
        );
        return Ok(existing_turns.len());
    };

    let existing_by_turn_id: HashMap<String, (i64, Option<i64>)> = existing_turns
        .into_iter()
        .map(|turn| (turn.external_turn_id, (turn.created_at, turn.completed_at)))
        .collect();

    let mut completed = Vec::new();
    for turn in turns {
        let Some(external_turn_id) = turn
            .get("id")
            .and_then(Value::as_str)
            .map(|id| id.to_string())
        else {
            continue;
        };
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };

        let user_text = extract_user_message_payload_from_items(items)
            .map(|payload| payload.storage_text())
            .unwrap_or_default();

        let assistant_text = extract_assistant_turn_text(items);
        let status = if turn.get("error").is_some() {
            "failed"
        } else {
            "completed"
        }
        .to_string();

        let previous = existing_by_turn_id.get(&external_turn_id).copied();
        let created_at = parse_timestamp_opt(turn.get("createdAt"))
            .or_else(|| previous.map(|(created_at, _)| created_at))
            .unwrap_or_else(unix_now);
        let completed_at = turn
            .get("completedAt")
            .filter(|v| !v.is_null())
            .and_then(|v| parse_timestamp_opt(Some(v)))
            .or_else(|| previous.and_then(|(_, completed_at)| completed_at));

        if user_text.trim().is_empty() && assistant_text.trim().is_empty() && status == "completed"
        {
            continue;
        }

        completed.push(LocalChatTurnInput {
            external_turn_id,
            user_text,
            assistant_text,
            raw_items_json: serde_json::to_string(items).ok(),
            status,
            created_at,
            completed_at,
        });
    }

    completed.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| {
                a.completed_at
                    .unwrap_or(i64::MAX)
                    .cmp(&b.completed_at.unwrap_or(i64::MAX))
            })
            .then_with(|| a.external_turn_id.cmp(&b.external_turn_id))
    });
    let mut existing_by_turn: HashMap<
        String,
        (String, String, Option<String>, String, i64, Option<i64>),
    > = db
        .list_local_chat_turns_for_remote_thread(thread_id)?
        .into_iter()
        .map(|turn| {
            (
                turn.external_turn_id,
                (
                    turn.user_text,
                    turn.assistant_text,
                    turn.raw_items_json,
                    turn.status,
                    turn.created_at,
                    turn.completed_at,
                ),
            )
        })
        .collect();
    let mut changed_count = 0usize;
    for turn in &completed {
        let next = (
            turn.user_text.clone(),
            turn.assistant_text.clone(),
            turn.raw_items_json.clone(),
            turn.status.clone(),
            turn.created_at,
            turn.completed_at,
        );
        match existing_by_turn.remove(&turn.external_turn_id) {
            Some(current) if current == next => {}
            _ => changed_count += 1,
        }
    }
    changed_count += existing_by_turn.len();

    db.replace_local_chat_turns_for_remote_thread(thread_id, &completed)?;
    Ok(changed_count)
}

#[allow(clippy::too_many_arguments)]
fn render_local_turn(
    turn: &LocalChatTurnRecord,
    manager: &Option<Rc<CodexProfileManager>>,
    messages_box: &gtk::Box,
    conversation_stack: &gtk::Stack,
    thread_id: &str,
    checkpoint_by_turn: &HashMap<String, i64>,
    cached_commands_by_turn: &HashMap<String, Vec<Value>>,
    cached_file_changes_by_turn: &HashMap<String, Vec<Value>>,
    cached_tool_items_by_turn: &HashMap<String, Vec<Value>>,
    cached_turn_errors_by_turn: &HashMap<String, String>,
) -> bool {
    let mut has_any = false;
    let turn_cached_commands = cached_commands_by_turn
        .get(&turn.external_turn_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let turn_cached_file_changes = cached_file_changes_by_turn
        .get(&turn.external_turn_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let turn_cached_tool_items = cached_tool_items_by_turn
        .get(&turn.external_turn_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let has_cached_actions = !turn_cached_commands.is_empty()
        || !turn_cached_file_changes.is_empty()
        || !turn_cached_tool_items.is_empty();
    let max_cached_after_text_count = turn_cached_commands
        .iter()
        .chain(turn_cached_file_changes.iter())
        .chain(turn_cached_tool_items.iter())
        .filter_map(|cached| cached.get("afterTextCount").and_then(Value::as_u64))
        .max()
        .unwrap_or(0) as usize;

    if let Some(checkpoint_id) = checkpoint_by_turn.get(&turn.external_turn_id).copied() {
        messages_box.append(&create_checkpoint_strip_widget(
            thread_id,
            checkpoint_id,
            &turn.external_turn_id,
            Some(&turn.user_text),
            manager.clone(),
        ));
        has_any = true;
    }

    let parsed_items = turn.raw_items_json.as_deref().and_then(|raw| {
        if should_parse_raw_items_for_history_turn(
            turn,
            raw,
            has_cached_actions,
            max_cached_after_text_count,
        ) {
            serde_json::from_str::<Vec<Value>>(raw).ok()
        } else {
            None
        }
    });
    let user_payload_from_items = parsed_items
        .as_ref()
        .and_then(|items| extract_user_message_payload_from_items(items));
    if let Some(payload) = user_payload_from_items {
        let user_content = super::message_render::append_user_message_with_images(
            messages_box,
            None,
            conversation_stack,
            &payload.render_text(),
            &payload.local_image_paths,
            to_system_time(turn.created_at),
        );
        let _ = super::message_render::set_message_row_marker(
            &user_content,
            &user_row_marker(&turn.external_turn_id),
        );
        has_any = true;
    } else if !turn.user_text.trim().is_empty() {
        let user_bubble = super::message_render::append_message(
            messages_box,
            None,
            conversation_stack,
            &turn.user_text,
            true,
            to_system_time(turn.created_at),
        );
        let _ = super::message_render::set_message_row_marker(
            &user_bubble,
            &user_row_marker(&turn.external_turn_id),
        );
        has_any = true;
    }

    let mut synthetic_items = parsed_items.unwrap_or_default();
    let has_renderable_agent_message = synthetic_items.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("agentMessage")
            && super::codex_events::extract_agent_message_text(item)
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false)
    });
    if !has_renderable_agent_message && !turn.assistant_text.trim().is_empty() {
        synthetic_items.push(serde_json::json!({
            "type": "agentMessage",
            "text": turn.assistant_text
        }));
    }

    let turn_cached_error = cached_turn_errors_by_turn
        .get(&turn.external_turn_id)
        .map(String::as_str);
    let fallback_error = if turn.status == "failed" {
        Some("Turn failed.")
    } else {
        None
    };
    let turn_error = turn_cached_error.or(fallback_error);

    if append_history_assistant_turn(
        messages_box,
        conversation_stack,
        &synthetic_items,
        turn_cached_commands,
        turn_cached_file_changes,
        turn_cached_tool_items,
        turn_error,
        to_system_time(turn.completed_at.unwrap_or(turn.created_at)),
    ) {
        has_any = true;
    }

    has_any
}

pub(super) fn render_local_thread_history_from_db(
    db: &AppDb,
    manager: Option<Rc<CodexProfileManager>>,
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    suggestion_row: &gtk::Box,
    thread_id: &str,
    preloaded_snapshot: Option<ThreadHistoryRenderSnapshot>,
) -> bool {
    super::message_render::set_messages_box_thread_context(messages_box, Some(thread_id));
    let (
        turns,
        checkpoint_by_turn,
        cached_commands,
        cached_file_changes,
        cached_tool_items,
        cached_turn_errors,
    ) = if let Some(snapshot) = preloaded_snapshot {
        (
            snapshot.turns,
            snapshot.checkpoint_by_turn,
            snapshot.caches.commands,
            snapshot.caches.file_changes,
            snapshot.caches.tool_items,
            snapshot.caches.turn_errors,
        )
    } else {
        let turns = db
            .list_local_chat_turns_for_remote_thread(thread_id)
            .unwrap_or_default();
        let checkpoints = crate::restore::list_checkpoints_for_remote_thread(db, thread_id);
        let checkpoint_by_turn = resolve_checkpoint_map_for_turns(&turns, checkpoints);
        (
            turns,
            checkpoint_by_turn,
            load_cached_commands(db, thread_id),
            load_cached_file_changes(db, thread_id),
            load_cached_tool_items(db, thread_id),
            load_cached_turn_errors(db, thread_id),
        )
    };
    let cached_commands_by_turn = group_cached_entries_by_turn_id(cached_commands);
    let cached_file_changes_by_turn = group_cached_entries_by_turn_id(cached_file_changes);
    let cached_tool_items_by_turn = group_cached_entries_by_turn_id(cached_tool_items);
    let cached_turn_errors_by_turn = map_cached_turn_errors_by_turn_id(cached_turn_errors);

    let generation = begin_local_history_render(messages_box);
    clear_local_history_lazy_state(messages_box);
    super::clear_messages(messages_box);
    if turns.is_empty() {
        conversation_stack.set_visible_child_name("empty");
        suggestion_row.set_visible(true);
        super::codex_runtime::mark_history_load_finished(thread_id, "empty sqlite snapshot");
        return false;
    }

    conversation_stack.set_visible_child_name("messages");
    suggestion_row.set_visible(false);
    let total = turns.len();
    let initial_start = total.saturating_sub(INITIAL_VISIBLE_TURNS);
    let turns = Rc::new(turns);
    let checkpoint_by_turn = Rc::new(checkpoint_by_turn);
    let cached_commands_by_turn = Rc::new(cached_commands_by_turn);
    let cached_file_changes_by_turn = Rc::new(cached_file_changes_by_turn);
    let cached_tool_items_by_turn = Rc::new(cached_tool_items_by_turn);
    let cached_turn_errors_by_turn = Rc::new(cached_turn_errors_by_turn);
    let rendered_any = render_history_turn_range(
        messages_box,
        conversation_stack,
        thread_id,
        &manager,
        &turns[initial_start..],
        &checkpoint_by_turn,
        &cached_commands_by_turn,
        &cached_file_changes_by_turn,
        &cached_tool_items_by_turn,
        &cached_turn_errors_by_turn,
    );
    super::message_render::set_chat_reasoning_visibility(
        messages_box,
        super::message_render::messages_reasoning_visible(messages_box),
    );

    if rendered_any {
        conversation_stack.set_visible_child_name("messages");
        super::message_render::scroll_to_bottom(messages_scroll);
    } else {
        conversation_stack.set_visible_child_name("empty");
    }
    suggestion_row.set_visible(!rendered_any);

    let lazy_state = Rc::new(RefCell::new(LocalHistoryLazyState {
        thread_id: thread_id.to_string(),
        generation,
        manager,
        turns,
        checkpoint_by_turn,
        cached_commands_by_turn,
        cached_file_changes_by_turn,
        cached_tool_items_by_turn,
        cached_turn_errors_by_turn,
        initial_start,
        rendered_start: initial_start,
        lazy_loaded_row_count: 0,
        loading_older: false,
        pruning_older: false,
    }));
    set_local_history_lazy_state(messages_box, lazy_state);
    ensure_local_history_scroll_watcher(
        messages_box,
        messages_scroll,
        conversation_stack,
        suggestion_row,
    );

    if rendered_any {
        if local_history_render_is_current(messages_box, generation) {
            super::message_render::scroll_to_bottom(messages_scroll);
            super::codex_runtime::mark_history_load_finished(
                thread_id,
                &format!(
                    "initial sqlite render complete (shown turns={}/{total})",
                    total - initial_start
                ),
            );
        }
    } else {
        super::codex_runtime::mark_history_load_finished(
            thread_id,
            &format!(
                "initial sqlite render complete (shown turns={}/{total})",
                total - initial_start
            ),
        );
    }
    true
}

fn load_setting_from_connection(conn: &Connection, key: &str) -> Option<String> {
    let mut stmt = conn
        .prepare(
            "SELECT value
             FROM settings
             WHERE key = ?1
             LIMIT 1",
        )
        .ok()?;
    let mut rows = stmt.query(params![key]).ok()?;
    let row = rows.next().ok()??;
    row.get(0).ok()
}

fn load_cached_entries_from_connection(conn: &Connection, key: &str) -> Vec<Value> {
    let raw = load_setting_from_connection(conn, key);
    raw.and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
        .unwrap_or_default()
}

pub(super) fn load_thread_history_render_snapshot_detached(
    thread_id: &str,
) -> Result<ThreadHistoryRenderSnapshot, String> {
    let conn = AppDb::open_file_connection()
        .map_err(|err| format!("open sqlite connection failed: {err}"))?;

    let local_thread_id = {
        let mut stmt = conn
            .prepare(
                "SELECT id
                 FROM threads
                 WHERE codex_thread_id = ?1
                 LIMIT 1",
            )
            .map_err(|err| format!("prepare local_thread_id query failed: {err}"))?;
        let mut rows = stmt
            .query(params![thread_id])
            .map_err(|err| format!("query local_thread_id failed: {err}"))?;
        let Some(row) = rows
            .next()
            .map_err(|err| format!("read local_thread_id row failed: {err}"))?
        else {
            return Ok(ThreadHistoryRenderSnapshot {
                turns: Vec::new(),
                checkpoint_by_turn: HashMap::new(),
                caches: ThreadHistoryRenderCaches {
                    commands: Vec::new(),
                    file_changes: Vec::new(),
                    tool_items: Vec::new(),
                    pending_requests: Vec::new(),
                    turn_errors: Vec::new(),
                },
            });
        };
        row.get::<_, i64>(0)
            .map_err(|err| format!("decode local_thread_id failed: {err}"))?
    };

    let turns = {
        let mut stmt = conn
            .prepare(
                "SELECT external_turn_id, user_text, assistant_text, raw_items_json, status, created_at, completed_at
                 FROM chat_turns
                 WHERE local_thread_id = ?1
                   AND provider_id = 'codex'
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| format!("prepare chat_turns query failed: {err}"))?;
        let rows = stmt
            .query_map(params![local_thread_id], |row| {
                Ok(LocalChatTurnRecord {
                    external_turn_id: row.get(0)?,
                    user_text: row.get(1)?,
                    assistant_text: row.get(2)?,
                    raw_items_json: row.get(3)?,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                    completed_at: row.get(6)?,
                })
            })
            .map_err(|err| format!("query chat_turns failed: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("decode chat_turns row failed: {err}"))?);
        }
        out
    };

    let checkpoint_by_turn = {
        let mut checkpoints = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT c.id, c.local_thread_id, c.codex_thread_id, c.turn_id, c.created_at
             FROM restore_checkpoints c
             WHERE c.local_thread_id = ?1
               AND c.turn_id NOT LIKE 'restore-%'
             ORDER BY c.created_at DESC, c.id DESC",
        ) {
            if let Ok(rows) = stmt.query_map(params![local_thread_id], |row| {
                Ok(RestoreCheckpoint {
                    id: row.get(0)?,
                    local_thread_id: row.get(1)?,
                    codex_thread_id: row.get(2)?,
                    turn_id: row.get(3)?,
                    created_at: row.get(4)?,
                })
            }) {
                for row in rows.flatten() {
                    checkpoints.push(row);
                }
            }
        }
        resolve_checkpoint_map_for_turns(&turns, checkpoints)
    };

    let commands = load_cached_entries_from_connection(&conn, &command_cache_key(thread_id));
    let file_changes =
        load_cached_entries_from_connection(&conn, &file_change_cache_key(thread_id));
    let tool_items = load_cached_entries_from_connection(&conn, &tool_item_cache_key(thread_id))
        .into_iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) != Some("fileChangeOutput"))
        .collect::<Vec<Value>>();
    let pending_requests =
        load_cached_entries_from_connection(&conn, &pending_request_cache_key(thread_id));
    let turn_errors = load_cached_entries_from_connection(&conn, &turn_error_cache_key(thread_id));

    Ok(ThreadHistoryRenderSnapshot {
        turns,
        checkpoint_by_turn,
        caches: ThreadHistoryRenderCaches {
            commands,
            file_changes,
            tool_items,
            pending_requests,
            turn_errors,
        },
    })
}

fn command_cache_key(thread_id: &str) -> String {
    format!("thread_commands:{thread_id}")
}

fn file_change_cache_key(thread_id: &str) -> String {
    format!("thread_file_changes:{thread_id}")
}

fn tool_item_cache_key(thread_id: &str) -> String {
    format!("thread_tool_items:{thread_id}")
}

fn pending_request_cache_key(thread_id: &str) -> String {
    format!("thread_pending_requests:{thread_id}")
}

fn turn_error_cache_key(thread_id: &str) -> String {
    format!("thread_turn_errors:{thread_id}")
}

pub(super) fn load_cached_commands(db: &AppDb, thread_id: &str) -> Vec<Value> {
    match db.get_setting(&command_cache_key(thread_id)) {
        Ok(Some(raw)) => serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub(super) fn load_cached_file_changes(db: &AppDb, thread_id: &str) -> Vec<Value> {
    match db.get_setting(&file_change_cache_key(thread_id)) {
        Ok(Some(raw)) => serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub(super) fn load_cached_tool_items(db: &AppDb, thread_id: &str) -> Vec<Value> {
    match db.get_setting(&tool_item_cache_key(thread_id)) {
        Ok(Some(raw)) => serde_json::from_str::<Vec<Value>>(&raw)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.get("type").and_then(Value::as_str) != Some("fileChangeOutput"))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn load_cached_pending_requests(db: &AppDb, thread_id: &str) -> Vec<Value> {
    match db.get_setting(&pending_request_cache_key(thread_id)) {
        Ok(Some(raw)) => serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub(super) fn load_cached_turn_errors(db: &AppDb, thread_id: &str) -> Vec<Value> {
    match db.get_setting(&turn_error_cache_key(thread_id)) {
        Ok(Some(raw)) => serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub(super) fn save_cached_commands(db: &AppDb, thread_id: &str, commands: &[Value]) {
    if let Ok(raw) = serde_json::to_string(commands) {
        let _ = db.set_setting(&command_cache_key(thread_id), &raw);
    }
}

pub(super) fn save_cached_file_changes(db: &AppDb, thread_id: &str, file_changes: &[Value]) {
    if let Ok(raw) = serde_json::to_string(file_changes) {
        let _ = db.set_setting(&file_change_cache_key(thread_id), &raw);
    }
}

pub(super) fn save_cached_tool_items(db: &AppDb, thread_id: &str, tool_items: &[Value]) {
    if let Ok(raw) = serde_json::to_string(tool_items) {
        let _ = db.set_setting(&tool_item_cache_key(thread_id), &raw);
    }
}

pub(super) fn save_cached_pending_requests(
    db: &AppDb,
    thread_id: &str,
    pending_requests: &[Value],
) {
    if let Ok(raw) = serde_json::to_string(pending_requests) {
        let _ = db.set_setting(&pending_request_cache_key(thread_id), &raw);
    }
}

pub(super) fn save_cached_turn_errors(db: &AppDb, thread_id: &str, turn_errors: &[Value]) {
    if let Ok(raw) = serde_json::to_string(turn_errors) {
        let _ = db.set_setting(&turn_error_cache_key(thread_id), &raw);
    }
}

pub(super) fn upsert_cached_command(commands: &mut Vec<Value>, entry: Value) {
    let entry_item_id = entry
        .get("itemId")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    if let Some(item_id) = entry_item_id.as_deref() {
        if let Some(existing) = commands
            .iter_mut()
            .find(|value| value.get("itemId").and_then(Value::as_str) == Some(item_id))
        {
            *existing = entry;
            return;
        }
    }
    commands.push(entry);
}

pub(super) fn upsert_cached_file_change(file_changes: &mut Vec<Value>, entry: Value) {
    let entry_item_id = entry
        .get("itemId")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    if let Some(item_id) = entry_item_id.as_deref() {
        if let Some(existing) = file_changes
            .iter_mut()
            .find(|value| value.get("itemId").and_then(Value::as_str) == Some(item_id))
        {
            *existing = entry;
            return;
        }
    }
    file_changes.push(entry);
}

pub(super) fn upsert_cached_tool_item(tool_items: &mut Vec<Value>, entry: Value) {
    let entry_item_id = entry
        .get("itemId")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    if let Some(item_id) = entry_item_id.as_deref() {
        if let Some(existing) = tool_items
            .iter_mut()
            .find(|value| value.get("itemId").and_then(Value::as_str) == Some(item_id))
        {
            *existing = entry;
            return;
        }
    }
    tool_items.push(entry);
}

pub(super) fn upsert_cached_pending_request(pending_requests: &mut Vec<Value>, entry: Value) {
    let entry_request_id = entry.get("requestId").and_then(Value::as_i64);
    if let Some(request_id) = entry_request_id {
        if let Some(existing) = pending_requests
            .iter_mut()
            .find(|value| value.get("requestId").and_then(Value::as_i64) == Some(request_id))
        {
            *existing = entry;
            return;
        }
    }
    pending_requests.push(entry);
}

pub(super) fn remove_cached_pending_request(
    pending_requests: &mut Vec<Value>,
    request_id: i64,
) -> bool {
    let before = pending_requests.len();
    pending_requests
        .retain(|value| value.get("requestId").and_then(Value::as_i64) != Some(request_id));
    pending_requests.len() != before
}

pub(super) fn remove_cached_pending_requests_for_turn(
    pending_requests: &mut Vec<Value>,
    turn_id: &str,
) -> bool {
    let before = pending_requests.len();
    pending_requests.retain(|value| value.get("turnId").and_then(Value::as_str) != Some(turn_id));
    pending_requests.len() != before
}

pub(super) fn upsert_cached_turn_error(turn_errors: &mut Vec<Value>, entry: Value) {
    let entry_turn_id = entry
        .get("turnId")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    if let Some(turn_id) = entry_turn_id.as_deref() {
        if let Some(existing) = turn_errors
            .iter_mut()
            .find(|value| value.get("turnId").and_then(Value::as_str) == Some(turn_id))
        {
            *existing = entry;
            return;
        }
    }
    turn_errors.push(entry);
}
