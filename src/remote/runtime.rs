use crate::data::{AppDb, RemoteTelegramAccountRecord, ThreadRecord, WorkspaceWithThreads};
use crate::remote::formatting;
use crate::remote::telegram::TelegramClient;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_THREADS_LISTED: usize = 20;

fn worker_running_flag() -> &'static Arc<AtomicBool> {
    static RUNNING: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    RUNNING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn start_background_worker() {
    let running = worker_running_flag().clone();
    if running.load(Ordering::Relaxed) {
        return;
    }
    running.store(true, Ordering::Relaxed);
    let worker_flag = running.clone();
    thread::spawn(move || worker_loop(worker_flag));
}

pub fn stop_background_worker() {
    worker_running_flag().store(false, Ordering::Relaxed);
}

pub fn forward_turn_completion_if_enabled(
    db: &AppDb,
    codex_thread_id: &str,
    turn_id: &str,
    assistant_text: &str,
    command_count: usize,
    file_edit_count: usize,
    other_action_count: usize,
) {
    if !db.remote_mode_enabled() {
        return;
    }
    let polling_enabled = crate::remote::bool_from_setting(
        db.get_setting(crate::remote::SETTING_REMOTE_TELEGRAM_POLLING_ENABLED)
            .ok()
            .flatten(),
        true,
    );
    if !polling_enabled {
        return;
    }

    let Some(account) = db.remote_telegram_active_account().ok().flatten() else {
        return;
    };
    let Some(thread) = db
        .get_thread_record_by_remote_thread_id(codex_thread_id)
        .ok()
        .flatten()
    else {
        return;
    };

    let dedupe_key = format!("remote:telegram:forwarded:{codex_thread_id}:{turn_id}");
    let dedupe_state = db
        .get_setting(&dedupe_key)
        .ok()
        .flatten()
        .unwrap_or_default();
    if dedupe_state == "1" || dedupe_state == "pending" {
        return;
    }
    let _ = db.set_setting(&dedupe_key, "pending");

    let bot_token = account.bot_token.clone();
    let chat_id = account.telegram_chat_id.clone();
    let local_thread_id = thread.id;
    let assistant_text_seed = assistant_text.to_string();
    let command_count_seed = command_count;
    let file_edit_count_seed = file_edit_count;
    let other_action_count_seed = other_action_count;
    let codex_thread_id = codex_thread_id.to_string();
    let turn_id = turn_id.to_string();
    let dedupe_key_for_thread = dedupe_key.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));

        let client = match TelegramClient::new(bot_token) {
            Ok(client) => client,
            Err(err) => {
                eprintln!("[remote] telegram client init failed: {err}");
                return;
            }
        };
        let db_bg = AppDb::open_default();
        let mut assistant_text = String::new();
        let mut command_count = 0usize;
        let mut file_edit_count = 0usize;
        let mut other_action_count = 0usize;

        for attempt in 0..10 {
            if let Ok(turns) = db_bg.list_local_chat_turns_for_remote_thread(&codex_thread_id) {
                if let Some(turn) = turns.iter().find(|turn| turn.external_turn_id == turn_id) {
                    if !turn.assistant_text.trim().is_empty() {
                        assistant_text = turn.assistant_text.clone();
                    }
                    let (c, f, o) = count_actions_from_raw_items(turn.raw_items_json.as_deref());
                    if c + f + o > 0 {
                        command_count = c;
                        file_edit_count = f;
                        other_action_count = o;
                    }
                }
            }

            let has_text = !assistant_text.trim().is_empty();
            let has_action_counts = command_count + file_edit_count + other_action_count > 0;
            if has_text && has_action_counts {
                break;
            }
            if attempt < 9 {
                thread::sleep(Duration::from_millis(260));
            }
        }
        if assistant_text.trim().is_empty() {
            assistant_text = assistant_text_seed;
        }
        if command_count + file_edit_count + other_action_count == 0 {
            command_count = command_count_seed;
            file_edit_count = file_edit_count_seed;
            other_action_count = other_action_count_seed;
        }

        let summary = build_turn_summary(
            &assistant_text,
            command_count,
            file_edit_count,
            other_action_count,
        );
        let html = formatting::markdown_to_telegram_html(&summary);
        let chunks = formatting::chunk_telegram_html(&html);
        let mut sent_any = false;
        for chunk in chunks {
            if chunk.trim().is_empty() {
                continue;
            }
            match client.send_html_message(&chat_id, &chunk, None) {
                Ok(message_id) => {
                    sent_any = true;
                    let _ = db_bg.upsert_remote_telegram_message_map(
                        &chat_id,
                        &message_id.to_string(),
                        local_thread_id,
                        Some(&codex_thread_id),
                        Some(&turn_id),
                    );
                }
                Err(err) => {
                    eprintln!("[remote] failed to forward turn to telegram: {err}");
                    break;
                }
            }
        }
        let _ = db_bg.set_setting(&dedupe_key_for_thread, if sent_any { "1" } else { "0" });
    });
}

fn worker_loop(running: Arc<AtomicBool>) {
    let mut active_token = String::new();
    let mut active_chat_id = String::new();
    let mut update_offset: Option<i64> = None;
    let mut client: Option<TelegramClient> = None;
    let db = AppDb::open_default();
    let mut last_remote_mode_enabled = db.remote_mode_enabled();
    let mut pending_activation_announcement = false;

    while running.load(Ordering::Relaxed) {
        let remote_mode_enabled = db.remote_mode_enabled();
        let transitioned_enabled = remote_mode_enabled && !last_remote_mode_enabled;
        let transitioned_disabled = !remote_mode_enabled && last_remote_mode_enabled;
        if transitioned_enabled {
            pending_activation_announcement = true;
        }
        if transitioned_disabled {
            announce_remote_mode_deactivated(
                db.as_ref(),
                &mut client,
                &mut active_token,
                &mut active_chat_id,
            );
        }
        if !remote_mode_enabled {
            pending_activation_announcement = false;
        }
        last_remote_mode_enabled = remote_mode_enabled;

        let polling_enabled = crate::remote::bool_from_setting(
            db.get_setting(crate::remote::SETTING_REMOTE_TELEGRAM_POLLING_ENABLED)
                .ok()
                .flatten(),
            true,
        );
        if !remote_mode_enabled || !polling_enabled {
            thread::sleep(Duration::from_millis(750));
            continue;
        }

        let Some(account) = db.remote_telegram_active_account().ok().flatten() else {
            thread::sleep(Duration::from_millis(1000));
            continue;
        };

        let should_reset_client =
            account.bot_token != active_token || account.telegram_chat_id != active_chat_id;
        if should_reset_client {
            active_token = account.bot_token.clone();
            active_chat_id = account.telegram_chat_id.clone();
            update_offset = None;
            client = TelegramClient::new(account.bot_token.clone()).ok();
        }
        let Some(client) = client.as_ref() else {
            thread::sleep(Duration::from_millis(1200));
            continue;
        };
        if pending_activation_announcement {
            announce_remote_mode_activated(db.as_ref(), client, &account.telegram_chat_id);
            pending_activation_announcement = false;
        }

        match client.get_updates(update_offset, 20) {
            Ok((next_offset, updates)) => {
                if let Some(next_offset) = next_offset {
                    update_offset = Some(next_offset);
                }
                process_updates(db.as_ref(), client, &account, updates);
            }
            Err(err) => {
                eprintln!("[remote] telegram polling error: {err}");
                thread::sleep(Duration::from_millis(1200));
            }
        }
    }
}

fn announce_remote_mode_activated(db: &AppDb, client: &TelegramClient, chat_id: &str) {
    let workspaces = db.list_workspaces_with_threads().unwrap_or_default();
    if workspaces.is_empty() {
        let body = "<b>Remote mode activated.</b>\nTelegram control is online.\n\n<b>Remote Navigator · Workspaces</b>\nNo workspaces found.";
        let _ = client.send_html_message(chat_id, body, None);
        return;
    }

    let mut rows = Vec::<Vec<Value>>::new();
    for entry in &workspaces {
        rows.push(vec![json!({
            "text": format!("🗂 {}", entry.workspace.name),
            "callback_data": format!("remote:ws:{}", entry.workspace.id)
        })]);
    }
    let markup = json!({ "inline_keyboard": rows });
    let body = "<b>Remote mode activated.</b>\nTelegram control is online.\n\n<b>Remote Navigator · Workspaces</b>\nSelect a workspace to continue. The buttons below open each workspace and then show its thread list for quick routing.";
    let _ = client.send_html_message_with_markup(chat_id, body, None, Some(markup));
}

fn announce_remote_mode_deactivated(
    db: &AppDb,
    client_slot: &mut Option<TelegramClient>,
    active_token: &mut String,
    active_chat_id: &mut String,
) {
    let Some(account) = db.remote_telegram_active_account().ok().flatten() else {
        return;
    };
    if client_slot.is_none()
        || account.bot_token != *active_token
        || account.telegram_chat_id != *active_chat_id
    {
        *active_token = account.bot_token.clone();
        *active_chat_id = account.telegram_chat_id.clone();
        *client_slot = TelegramClient::new(account.bot_token.clone()).ok();
    }
    let Some(client) = client_slot.as_ref() else {
        return;
    };
    let body = "<b>Remote mode disabled</b>\nForwarding has stopped.\nThe bot is now idle until remote mode is enabled again.";
    let _ = client.send_html_message(&account.telegram_chat_id, body, None);
}

fn process_updates(
    db: &AppDb,
    client: &TelegramClient,
    account: &RemoteTelegramAccountRecord,
    updates: Vec<Value>,
) {
    for update in updates {
        if let Some(callback) = update.get("callback_query") {
            handle_callback_query(db, client, account, callback);
            continue;
        }

        let Some(message) = update.get("message") else {
            continue;
        };
        let Some(chat_id) = value_to_id_string(message.get("chat").and_then(|chat| chat.get("id")))
        else {
            continue;
        };
        if chat_id != account.telegram_chat_id {
            continue;
        }

        let message_id = value_to_id_string(message.get("message_id"));
        let from = message.get("from");
        let from_is_bot = from
            .and_then(|value| value.get("is_bot"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if from_is_bot {
            continue;
        }
        let from_user_id = value_to_id_string(from.and_then(|value| value.get("id")));
        let from_username = from
            .and_then(|value| value.get("username"))
            .and_then(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let text = message
            .get("text")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        if is_guarded_auth_code_message(db, &text) {
            continue;
        }

        let reply_to_message_id = value_to_id_string(
            message
                .get("reply_to_message")
                .and_then(|value| value.get("message_id")),
        );
        if let Some(reply_to_message_id) = reply_to_message_id {
            handle_reply_to_forwarded_message(
                db,
                client,
                &chat_id,
                &reply_to_message_id,
                &text,
                message_id.as_deref(),
                from_user_id.as_deref(),
                from_username.as_deref(),
            );
            continue;
        }

        handle_menu_or_command(
            db,
            client,
            &chat_id,
            &text,
            from_user_id.as_deref(),
            from_username.as_deref(),
            None,
        );
    }
}

fn handle_callback_query(
    db: &AppDb,
    client: &TelegramClient,
    account: &RemoteTelegramAccountRecord,
    callback: &Value,
) {
    let Some(query_id) = callback.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(data) = callback.get("data").and_then(Value::as_str) else {
        let _ = client.answer_callback_query(query_id, None);
        return;
    };
    let Some(chat_id) = value_to_id_string(
        callback
            .get("message")
            .and_then(|message| message.get("chat"))
            .and_then(|chat| chat.get("id")),
    ) else {
        let _ = client.answer_callback_query(query_id, None);
        return;
    };
    if chat_id != account.telegram_chat_id {
        let _ = client.answer_callback_query(query_id, None);
        return;
    }
    let message_id = callback
        .get("message")
        .and_then(|message| message.get("message_id"))
        .and_then(Value::as_i64);

    if data == "remote:workspaces" || data == "remote:home" {
        show_workspaces_inline(db, client, &chat_id, message_id);
        let _ = client.answer_callback_query(query_id, None);
        return;
    }
    if data == "remote:threads" {
        show_threads_inline(db, client, &chat_id, message_id, None);
        let _ = client.answer_callback_query(query_id, None);
        return;
    }
    if data == "remote:lastresp" {
        send_last_agent_response_for_selected_thread(db, client, &chat_id);
        let _ = client.answer_callback_query(query_id, None);
        return;
    }
    if let Some(raw_workspace_id) = data.strip_prefix("remote:ws:") {
        if let Ok(workspace_id) = raw_workspace_id.parse::<i64>() {
            let _ = db.set_setting(
                &chat_workspace_setting_key(&chat_id),
                &workspace_id.to_string(),
            );
            let _ = db.set_setting(&chat_thread_setting_key(&chat_id), "");
            show_threads_inline(db, client, &chat_id, message_id, Some(workspace_id));
        }
        let _ = client.answer_callback_query(query_id, None);
        return;
    }
    if let Some(raw_thread_id) = data.strip_prefix("remote:th:") {
        if let Ok(thread_id) = raw_thread_id.parse::<i64>() {
            let _ = select_thread_for_chat_and_ui(db, &chat_id, thread_id);
            show_active_selection_inline(db, client, &chat_id, message_id);
        }
        let _ = client.answer_callback_query(query_id, None);
        return;
    }

    let _ = client.answer_callback_query(query_id, Some("Unknown action"));
}

fn handle_reply_to_forwarded_message(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    reply_to_message_id: &str,
    text: &str,
    incoming_message_id: Option<&str>,
    from_user_id: Option<&str>,
    from_username: Option<&str>,
) {
    match db.local_thread_id_for_remote_telegram_reply(chat_id, reply_to_message_id) {
        Ok(Some(local_thread_id)) => {
            if db
                .enqueue_remote_pending_prompt(
                    local_thread_id,
                    text,
                    "telegram-reply",
                    Some(chat_id),
                    incoming_message_id,
                    from_user_id,
                    from_username,
                )
                .is_ok()
            {
                let thread_label = db
                    .get_thread_record(local_thread_id)
                    .ok()
                    .flatten()
                    .map(|thread| thread.title)
                    .unwrap_or_else(|| local_thread_id.to_string());
                let ack = format!(
                    "<b>Queued.</b>\nReply added to thread: <code>{}</code>",
                    escape_html(&thread_label)
                );
                let _ = client.send_html_message(chat_id, &ack, None);
            }
        }
        Ok(None) => {
            let _ = client.send_html_message(
                chat_id,
                "I could not map that reply to a thread. Open <code>Threads</code>, select one, then send plain text.",
                None,
            );
        }
        Err(err) => {
            eprintln!("[remote] failed to map telegram reply: {err}");
        }
    }
}

fn handle_menu_or_command(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    raw_text: &str,
    from_user_id: Option<&str>,
    from_username: Option<&str>,
    edit_message_id: Option<i64>,
) {
    let text = raw_text.trim();
    let lower = text.to_ascii_lowercase();

    if matches!(
        lower.as_str(),
        "/start" | "start" | "/help" | "help" | "menu"
    ) {
        show_workspaces_inline(db, client, chat_id, edit_message_id);
        return;
    }

    if matches!(lower.as_str(), "/workspaces" | "workspaces") {
        show_workspaces_inline(db, client, chat_id, edit_message_id);
        return;
    }

    if let Some(arg) = lower.strip_prefix("/ws ") {
        select_workspace_by_index(db, client, chat_id, arg.trim(), edit_message_id);
        return;
    }

    if matches!(lower.as_str(), "/threads" | "threads") {
        show_threads_inline(db, client, chat_id, edit_message_id, None);
        return;
    }

    if let Some(arg) = lower.strip_prefix("/th ") {
        select_thread_by_index(db, client, chat_id, arg.trim(), edit_message_id);
        return;
    }

    if matches!(lower.as_str(), "send message") {
        let _ = client.send_html_message(
            chat_id,
            "Select a thread, then send plain text to queue a prompt into it.",
            None,
        );
        return;
    }

    if let Some(text) = text.strip_prefix("/send ") {
        send_text_to_selected_thread(
            db,
            client,
            chat_id,
            text.trim(),
            from_user_id,
            from_username,
        );
        return;
    }

    if matches!(lower.as_str(), "new thread") {
        let _ = client.send_html_message(
            chat_id,
            "Use <code>/new_thread title</code> to create a new local thread in the selected workspace.",
            None,
        );
        return;
    }

    if let Some(title) = text.strip_prefix("/new_thread") {
        create_thread_from_bot(db, client, chat_id, title.trim());
        show_active_selection_inline(db, client, chat_id, edit_message_id);
        return;
    }

    if !text.is_empty() && !text.starts_with('/') {
        send_text_to_selected_thread(db, client, chat_id, text, from_user_id, from_username);
        return;
    }

    show_active_selection_inline(db, client, chat_id, edit_message_id);
}

fn show_workspaces_inline(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    edit_message_id: Option<i64>,
) {
    let workspaces = db.list_workspaces_with_threads().unwrap_or_default();
    if workspaces.is_empty() {
        let _ = send_or_edit_inline(
            client,
            chat_id,
            edit_message_id,
            "<b>Workspaces</b>\nNo workspaces found.",
            None,
        );
        return;
    }

    let mut rows = Vec::<Vec<Value>>::new();
    for entry in &workspaces {
        rows.push(vec![json!({
            "text": format!("🗂 {}", entry.workspace.name),
            "callback_data": format!("remote:ws:{}", entry.workspace.id)
        })]);
    }
    let markup = json!({ "inline_keyboard": rows });
    let body = "<b>Remote Navigator · Workspaces</b>\nSelect a workspace to continue. The buttons below open each workspace and then show its thread list for quick routing.";
    let _ = send_or_edit_inline(client, chat_id, edit_message_id, body, Some(markup));
}

fn show_threads_inline(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    edit_message_id: Option<i64>,
    preferred_workspace_id: Option<i64>,
) {
    let workspaces = db.list_workspaces_with_threads().unwrap_or_default();
    if workspaces.is_empty() {
        show_workspaces_inline(db, client, chat_id, edit_message_id);
        return;
    }

    if let Some(preferred_workspace_id) = preferred_workspace_id {
        let _ = db.set_setting(
            &chat_workspace_setting_key(chat_id),
            &preferred_workspace_id.to_string(),
        );
    }
    let Some(workspace) = resolve_selected_workspace(db, chat_id, &workspaces) else {
        show_workspaces_inline(db, client, chat_id, edit_message_id);
        return;
    };

    let threads = workspace
        .threads
        .iter()
        .take(MAX_THREADS_LISTED)
        .cloned()
        .collect::<Vec<_>>();
    if threads.is_empty() {
        let markup = json!({
            "inline_keyboard": [[{
                "text": "⬅️ Back to Workspaces",
                "callback_data": "remote:workspaces"
            }]]
        });
        let body = format!(
            "<b>Workspace: {}</b>\nNo open threads.\nUse <code>/new_thread title</code> to create one.",
            escape_html(&workspace.workspace.name)
        );
        let _ = send_or_edit_inline(client, chat_id, edit_message_id, &body, Some(markup));
        return;
    }

    let mut rows = Vec::<Vec<Value>>::new();
    for thread in &threads {
        rows.push(vec![json!({
            "text": format!("💬 {}", thread.title),
            "callback_data": format!("remote:th:{}", thread.id)
        })]);
    }
    rows.push(vec![json!({
        "text": "⬅️ Back to Workspaces",
        "callback_data": "remote:workspaces"
    })]);
    let markup = json!({ "inline_keyboard": rows });
    let body = format!(
        "<b>Remote Navigator · Threads</b>\nWorkspace: <b>{}</b>\nSelect a thread to make it active. After selection, any plain text you send will be queued into that thread.",
        escape_html(&workspace.workspace.name),
    );
    let _ = send_or_edit_inline(client, chat_id, edit_message_id, &body, Some(markup));
}

fn show_active_selection_inline(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    edit_message_id: Option<i64>,
) {
    let workspaces = db.list_workspaces_with_threads().unwrap_or_default();
    let Some(workspace) = resolve_selected_workspace(db, chat_id, &workspaces) else {
        show_workspaces_inline(db, client, chat_id, edit_message_id);
        return;
    };
    let Some(thread_id) = selected_thread_id(db, chat_id) else {
        show_threads_inline(
            db,
            client,
            chat_id,
            edit_message_id,
            Some(workspace.workspace.id),
        );
        return;
    };
    let thread = workspace.threads.iter().find(|entry| entry.id == thread_id);
    let Some(thread) = thread else {
        show_threads_inline(
            db,
            client,
            chat_id,
            edit_message_id,
            Some(workspace.workspace.id),
        );
        return;
    };

    let markup = json!({
        "inline_keyboard": [
            [{"text": "📨 Last Agent Response", "callback_data": "remote:lastresp"}],
            [{"text": "💬 Threads", "callback_data": "remote:threads"}],
            [{"text": "🗂 Workspaces", "callback_data": "remote:workspaces"}]
        ]
    });
    let body = format!(
        "<b>Remote Navigator · Active Target</b>\nWorkspace: <b>{}</b>\nThread: <b>{}</b>\n\nThis thread is now active. Send plain text directly in chat and it will be queued here automatically.",
        escape_html(&workspace.workspace.name),
        escape_html(&thread.title)
    );
    let _ = send_or_edit_inline(client, chat_id, edit_message_id, &body, Some(markup));
}

fn select_workspace_by_index(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    arg: &str,
    edit_message_id: Option<i64>,
) {
    let Ok(position) = arg.parse::<usize>() else {
        let _ = client.send_html_message(chat_id, "Workspace index must be a number.", None);
        return;
    };
    if position == 0 {
        let _ = client.send_html_message(chat_id, "Workspace index starts at 1.", None);
        return;
    }
    let workspaces = db.list_workspaces_with_threads().unwrap_or_default();
    let Some(entry) = workspaces.get(position - 1) else {
        let _ = client.send_html_message(chat_id, "Workspace index out of range.", None);
        return;
    };
    let _ = db.set_setting(
        &chat_workspace_setting_key(chat_id),
        &entry.workspace.id.to_string(),
    );
    let _ = db.set_setting(&chat_thread_setting_key(chat_id), "");
    show_threads_inline(
        db,
        client,
        chat_id,
        edit_message_id,
        Some(entry.workspace.id),
    );
}

fn select_thread_by_index(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    arg: &str,
    edit_message_id: Option<i64>,
) {
    let Ok(position) = arg.parse::<usize>() else {
        let _ = client.send_html_message(chat_id, "Thread index must be a number.", None);
        return;
    };
    if position == 0 {
        let _ = client.send_html_message(chat_id, "Thread index starts at 1.", None);
        return;
    }

    let workspaces = db.list_workspaces_with_threads().unwrap_or_default();
    let Some(workspace) = resolve_selected_workspace(db, chat_id, &workspaces) else {
        let _ = client.send_html_message(
            chat_id,
            "Select a workspace first with <code>/ws N</code>.",
            None,
        );
        return;
    };
    let Some(thread) = workspace.threads.get(position - 1) else {
        let _ = client.send_html_message(chat_id, "Thread index out of range.", None);
        return;
    };
    let _ = select_thread_for_chat_and_ui(db, chat_id, thread.id);
    show_active_selection_inline(db, client, chat_id, edit_message_id);
}

fn send_or_edit_inline(
    client: &TelegramClient,
    chat_id: &str,
    edit_message_id: Option<i64>,
    body: &str,
    markup: Option<Value>,
) -> Option<i64> {
    if let Some(message_id) = edit_message_id {
        if client
            .edit_html_message_with_markup(chat_id, message_id, body, markup.clone())
            .is_ok()
        {
            return Some(message_id);
        }
    }
    client
        .send_html_message_with_markup(chat_id, body, None, markup)
        .ok()
}

fn send_text_to_selected_thread(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
    text: &str,
    from_user_id: Option<&str>,
    from_username: Option<&str>,
) {
    if text.trim().is_empty() {
        let _ = client.send_html_message(chat_id, "Message text is empty.", None);
        return;
    }

    let Some(thread_id) = selected_thread_id(db, chat_id) else {
        let _ = client.send_html_message(
            chat_id,
            "No thread selected. Run <code>/threads</code> then <code>/th N</code>.",
            None,
        );
        return;
    };
    request_app_thread_activation(db, thread_id);

    if db
        .enqueue_remote_pending_prompt(
            thread_id,
            text,
            "telegram-command",
            Some(chat_id),
            None,
            from_user_id,
            from_username,
        )
        .is_ok()
    {
        let thread = db.get_thread_record(thread_id).ok().flatten();
        let thread_title = thread
            .as_ref()
            .map(|thread| thread.title.clone())
            .unwrap_or_else(|| thread_id.to_string());
        let linked = thread
            .as_ref()
            .and_then(|thread| thread.remote_thread_id())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        let mut body = format!(
            "<b>Queued.</b>\nMessage queued in thread <code>{}</code>.",
            escape_html(&thread_title)
        );
        if !linked {
            body.push_str(
                "\nThis thread is local-only right now; open it in the app to start/attach Codex.",
            );
        }
        let _ = client.send_html_message(chat_id, &body, None);
    }
}

fn send_last_agent_response_for_selected_thread(
    db: &AppDb,
    client: &TelegramClient,
    chat_id: &str,
) {
    let Some(thread_id) = selected_thread_id(db, chat_id) else {
        let _ = client.send_html_message(
            chat_id,
            "No active thread selected yet. Pick one from <code>Threads</code> first.",
            None,
        );
        return;
    };
    let Some(thread) = db.get_thread_record(thread_id).ok().flatten() else {
        let _ = client.send_html_message(chat_id, "Thread record not found.", None);
        return;
    };
    let Some(codex_thread_id) = thread
        .remote_thread_id()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        let _ = client.send_html_message(
            chat_id,
            "This thread has no linked Codex history yet.",
            None,
        );
        return;
    };
    let turns = db
        .list_local_chat_turns_for_remote_thread(codex_thread_id)
        .unwrap_or_default();
    let Some(last_assistant_turn) = turns
        .iter()
        .rev()
        .find(|turn| !turn.assistant_text.trim().is_empty())
    else {
        let _ = client.send_html_message(
            chat_id,
            "No assistant responses found yet for this thread.",
            None,
        );
        return;
    };

    let converted =
        formatting::markdown_to_telegram_html(last_assistant_turn.assistant_text.trim());
    let body = format!(
        "<b>Last agent response</b>\nThread: <b>{}</b>\n\n{}",
        escape_html(&thread.title),
        converted
    );
    for chunk in formatting::chunk_telegram_html(&body) {
        if chunk.trim().is_empty() {
            continue;
        }
        if client.send_html_message(chat_id, &chunk, None).is_err() {
            break;
        }
    }
}

fn create_thread_from_bot(db: &AppDb, client: &TelegramClient, chat_id: &str, title: &str) {
    let trimmed_title = if title.is_empty() {
        "Remote Thread"
    } else {
        title
    };
    let workspaces = db.list_workspaces_with_threads().unwrap_or_default();
    let Some(workspace) = resolve_selected_workspace(db, chat_id, &workspaces) else {
        let _ = client.send_html_message(
            chat_id,
            "Select a workspace first with <code>/ws N</code>.",
            None,
        );
        return;
    };

    let profile_id = db.active_profile_id().ok().flatten().unwrap_or(1);
    match db.create_thread_with_remote_identity(
        workspace.workspace.id,
        profile_id,
        None,
        trimmed_title,
        None,
        None,
        None,
    ) {
        Ok(thread) => {
            let _ = select_thread_for_chat_and_ui(db, chat_id, thread.id);
            let body = format!(
                "Created thread <b>{}</b> (id <code>{}</code>).",
                escape_html(&thread.title),
                thread.id
            );
            let _ = client.send_html_message(chat_id, &body, None);
        }
        Err(err) => {
            let _ = client.send_html_message(
                chat_id,
                &format!("Failed to create thread: {}", escape_html(&err.to_string())),
                None,
            );
        }
    }
}

fn build_turn_summary(
    assistant_text: &str,
    command_count: usize,
    file_edit_count: usize,
    other_action_count: usize,
) -> String {
    let mut lines = Vec::<String>::new();
    if !assistant_text.trim().is_empty() {
        lines.push(assistant_text.trim().to_string());
    }

    let total = command_count + file_edit_count + other_action_count;
    if total > 0 {
        let mut details = Vec::<String>::new();
        if command_count > 0 {
            details.push(format!("{command_count} commands"));
        }
        if file_edit_count > 0 {
            details.push(format!("{file_edit_count} file edits"));
        }
        if other_action_count > 0 {
            details.push(format!("{other_action_count} other actions"));
        }
        lines.push(format!("{total} actions run - {}", details.join(", ")));
    }

    if lines.is_empty() {
        "Turn completed.".to_string()
    } else {
        lines.join("\n\n")
    }
}

fn resolve_selected_workspace(
    db: &AppDb,
    chat_id: &str,
    workspaces: &[WorkspaceWithThreads],
) -> Option<WorkspaceWithThreads> {
    let selected = db
        .get_setting(&chat_workspace_setting_key(chat_id))
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok());
    if let Some(selected) = selected {
        if let Some(workspace) = workspaces
            .iter()
            .find(|entry| entry.workspace.id == selected)
            .cloned()
        {
            return Some(workspace);
        }
    }
    let fallback = workspaces.first().cloned();
    if let Some(workspace) = fallback.as_ref() {
        let _ = db.set_setting(
            &chat_workspace_setting_key(chat_id),
            &workspace.workspace.id.to_string(),
        );
    }
    fallback
}

fn selected_thread_id(db: &AppDb, chat_id: &str) -> Option<i64> {
    db.get_setting(&chat_thread_setting_key(chat_id))
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
}

fn select_thread_for_chat_and_ui(db: &AppDb, chat_id: &str, thread_id: i64) -> bool {
    let Some(thread) = db.get_thread_record(thread_id).ok().flatten() else {
        return false;
    };
    let _ = db.set_setting(
        &chat_workspace_setting_key(chat_id),
        &thread.workspace_id.to_string(),
    );
    let _ = db.set_setting(&chat_thread_setting_key(chat_id), &thread_id.to_string());
    request_app_thread_activation(db, thread_id);
    true
}

fn request_app_thread_activation(db: &AppDb, thread_id: i64) {
    let Some(thread) = db.get_thread_record(thread_id).ok().flatten() else {
        return;
    };

    let _ = db.set_setting("last_active_thread_id", &thread.id.to_string());
    if let Some(workspace_path) = runtime_workspace_path_for_thread(db, &thread) {
        let _ = db.set_setting("last_active_workspace_path", &workspace_path);
    }

    let has_linked_thread = thread
        .remote_thread_id()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some();
    if has_linked_thread {
        let _ = db.set_setting("pending_profile_thread_id", "");
    } else {
        let _ = db.set_setting("pending_profile_thread_id", &thread.id.to_string());
    }

    let _ = db.set_setting(
        crate::remote::SETTING_REMOTE_TELEGRAM_ACTIVATE_LOCAL_THREAD_ID,
        &thread.id.to_string(),
    );
}

fn runtime_workspace_path_for_thread(db: &AppDb, thread: &ThreadRecord) -> Option<String> {
    thread
        .worktree_path
        .as_deref()
        .map(str::trim)
        .filter(|path| thread.worktree_active && !path.is_empty())
        .map(|path| path.to_string())
        .or_else(|| db.workspace_path_for_local_thread(thread.id).ok().flatten())
}

fn chat_workspace_setting_key(chat_id: &str) -> String {
    format!("remote:telegram:chat:{chat_id}:workspace_id")
}

fn chat_thread_setting_key(chat_id: &str) -> String {
    format!("remote:telegram:chat:{chat_id}:thread_id")
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn is_guarded_auth_code_message(db: &AppDb, text: &str) -> bool {
    let expected_code = db
        .get_setting(crate::remote::SETTING_REMOTE_TELEGRAM_AUTH_EXPECTED_CODE)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if expected_code.is_empty() {
        return false;
    }

    let expires_at = db
        .get_setting(crate::remote::SETTING_REMOTE_TELEGRAM_AUTH_EXPIRES_AT)
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if expires_at > 0 && unix_now_secs() > expires_at {
        let _ = db.set_setting(
            crate::remote::SETTING_REMOTE_TELEGRAM_AUTH_EXPECTED_CODE,
            "",
        );
        let _ = db.set_setting(crate::remote::SETTING_REMOTE_TELEGRAM_AUTH_EXPIRES_AT, "0");
        return false;
    }

    if text.trim() != expected_code {
        return false;
    }

    let _ = db.set_setting(
        crate::remote::SETTING_REMOTE_TELEGRAM_AUTH_EXPECTED_CODE,
        "",
    );
    let _ = db.set_setting(crate::remote::SETTING_REMOTE_TELEGRAM_AUTH_EXPIRES_AT, "0");
    true
}

fn value_to_id_string(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(raw) = value.as_i64() {
        return Some(raw.to_string());
    }
    value
        .as_str()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn count_actions_from_raw_items(raw_items_json: Option<&str>) -> (usize, usize, usize) {
    let raw_items_json = raw_items_json.unwrap_or("").trim();
    if raw_items_json.is_empty() {
        return (0, 0, 0);
    }
    let Ok(value) = serde_json::from_str::<Value>(raw_items_json) else {
        return (0, 0, 0);
    };
    let Some(items) = value.as_array() else {
        return (0, 0, 0);
    };
    let mut command_count = 0usize;
    let mut file_edit_count = 0usize;
    let mut other_action_count = 0usize;
    for item in items {
        let kind = item
            .get("kind")
            .and_then(Value::as_str)
            .or_else(|| item.get("type").and_then(Value::as_str))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if kind.contains("command") {
            command_count += 1;
        } else if kind.contains("file") || kind.contains("patch") || kind.contains("edit") {
            file_edit_count += 1;
        } else if !kind.is_empty() {
            other_action_count += 1;
        }
    }
    (command_count, file_edit_count, other_action_count)
}
