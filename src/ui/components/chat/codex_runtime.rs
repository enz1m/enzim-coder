use crate::codex_appserver::{AppServerNotification, CodexAppServer};
use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use gtk::prelude::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

#[path = "codex_runtime/command_write_detection.rs"]
mod command_write_detection;
use command_write_detection::{extract_write_paths_from_command, is_probably_file_write_command};

#[derive(Clone)]
struct PendingServerRequestUi {
    thread_id: String,
    turn_id: String,
    card: gtk::Box,
}

type SharedEvent = (i64, AppServerNotification);

#[derive(Default)]
struct SharedRuntimeState {
    event_sinks: Vec<mpsc::Sender<SharedEvent>>,
    event_workers: HashSet<i64>,
    active_turns: HashMap<String, String>,
    active_turn_started_micros_by_turn: HashMap<String, i64>,
    completed_unseen_threads: HashSet<String>,
    unavailable_history_threads: HashSet<String>,
    dirty_history_threads: HashSet<String>,
    requested_history_reloads: HashSet<String>,
}

fn shared_runtime_state() -> &'static Mutex<SharedRuntimeState> {
    static SHARED_RUNTIME_STATE: OnceLock<Mutex<SharedRuntimeState>> = OnceLock::new();
    SHARED_RUNTIME_STATE.get_or_init(|| Mutex::new(SharedRuntimeState::default()))
}

pub(super) fn mark_history_load_started(_thread_id: &str, _trigger: &str) {}

pub(super) fn log_history_load_step(_thread_id: &str, _stage: &str) {}

pub(super) fn mark_history_load_finished(_thread_id: &str, _stage: &str) {}

fn mark_history_unavailable_for_thread(thread_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state
            .unavailable_history_threads
            .insert(thread_id.to_string());
    }
}

fn clear_history_unavailable_for_thread(thread_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state.unavailable_history_threads.remove(thread_id);
    }
}

fn history_is_unavailable_for_thread(thread_id: &str) -> bool {
    shared_runtime_state()
        .lock()
        .ok()
        .map(|state| state.unavailable_history_threads.contains(thread_id))
        .unwrap_or(false)
}

fn mark_history_dirty_for_thread(thread_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state.dirty_history_threads.insert(thread_id.to_string());
    }
}

fn clear_history_dirty_for_thread(thread_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state.dirty_history_threads.remove(thread_id);
    }
}

fn history_is_dirty_for_thread(thread_id: &str) -> bool {
    shared_runtime_state()
        .lock()
        .ok()
        .map(|state| state.dirty_history_threads.contains(thread_id))
        .unwrap_or(false)
}

pub(super) fn request_history_reload_for_thread(thread_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state
            .requested_history_reloads
            .insert(thread_id.to_string());
    }
}

fn take_history_reload_request(thread_id: &str) -> bool {
    shared_runtime_state()
        .lock()
        .ok()
        .map(|mut state| state.requested_history_reloads.remove(thread_id))
        .unwrap_or(false)
}

fn register_shared_event_sink(tx: mpsc::Sender<SharedEvent>) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state.event_sinks.push(tx);
    }
}

fn broadcast_shared_event(profile_id: i64, event: AppServerNotification) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state
            .event_sinks
            .retain(|tx| tx.send((profile_id, event.clone())).is_ok());
    }
}

fn ensure_shared_event_workers(manager: &Rc<CodexProfileManager>) {
    for (profile_id, client) in manager.running_clients() {
        let should_start = if let Ok(mut state) = shared_runtime_state().lock() {
            state.event_workers.insert(profile_id)
        } else {
            false
        };
        if !should_start {
            continue;
        }
        thread::spawn(move || {
            let rx = client.subscribe_notifications();
            while let Ok(event) = rx.recv() {
                broadcast_shared_event(profile_id, event);
            }
            if let Ok(mut state) = shared_runtime_state().lock() {
                state.event_workers.remove(&profile_id);
            }
        });
    }
}

fn set_active_turn_for_thread(thread_id: &str, turn_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state
            .active_turns
            .insert(thread_id.to_string(), turn_id.to_string());
        state
            .active_turn_started_micros_by_turn
            .insert(turn_id.to_string(), gtk::glib::monotonic_time());
    }
}

fn clear_active_turn_for_thread(thread_id: &str, turn_id: Option<&str>) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        match (state.active_turns.get(thread_id).cloned(), turn_id) {
            (Some(current), Some(expected)) if current == expected => {
                state.active_turns.remove(thread_id);
                state.active_turn_started_micros_by_turn.remove(&current);
            }
            (Some(_), None) => {
                if let Some(current) = state.active_turns.remove(thread_id) {
                    state.active_turn_started_micros_by_turn.remove(&current);
                }
            }
            _ => {}
        }
    }
}

pub(super) fn active_turn_for_thread(thread_id: &str) -> Option<String> {
    shared_runtime_state()
        .lock()
        .ok()
        .and_then(|state| state.active_turns.get(thread_id).cloned())
}

pub(super) fn has_any_active_turn() -> bool {
    shared_runtime_state()
        .lock()
        .ok()
        .map(|state| !state.active_turns.is_empty())
        .unwrap_or(false)
}

pub(super) fn active_turn_started_micros(turn_id: &str) -> Option<i64> {
    shared_runtime_state().lock().ok().and_then(|state| {
        state
            .active_turn_started_micros_by_turn
            .get(turn_id)
            .copied()
    })
}

pub(super) fn mark_thread_completed_unseen(thread_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state.completed_unseen_threads.insert(thread_id.to_string());
    }
}

pub(super) fn clear_thread_completed_unseen(thread_id: &str) {
    if let Ok(mut state) = shared_runtime_state().lock() {
        state.completed_unseen_threads.remove(thread_id);
    }
}

pub(super) fn thread_has_completed_unseen(thread_id: &str) -> bool {
    shared_runtime_state()
        .lock()
        .ok()
        .map(|state| state.completed_unseen_threads.contains(thread_id))
        .unwrap_or(false)
}

fn looks_like_reconnect_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("error reconnecting")
        || (lower.contains("reconnect")
            && (lower.contains("turn failed") || lower.contains("error")))
}

fn is_profile_logged_in(
    db: &AppDb,
    manager: &Rc<CodexProfileManager>,
    profile_id: i64,
    fallback_profile: &crate::data::CodexProfileRecord,
) -> bool {
    let profile = db
        .get_codex_profile(profile_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| fallback_profile.clone());

    let has_cached_auth = profile
        .last_email
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || profile
            .last_account_type
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
    if has_cached_auth {
        return true;
    }

    let Some(client) = manager.running_client_for_profile(profile_id) else {
        return false;
    };
    client
        .account_read(false)
        .ok()
        .flatten()
        .map(|account| {
            account
                .email
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
                || !account.account_type.trim().is_empty()
        })
        .unwrap_or(false)
}

pub(super) fn maybe_replace_profile_auth_error_message(
    db: &AppDb,
    manager: &Rc<CodexProfileManager>,
    resolved_thread_id: Option<&str>,
    message: &str,
) -> String {
    if !looks_like_reconnect_error(message) {
        return message.to_string();
    }

    let Some(codex_thread_id) = resolved_thread_id else {
        return message.to_string();
    };
    let Some(thread_record) = db
        .get_thread_record_by_codex_thread_id(codex_thread_id)
        .ok()
        .flatten()
    else {
        return message.to_string();
    };
    let Some(profile) = db
        .get_codex_profile(thread_record.profile_id)
        .ok()
        .flatten()
    else {
        return message.to_string();
    };

    if is_profile_logged_in(db, manager, thread_record.profile_id, &profile) {
        return message.to_string();
    }

    let system_home =
        crate::data::configured_profile_home_dir(&crate::data::default_app_data_dir())
            .to_string_lossy()
            .to_string();
    let is_system_profile = profile.home_dir.trim() == system_home.trim();

    if is_system_profile {
        "You are logged out. Please login with Codex CLI in your terminal.".to_string()
    } else {
        "Open Profile settings to reauthenticate / log in with ChatGPT.".to_string()
    }
}

fn approval_decision_options_for_event(method: &str, params: &Value) -> Vec<Value> {
    if let Some(raw) = params.get("availableDecisions").and_then(Value::as_array) {
        let mut out = Vec::new();
        for decision in raw {
            out.push(decision.clone());
        }
        if !out.is_empty() {
            return out;
        }
    }

    let defaults = if method == "item/tool/requestUserInput" {
        vec!["accept", "decline", "cancel"]
    } else {
        vec!["accept", "acceptForSession", "decline", "cancel"]
    };
    defaults
        .into_iter()
        .map(|decision| Value::String(decision.to_string()))
        .collect()
}

fn approval_decision_label(decision: &Value) -> String {
    if let Some(value) = decision.as_str() {
        return match value {
            "acceptForSession" => "Accept for session".to_string(),
            "accept" => "Accept".to_string(),
            "decline" => "Decline".to_string(),
            "cancel" => "Cancel".to_string(),
            _ => value.to_string(),
        };
    }

    if let Some(obj) = decision.as_object() {
        if obj.get("acceptWithExecpolicyAmendment").is_some() {
            return "Accept with policy amendment".to_string();
        }
        if obj.get("applyNetworkPolicyAmendment").is_some() {
            return "Apply network policy amendment".to_string();
        }
    }

    "Submit".to_string()
}

fn decision_key(decision: &Value) -> Option<String> {
    if let Some(value) = decision.as_str() {
        return Some(value.to_string());
    }
    if let Some(obj) = decision.as_object() {
        if let Some((key, _)) = obj.iter().next() {
            return Some(key.clone());
        }
    }
    None
}

fn parse_execpolicy_tokens(raw: &str) -> Vec<String> {
    raw.split_whitespace()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

fn append_checkpoint_strip_for_turn(
    manager: &Rc<CodexProfileManager>,
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    active_thread_id: Option<&str>,
    thread_id: &str,
    turn_id: &str,
    checkpoint_id: i64,
) {
    if !super::codex_events::should_render_for_active(Some(thread_id), active_thread_id) {
        return;
    }
    conversation_stack.set_visible_child_name("messages");
    let marker_name = format!("checkpoint-strip:{turn_id}");
    let mut existing = messages_box.first_child();
    while let Some(node) = existing {
        if node.widget_name() == marker_name {
            return;
        }
        existing = node.next_sibling();
    }

    let strip = super::codex_history::create_checkpoint_strip_widget(
        thread_id,
        checkpoint_id,
        turn_id,
        None,
        Some(manager.clone()),
    );
    strip.set_widget_name(&marker_name);

    let user_marker = format!("turn-user-row:{turn_id}");
    let mut child = messages_box.first_child();
    while let Some(node) = child {
        if node.widget_name() == user_marker {
            let previous = node.prev_sibling();
            messages_box.insert_child_after(&strip, previous.as_ref());
            super::message_render::scroll_to_bottom(messages_scroll);
            return;
        }
        child = node.next_sibling();
    }

    messages_box.append(&strip);
    super::message_render::scroll_to_bottom(messages_scroll);
}

include!("codex_runtime/request_cards.rs");
include!("codex_runtime/attach_impl.rs");
