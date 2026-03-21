use crate::services::chat::{
    AppDb, EnzimAgentConfigRecord, EnzimAgentLoopEventRecord, EnzimAgentLoopRecord,
    LocalChatTurnRecord,
};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

const MAX_LOOP_ITERATIONS: i64 = 25;
const MAX_LOOP_ERRORS: i64 = 3;
const MAX_LOOP_JSON_RETRIES: usize = 3;
const DEFAULT_LOOP_PROMPT_KEY: &str = "enzim_agent:default_loop_prompt";
const DEFAULT_LOOP_INSTRUCTIONS_KEY: &str = "enzim_agent:default_loop_instructions";
const TELEGRAM_QUESTION_SESSION_KEY: &str = "enzim_agent:telegram_question_session";
const ASK_STATE_KEY: &str = "enzim_agent:ask_state";
const ASK_MODEL_KEY: &str = "enzim_agent:ask_model_id";
const ASK_SYSTEM_PROMPT_OVERRIDE_KEY: &str = "enzim_agent:ask_system_prompt_override";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnzimAgentModelOption {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnzimAskState {
    pub current_chat_id: Option<String>,
    pub chats: Vec<EnzimAskChat>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnzimAskChat {
    pub id: String,
    pub title: String,
    pub messages: Vec<EnzimAskMessage>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnzimAskMessage {
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct EnzimAgentConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model_id: Option<String>,
    pub system_prompt_override: Option<String>,
    pub cached_models: Vec<EnzimAgentModelOption>,
    pub cached_models_refreshed_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingUserQuestion {
    pub loop_id: i64,
    pub question: String,
    pub asked_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoopDraftDefaults {
    pub prompt_text: String,
    pub instructions_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramQuestionSession {
    pub loop_id: i64,
    pub local_thread_id: i64,
    pub remote_thread_id: String,
    pub telegram_chat_id: String,
    pub question_message_id: String,
    pub question_text: String,
    pub started_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopPendingRequestOption {
    pub id: String,
    pub label: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopPendingRequest {
    pub request_id: i64,
    pub method: String,
    pub title: String,
    pub details: String,
    pub options: Vec<LoopPendingRequestOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct LoopDecision {
    action: String,
    message: Option<String>,
    reason: Option<String>,
    summary_for_user: Option<String>,
    request_option_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum LoopDriverAction {
    Continue {
        loop_id: i64,
        event_id: i64,
        message: String,
    },
    AskUser {
        loop_id: i64,
        question: String,
    },
    Finish {
        loop_id: i64,
        summary: String,
    },
    Respond {
        loop_id: i64,
        request_id: i64,
        option_label: String,
        payload: Value,
    },
}

#[derive(Clone, Debug)]
pub struct ProcessedAssistantTurn {
    pub loop_id: i64,
    pub turn_id: String,
    pub event_id: i64,
    pub action: LoopDriverAction,
}

fn now() -> i64 {
    crate::data::unix_now()
}

fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn save_telegram_question_session(
    db: &AppDb,
    session: Option<&TelegramQuestionSession>,
) -> Result<(), String> {
    let value = session
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| err.to_string())?
        .unwrap_or_default();
    db.set_setting(TELEGRAM_QUESTION_SESSION_KEY, &value)
        .map_err(|err| err.to_string())
}

pub fn active_telegram_question_session(db: &AppDb) -> Option<TelegramQuestionSession> {
    db.get_setting(TELEGRAM_QUESTION_SESSION_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<TelegramQuestionSession>(&raw).ok())
}

pub fn stop_telegram_question_session(db: &AppDb) -> Result<(), String> {
    save_telegram_question_session(db, None)
}

pub fn stop_telegram_question_session_for_loop(db: &AppDb, loop_id: i64) -> Result<(), String> {
    if active_telegram_question_session(db).is_some_and(|session| session.loop_id == loop_id) {
        stop_telegram_question_session(db)?;
    }
    Ok(())
}

pub fn start_telegram_question_session(
    db: &AppDb,
    loop_id: i64,
    remote_thread_id: &str,
    question: &str,
) -> Result<(), String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("Telegram question text is empty.".to_string());
    }
    let loop_record = db
        .get_enzim_agent_loop(loop_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "Loop record not found.".to_string())?;
    let account = db
        .remote_telegram_active_account()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            "No linked Telegram account is available for Enzim Agent questions.".to_string()
        })?;
    let thread_title = db
        .get_thread_record(loop_record.local_thread_id)
        .ok()
        .flatten()
        .map(|thread| thread.title)
        .unwrap_or_else(|| remote_thread_id.to_string());
    let client = crate::remote::telegram::TelegramClient::new(account.bot_token.clone())?;
    let body = format!(
        "<b>Enzim Agent question</b>\n<b>Thread:</b> {}\n\n{}\n\nReply directly to this message to answer.",
        escape_html(&thread_title),
        escape_html(question),
    );
    let message_id = client
        .send_html_message(&account.telegram_chat_id, &body, None)?
        .to_string();
    let session = TelegramQuestionSession {
        loop_id,
        local_thread_id: loop_record.local_thread_id,
        remote_thread_id: remote_thread_id.trim().to_string(),
        telegram_chat_id: account.telegram_chat_id,
        question_message_id: message_id,
        question_text: question.to_string(),
        started_at: now(),
    };
    save_telegram_question_session(db, Some(&session))?;
    crate::services::app::remote::start_background_worker();
    Ok(())
}

pub fn enqueue_telegram_question_answer_if_match(
    db: &AppDb,
    chat_id: &str,
    reply_to_message_id: Option<&str>,
    text: &str,
    incoming_message_id: Option<&str>,
    from_user_id: Option<&str>,
    from_username: Option<&str>,
) -> Result<bool, String> {
    let Some(session) = active_telegram_question_session(db) else {
        return Ok(false);
    };
    if session.telegram_chat_id != chat_id.trim() {
        return Ok(false);
    }
    if reply_to_message_id.map(str::trim) != Some(session.question_message_id.as_str()) {
        return Ok(false);
    }
    if active_loop_for_remote_thread(db, &session.remote_thread_id)
        .map(|loop_record| {
            loop_record.id != session.loop_id || loop_record.status != "waiting_user"
        })
        .unwrap_or(true)
    {
        stop_telegram_question_session(db)?;
        return Ok(false);
    }
    db.enqueue_remote_pending_prompt(
        session.local_thread_id,
        text,
        "telegram-loop-answer",
        Some(chat_id),
        incoming_message_id,
        from_user_id,
        from_username,
    )
    .map_err(|err| err.to_string())?;
    stop_telegram_question_session(db)?;
    Ok(true)
}

pub fn default_system_prompt() -> &'static str {
    "You are Enzim Agent, a loop supervisor for another coding agent.\n\
\n\
ROLE\n\
- Your job is to prevent the coding agent from stopping too early.\n\
- Your job is also to stop the loop once the visible history shows the task is done.\n\
- You supervise only from the loop prompt, looping instructions, and loop history.\n\
\n\
HARD CONSTRAINTS\n\
- Do not inspect files, tools, builds, git state, or any outside context.\n\
- Do not assume work is complete unless the history clearly shows it.\n\
- Do not invent validation, test results, or file changes that are not visible in the history.\n\
- Return JSON only. No prose, no markdown, no code fences.\n\
\n\
DECISION POLICY\n\
- Prefer `continue` while there is still concrete unfinished work in the visible history.\n\
- If a `pending_request` object is present, you may use `respond` to answer that runtime approval request instead of asking the coding agent for another text turn.\n\
- Use `ask_user` only when missing information must come from the human.\n\
- Use `finish` only when the task appears fully done from the visible history.\n\
- If the coding agent gave a summary plus next steps, that usually means the task is not done yet, so prefer `continue`.\n\
- If the latest coding-agent message says the requested implementation is complete and does not list remaining work, blockers, or next-step recommendations, prefer `finish`.\n\
- If the latest coding-agent message reports successful validation for the requested work, do not ask for repeated re-verification unless the history also shows an unresolved failure or missing requirement.\n\
- Do not keep the loop running just to ask the coding agent to verify again, re-check again, or continue again when the visible history already indicates completion.\n\
- Repeatedly asking for the same kind of follow-up without new unfinished work is a loop failure. Use `finish` instead.\n\
- Keep follow-up messages short, concrete, and user-like.\n\
\n\
OUTPUT FORMAT\n\
Return one JSON object with exactly these keys:\n\
- `action`: `continue` | `ask_user` | `finish` | `respond`\n\
- `message`: string or null\n\
- `reason`: short string\n\
- `summary_for_user`: string or null\n\
- `request_option_id`: string or null\n\
\n\
FIELD RULES\n\
- For `continue`, `message` must be a short follow-up prompt to the coding agent.\n\
- For `ask_user`, `message` must be the exact question for the human.\n\
- For `finish`, `summary_for_user` must clearly summarize what was completed.\n\
- For `respond`, `request_option_id` must exactly match one of the option ids from `pending_request.options`.\n\
- The loop history already includes the full prior messages. Do not summarize or rewrite the prior backend-agent messages inside your JSON.\n\
- Treat phrases like \"done\", \"completed\", \"implemented\", \"finished\", \"build passed\", \"tests passed\", or \"all requested changes are in place\" as strong completion signals unless the same visible message also lists remaining work.\n\
- Use null for fields that do not apply.\n\
\n\
EXAMPLES\n\
{\"action\":\"continue\",\"message\":\"Continue and finish the remaining migration work. Do not stop until the app builds successfully.\",\"reason\":\"The visible history still shows unresolved compile errors and no successful build.\",\"summary_for_user\":null,\"request_option_id\":null}\n\
\n\
{\"action\":\"ask_user\",\"message\":\"Do you want the migration to keep the existing pages router structure, or switch to the app router as part of this task?\",\"reason\":\"A product decision from the user is required before the remaining work can be completed.\",\"summary_for_user\":null,\"request_option_id\":null}\n\
\n\
{\"action\":\"respond\",\"message\":null,\"reason\":\"The runtime is waiting for approval on a clearly required cleanup command, and the available option should be accepted.\",\"summary_for_user\":null,\"request_option_id\":\"accept\"}\n\
\n\
{\"action\":\"finish\",\"message\":null,\"reason\":\"The visible history indicates the requested work is complete, so repeating verification would be unnecessary.\",\"summary_for_user\":\"The task appears complete based on the loop history, including the reported validation result.\",\"request_option_id\":null}"
}

pub fn default_loop_prompt() -> &'static str {
    "Continue this task from the current state and do not stop early."
}

pub fn default_loop_instructions() -> &'static str {
    ""
}

pub fn ask_system_prompt() -> &'static str {
    "You are Enzim Agent, Enzim's quick helper AI.\n\
\n\
ROLE\n\
- This conversation is separate from the project thread.\n\
- Answer the user's question directly and helpfully.\n\
- Use markdown when it improves readability.\n\
\n\
CONSTRAINTS\n\
- Do not claim you inspected files, ran commands, or used tools unless the user explicitly provided that context in this chat.\n\
- Do not refer to hidden project state or background loop state.\n\
- Keep answers concise unless the user asks for depth."
}

pub fn load_ask_system_prompt_override(db: &AppDb) -> Option<String> {
    db.get_setting(ASK_SYSTEM_PROMPT_OVERRIDE_KEY)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn save_ask_system_prompt_override(db: &AppDb, prompt: Option<&str>) -> Result<(), String> {
    let value = prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    db.set_setting(ASK_SYSTEM_PROMPT_OVERRIDE_KEY, value)
        .map_err(|err| err.to_string())
}

pub fn effective_ask_system_prompt(db: &AppDb) -> String {
    load_ask_system_prompt_override(db).unwrap_or_else(|| ask_system_prompt().to_string())
}

impl Default for EnzimAgentConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: None,
            model_id: None,
            system_prompt_override: None,
            cached_models: Vec::new(),
            cached_models_refreshed_at: None,
            updated_at: now(),
        }
    }
}

fn parse_cached_models(raw: Option<&str>) -> Vec<EnzimAgentModelOption> {
    raw.and_then(|value| serde_json::from_str::<Vec<EnzimAgentModelOption>>(value).ok())
        .unwrap_or_default()
}

fn serialize_cached_models(models: &[EnzimAgentModelOption]) -> Option<String> {
    serde_json::to_string(models).ok()
}

pub fn load_config(db: &AppDb) -> EnzimAgentConfig {
    let Some(record) = db.enzim_agent_config().ok().flatten() else {
        return EnzimAgentConfig::default();
    };
    EnzimAgentConfig {
        base_url: record.base_url,
        api_key: record.api_key,
        model_id: record.model_id,
        system_prompt_override: record.system_prompt_override,
        cached_models: parse_cached_models(record.cached_models_json.as_deref()),
        cached_models_refreshed_at: record.cached_models_refreshed_at,
        updated_at: record.updated_at,
    }
}

pub fn save_config(db: &AppDb, config: &EnzimAgentConfig) -> Result<(), String> {
    let record = EnzimAgentConfigRecord {
        base_url: config.base_url.trim().to_string(),
        api_key: config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        model_id: config
            .model_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        system_prompt_override: config
            .system_prompt_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        cached_models_json: serialize_cached_models(&config.cached_models),
        cached_models_refreshed_at: config.cached_models_refreshed_at,
        updated_at: now(),
    };
    db.upsert_enzim_agent_config(&record)
        .map_err(|err| err.to_string())
}

pub fn load_loop_draft_defaults(db: &AppDb) -> LoopDraftDefaults {
    let prompt_text = db
        .get_setting(DEFAULT_LOOP_PROMPT_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    let instructions_text = db
        .get_setting(DEFAULT_LOOP_INSTRUCTIONS_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    LoopDraftDefaults {
        prompt_text,
        instructions_text,
    }
}

pub fn effective_loop_draft_defaults(db: &AppDb) -> LoopDraftDefaults {
    let stored = load_loop_draft_defaults(db);
    LoopDraftDefaults {
        prompt_text: if stored.prompt_text.trim().is_empty() {
            default_loop_prompt().to_string()
        } else {
            stored.prompt_text
        },
        instructions_text: if stored.instructions_text.trim().is_empty() {
            default_loop_instructions().to_string()
        } else {
            stored.instructions_text
        },
    }
}

pub fn save_loop_draft_defaults(db: &AppDb, defaults: &LoopDraftDefaults) -> Result<(), String> {
    db.set_setting(DEFAULT_LOOP_PROMPT_KEY, defaults.prompt_text.trim())
        .map_err(|err| err.to_string())?;
    db.set_setting(
        DEFAULT_LOOP_INSTRUCTIONS_KEY,
        defaults.instructions_text.trim(),
    )
    .map_err(|err| err.to_string())
}

pub fn load_ask_state(db: &AppDb) -> EnzimAskState {
    db.get_setting(ASK_STATE_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<EnzimAskState>(&raw).ok())
        .unwrap_or_default()
}

pub fn save_ask_state(db: &AppDb, state: &EnzimAskState) -> Result<(), String> {
    let mut normalized = state.clone();
    normalized.chats.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    let raw = serde_json::to_string(&normalized).map_err(|err| err.to_string())?;
    db.set_setting(ASK_STATE_KEY, &raw)
        .map_err(|err| err.to_string())
}

pub fn load_ask_model_choice(db: &AppDb) -> Option<String> {
    db.get_setting(ASK_MODEL_KEY)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn save_ask_model_choice(db: &AppDb, model_id: &str) -> Result<(), String> {
    db.set_setting(ASK_MODEL_KEY, model_id.trim())
        .map_err(|err| err.to_string())
}

pub fn ask_model_options(config: &EnzimAgentConfig) -> Vec<EnzimAgentModelOption> {
    let mut models = config.cached_models.clone();
    if let Some(selected) = config
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !models.iter().any(|model| model.id == selected) {
            models.insert(
                0,
                EnzimAgentModelOption {
                    id: selected.to_string(),
                    display_name: selected.to_string(),
                },
            );
        }
    }
    models
}

pub fn effective_ask_model_id(db: &AppDb) -> Option<String> {
    let config = load_config(db);
    let models = ask_model_options(&config);
    if let Some(saved) = load_ask_model_choice(db) {
        if models.iter().any(|model| model.id == saved) {
            return Some(saved);
        }
    }
    config
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| models.first().map(|model| model.id.clone()))
}

fn config_system_prompt(config: &EnzimAgentConfig) -> String {
    config
        .system_prompt_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_system_prompt())
        .to_string()
}

fn normalized_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

fn build_headers(api_key: Option<&str>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(key) = api_key.map(str::trim).filter(|value| !value.is_empty()) {
        let value = format!("Bearer {key}");
        let header = HeaderValue::from_str(&value).map_err(|err| err.to_string())?;
        headers.insert(AUTHORIZATION, header);
    }
    Ok(headers)
}

fn build_client(api_key: Option<&str>) -> Result<Client, String> {
    let headers = build_headers(api_key)?;
    Client::builder()
        .timeout(Duration::from_secs(45))
        .default_headers(headers)
        .build()
        .map_err(|err| err.to_string())
}

fn endpoint(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        normalized_base_url(base_url),
        path.trim_start_matches('/')
    )
}

fn extract_response_text(value: &Value) -> Option<String> {
    let message = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))?;
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let parts = message.get("content").and_then(Value::as_array)?;
    let mut out = String::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            out.push_str(text);
        }
    }
    Some(out)
}

fn parse_error_body(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn extract_agent_message_text(item: &Value) -> Option<String> {
    if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
        return None;
    }

    if let Some(text) = item.get("text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return Some(text.to_string());
        }
    }

    let content = item.get("content").and_then(Value::as_array)?;
    let mut out = String::new();
    for part in content {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            out.push_str(text);
            continue;
        }
        if let Some(text) = part
            .get("text")
            .and_then(|value| value.get("value"))
            .and_then(Value::as_str)
        {
            out.push_str(text);
            continue;
        }
        if let Some(text) = part.get("value").and_then(Value::as_str) {
            out.push_str(text);
            continue;
        }
        if let Some(text) = part
            .get("content")
            .and_then(|value| value.get("text"))
            .and_then(Value::as_str)
        {
            out.push_str(text);
        }
    }

    let out = out.trim().to_string();
    if out.is_empty() { None } else { Some(out) }
}

fn effective_assistant_text(turn: &LocalChatTurnRecord) -> String {
    if !turn.assistant_text.trim().is_empty() {
        return turn.assistant_text.trim().to_string();
    }
    let Some(raw_items) = turn.raw_items_json.as_deref() else {
        return String::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<Value>>(raw_items) else {
        return String::new();
    };
    items
        .iter()
        .filter_map(extract_agent_message_text)
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string()
}

fn cached_turn_error_message(db: &AppDb, remote_thread_id: &str, turn_id: &str) -> Option<String> {
    let raw = db
        .get_setting(&format!("thread_turn_errors:{remote_thread_id}"))
        .ok()
        .flatten()?;
    let entries = serde_json::from_str::<Vec<Value>>(&raw).ok()?;
    entries
        .iter()
        .find(|entry| entry.get("turnId").and_then(Value::as_str) == Some(turn_id))
        .and_then(|entry| entry.get("message").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn build_failed_turn_diagnostics(
    db: &AppDb,
    remote_thread_id: &str,
    turn: &LocalChatTurnRecord,
) -> String {
    let mut lines = vec![
        "The coding agent turn failed.".to_string(),
        format!("Turn ID: {}", turn.external_turn_id),
        format!("Turn status: {}", turn.status),
    ];

    if let Some(message) = cached_turn_error_message(db, remote_thread_id, &turn.external_turn_id) {
        lines.push(format!("Runtime error: {message}"));
    } else {
        lines.push(
            "No detailed runtime error was captured from the runtime event stream.".to_string(),
        );
    }

    let assistant_text = effective_assistant_text(turn);
    if !assistant_text.trim().is_empty() {
        lines.push("Latest agent output before failure:".to_string());
        lines.push(assistant_text);
    }

    lines.join("\n")
}

pub fn detailed_runtime_error_for_turn(
    db: &AppDb,
    remote_thread_id: &str,
    local_thread_id: i64,
    turn_id: &str,
) -> Option<String> {
    let turns = db
        .list_local_chat_turns_for_local_thread(local_thread_id)
        .ok()?;
    let turn = turns
        .into_iter()
        .find(|turn| turn.external_turn_id == turn_id)?;
    Some(build_failed_turn_diagnostics(db, remote_thread_id, &turn))
}

fn parse_models(value: &Value) -> Vec<EnzimAgentModelOption> {
    let mut out = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let display_name = entry
                .get("display_name")
                .and_then(Value::as_str)
                .or_else(|| entry.get("name").and_then(Value::as_str))
                .unwrap_or(&id)
                .to_string();
            Some(EnzimAgentModelOption { id, display_name })
        })
        .collect::<Vec<_>>();
    out.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    out
}

pub fn refresh_models(db: &AppDb) -> Result<Vec<EnzimAgentModelOption>, String> {
    let mut config = load_config(db);
    if normalized_base_url(&config.base_url).is_empty() {
        return Err("Enzim Agent base URL is empty.".to_string());
    }
    let client = build_client(config.api_key.as_deref())?;
    let url = endpoint(&config.base_url, "/models");
    let response = client.get(url).send().map_err(|err| err.to_string())?;
    let status = response.status();
    let value = response.json::<Value>().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(
            parse_error_body(&value).unwrap_or_else(|| format!("Model refresh failed: {status}"))
        );
    }
    let models = parse_models(&value);
    config.cached_models = models.clone();
    config.cached_models_refreshed_at = Some(now());
    if config
        .model_id
        .as_deref()
        .map(|selected| models.iter().any(|model| model.id == selected))
        != Some(true)
    {
        config.model_id = models.first().map(|model| model.id.clone());
    }
    save_config(db, &config)?;
    Ok(models)
}

fn extract_json_object(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_string());
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(trimmed[start..=end].to_string())
}

fn parse_decision(raw: &str) -> Result<LoopDecision, String> {
    let json_slice = extract_json_object(raw)
        .ok_or_else(|| "Agent response did not contain JSON.".to_string())?;
    let decision =
        serde_json::from_str::<LoopDecision>(&json_slice).map_err(|err| err.to_string())?;
    match decision.action.as_str() {
        "continue" => {
            if decision
                .message
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err("Continue decision missing message.".to_string());
            }
        }
        "ask_user" => {
            if decision
                .message
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err("Ask-user decision missing message.".to_string());
            }
        }
        "finish" => {
            if decision
                .summary_for_user
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err("Finish decision missing summary_for_user.".to_string());
            }
        }
        "respond" => {
            if decision
                .request_option_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err("Respond decision missing request_option_id.".to_string());
            }
        }
        other => {
            return Err(format!("Unsupported Enzim Agent action `{other}`."));
        }
    }
    Ok(decision)
}

fn format_invalid_json_error(last_error: &str) -> String {
    format!(
        "Enzim Agent did not return valid JSON after {} retries. Last error: {}",
        MAX_LOOP_JSON_RETRIES,
        last_error.trim()
    )
}

fn render_history(events: &[EnzimAgentLoopEventRecord]) -> Vec<Value> {
    events
        .iter()
        .map(|event| {
            json!({
                "sequence": event.sequence_no,
                "event_kind": event.event_kind,
                "author_kind": event.author_kind,
                "text": event.full_text.as_deref().unwrap_or(""),
            })
        })
        .collect()
}

fn call_loop_model(
    config: &EnzimAgentConfig,
    system_prompt: &str,
    loop_record: &EnzimAgentLoopRecord,
    events: &[EnzimAgentLoopEventRecord],
    pending_request: Option<&LoopPendingRequest>,
) -> Result<LoopDecision, String> {
    let model_id = config
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Enzim Agent model is not configured.".to_string())?;
    let base_url = normalized_base_url(&config.base_url);
    if base_url.is_empty() {
        return Err("Enzim Agent base URL is not configured.".to_string());
    }

    let body = json!({
        "model": model_id,
        "temperature": 0.1,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": json!({
                "loop_prompt": loop_record.prompt_text,
                "loop_instructions": loop_record.instructions_text,
                "history": render_history(events),
                "current_status": loop_record.status,
                "iteration_count": loop_record.iteration_count,
                "pending_request": pending_request,
            }).to_string() }
        ]
    });

    let client = build_client(config.api_key.as_deref())?;
    let url = endpoint(&base_url, "/chat/completions");
    let mut last_json_error = String::new();

    for attempt in 0..=MAX_LOOP_JSON_RETRIES {
        let response = client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|err| err.to_string())?;
        let status = response.status();
        let value = response.json::<Value>().map_err(|err| err.to_string())?;
        if !status.is_success() {
            return Err(parse_error_body(&value)
                .unwrap_or_else(|| format!("Loop model request failed: {status}")));
        }

        let raw = match extract_response_text(&value) {
            Some(raw) => raw,
            None => {
                last_json_error = "Loop model response missing content.".to_string();
                if attempt < MAX_LOOP_JSON_RETRIES {
                    continue;
                }
                return Err(format_invalid_json_error(&last_json_error));
            }
        };

        match parse_decision(&raw) {
            Ok(decision) => return Ok(decision),
            Err(err) => {
                last_json_error = err;
                if attempt < MAX_LOOP_JSON_RETRIES {
                    continue;
                }
                return Err(format_invalid_json_error(&last_json_error));
            }
        }
    }

    Err(format_invalid_json_error(
        if last_json_error.trim().is_empty() {
            "Unknown JSON formatting error."
        } else {
            &last_json_error
        },
    ))
}

pub fn ask_chat_completion(
    config: &EnzimAgentConfig,
    system_prompt: &str,
    model_id: &str,
    messages: &[EnzimAskMessage],
) -> Result<String, String> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err("Ask model is not configured.".to_string());
    }
    let base_url = normalized_base_url(&config.base_url);
    if base_url.is_empty() {
        return Err("Enzim Agent base URL is not configured.".to_string());
    }

    let payload_messages = std::iter::once(json!({
        "role": "system",
        "content": system_prompt.trim(),
    }))
    .chain(messages.iter().filter_map(|message| {
        let role = message.role.trim();
        let content = message.content.trim();
        if content.is_empty() {
            return None;
        }
        let normalized_role = match role {
            "assistant" => "assistant",
            _ => "user",
        };
        Some(json!({
            "role": normalized_role,
            "content": content,
        }))
    }))
    .collect::<Vec<_>>();

    let body = json!({
        "model": model_id,
        "temperature": 0.4,
        "messages": payload_messages,
    });

    let client = build_client(config.api_key.as_deref())?;
    let url = endpoint(&base_url, "/chat/completions");
    let response = client
        .post(&url)
        .json(&body)
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let value = response.json::<Value>().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(
            parse_error_body(&value).unwrap_or_else(|| format!("Ask request failed: {status}"))
        );
    }
    extract_response_text(&value)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "Ask response did not include any message content.".to_string())
}

fn recent_agent_followup(events: &[EnzimAgentLoopEventRecord]) -> Option<String> {
    events
        .iter()
        .rev()
        .find(|event| event.event_kind == "agent_followup")
        .and_then(|event| event.full_text.clone())
}

fn apply_decision(
    db: &AppDb,
    loop_record: &EnzimAgentLoopRecord,
    decision: &LoopDecision,
    pending_request: Option<&LoopPendingRequest>,
) -> Result<LoopDriverAction, String> {
    match decision.action.as_str() {
        "continue" => {
            if loop_record.iteration_count >= MAX_LOOP_ITERATIONS {
                db.update_enzim_agent_loop_progress(
                    loop_record.id,
                    "paused_error",
                    None,
                    0,
                    1,
                    Some("Loop reached the maximum iteration limit."),
                    None,
                )
                .map_err(|err| err.to_string())?;
                return Err("Loop reached the maximum iteration limit.".to_string());
            }

            let message = decision
                .message
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string();
            if recent_agent_followup(
                &db.list_enzim_agent_loop_events(loop_record.id)
                    .map_err(|err| err.to_string())?,
            )
            .as_deref()
                == Some(message.as_str())
            {
                db.update_enzim_agent_loop_progress(
                    loop_record.id,
                    "paused_error",
                    None,
                    0,
                    1,
                    Some("Enzim Agent generated the same follow-up twice and paused the loop."),
                    None,
                )
                .map_err(|err| err.to_string())?;
                return Err(
                    "Enzim Agent generated a duplicate follow-up and paused the loop.".to_string(),
                );
            }
            let decision_json = serde_json::to_string(decision).ok();
            let event = db
                .append_enzim_agent_loop_event(
                    loop_record.id,
                    "agent_followup",
                    "enzim_agent",
                    None,
                    Some(&message),
                    None,
                    decision_json.as_deref(),
                )
                .map_err(|err| err.to_string())?;
            db.update_enzim_agent_loop_progress(
                loop_record.id,
                "waiting_runtime",
                None,
                1,
                0,
                None,
                None,
            )
            .map_err(|err| err.to_string())?;
            Ok(LoopDriverAction::Continue {
                loop_id: loop_record.id,
                event_id: event.id,
                message,
            })
        }
        "ask_user" => {
            let question = decision
                .message
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string();
            let decision_json = serde_json::to_string(decision).ok();
            db.append_enzim_agent_loop_event(
                loop_record.id,
                "agent_question",
                "enzim_agent",
                None,
                Some(&question),
                None,
                decision_json.as_deref(),
            )
            .map_err(|err| err.to_string())?;
            db.update_enzim_agent_loop_progress(
                loop_record.id,
                "waiting_user",
                None,
                0,
                0,
                None,
                None,
            )
            .map_err(|err| err.to_string())?;
            Ok(LoopDriverAction::AskUser {
                loop_id: loop_record.id,
                question,
            })
        }
        "finish" => {
            let summary = decision
                .summary_for_user
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string();
            let decision_json = serde_json::to_string(decision).ok();
            db.append_enzim_agent_loop_event(
                loop_record.id,
                "agent_finish",
                "enzim_agent",
                None,
                Some(&summary),
                None,
                decision_json.as_deref(),
            )
            .map_err(|err| err.to_string())?;
            stop_telegram_question_session_for_loop(db, loop_record.id)?;
            db.update_enzim_agent_loop_status(
                loop_record.id,
                "finished",
                None,
                Some(&summary),
                Some(now()),
            )
            .map_err(|err| err.to_string())?;
            Ok(LoopDriverAction::Finish {
                loop_id: loop_record.id,
                summary,
            })
        }
        "respond" => {
            let pending_request = pending_request.ok_or_else(|| {
                "Respond decision was returned without a pending request.".to_string()
            })?;
            let option_id = decision
                .request_option_id
                .as_deref()
                .unwrap_or_default()
                .trim();
            let option = pending_request
                .options
                .iter()
                .find(|option| option.id == option_id)
                .ok_or_else(|| {
                    "Respond decision selected an unknown request option.".to_string()
                })?;
            let decision_json = serde_json::to_string(decision).ok();
            db.append_enzim_agent_loop_event(
                loop_record.id,
                "agent_request_response",
                "enzim_agent",
                Some(&format!("request:{}", pending_request.request_id)),
                Some(&format!("{}: {}", pending_request.title, option.label)),
                None,
                decision_json.as_deref(),
            )
            .map_err(|err| err.to_string())?;
            db.update_enzim_agent_loop_progress(
                loop_record.id,
                "waiting_runtime",
                None,
                0,
                0,
                None,
                None,
            )
            .map_err(|err| err.to_string())?;
            Ok(LoopDriverAction::Respond {
                loop_id: loop_record.id,
                request_id: pending_request.request_id,
                option_label: option.label.clone(),
                payload: option.payload.clone(),
            })
        }
        _ => Err("Unsupported Enzim Agent action.".to_string()),
    }
}

fn evaluate_loop_from_history(
    db: &AppDb,
    loop_record: &EnzimAgentLoopRecord,
    pending_request: Option<&LoopPendingRequest>,
) -> Result<LoopDriverAction, String> {
    if loop_record.error_count >= MAX_LOOP_ERRORS {
        return Err("Loop paused because it reached the maximum error count.".to_string());
    }
    db.update_enzim_agent_loop_progress(loop_record.id, "evaluating", None, 0, 0, None, None)
        .map_err(|err| err.to_string())?;
    let config = load_config(db);
    let system_prompt = loop_record.system_prompt_snapshot.clone();
    let events = db
        .list_enzim_agent_loop_events(loop_record.id)
        .map_err(|err| err.to_string())?;
    let decision = call_loop_model(
        &config,
        &system_prompt,
        loop_record,
        &events,
        pending_request,
    )?;
    apply_decision(db, loop_record, &decision, pending_request)
}

pub fn start_loop(
    db: &AppDb,
    local_thread_id: i64,
    prompt: &str,
    instructions: &str,
) -> Result<EnzimAgentLoopRecord, String> {
    let prompt = prompt.trim();
    let instructions = instructions.trim();
    if prompt.is_empty() {
        return Err("Prompt is empty.".to_string());
    }
    if db
        .active_enzim_agent_loop_for_local_thread(local_thread_id)
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Err("This thread already has an active Enzim Agent loop.".to_string());
    }
    let config = load_config(db);
    let base_url = normalized_base_url(&config.base_url);
    let model_id = config
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Configure an Enzim Agent model first in Settings.".to_string())?;
    if base_url.is_empty() {
        return Err("Configure an Enzim Agent base URL first in Settings.".to_string());
    }
    let thread = db
        .get_thread_record(local_thread_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "Thread record not found.".to_string())?;
    let backend_kind = db
        .get_codex_profile(thread.profile_id)
        .map_err(|err| err.to_string())?
        .map(|profile| profile.backend_kind)
        .unwrap_or_else(|| "codex".to_string());
    let loop_record = db
        .create_enzim_agent_loop(
            local_thread_id,
            "waiting_runtime",
            prompt,
            instructions,
            &backend_kind,
            thread.codex_thread_id.as_deref(),
            &base_url,
            model_id,
            &config_system_prompt(&config),
        )
        .map_err(|err| err.to_string())?;
    db.append_enzim_agent_loop_event(
        loop_record.id,
        "initial_prompt",
        "human",
        None,
        Some(prompt),
        None,
        None,
    )
    .map_err(|err| err.to_string())?;
    Ok(loop_record)
}

pub fn active_loop_for_remote_thread(
    db: &AppDb,
    remote_thread_id: &str,
) -> Option<EnzimAgentLoopRecord> {
    let local_thread_id = db
        .get_thread_record_by_remote_thread_id(remote_thread_id)
        .ok()
        .flatten()?
        .id;
    db.active_enzim_agent_loop_for_local_thread(local_thread_id)
        .ok()
        .flatten()
}

pub fn cancel_active_loop_for_remote_thread(
    db: &AppDb,
    remote_thread_id: &str,
) -> Result<(), String> {
    let Some(loop_record) = active_loop_for_remote_thread(db, remote_thread_id) else {
        return Ok(());
    };
    stop_telegram_question_session_for_loop(db, loop_record.id)?;
    db.update_enzim_agent_loop_status(loop_record.id, "cancelled", None, None, Some(now()))
        .map_err(|err| err.to_string())
}

pub fn cancel_active_loop_for_local_thread(db: &AppDb, local_thread_id: i64) -> Result<(), String> {
    let Some(loop_record) = db
        .active_enzim_agent_loop_for_local_thread(local_thread_id)
        .map_err(|err| err.to_string())?
    else {
        return Ok(());
    };
    stop_telegram_question_session_for_loop(db, loop_record.id)?;
    db.update_enzim_agent_loop_status(loop_record.id, "cancelled", None, None, Some(now()))
        .map_err(|err| err.to_string())
}

pub fn pending_question(db: &AppDb, remote_thread_id: &str) -> Option<PendingUserQuestion> {
    let loop_record = active_loop_for_remote_thread(db, remote_thread_id)?;
    if loop_record.status != "waiting_user" {
        return None;
    }
    let events = db.list_enzim_agent_loop_events(loop_record.id).ok()?;
    let event = events
        .into_iter()
        .rev()
        .find(|event| event.event_kind == "agent_question")?;
    Some(PendingUserQuestion {
        loop_id: loop_record.id,
        question: event.full_text.unwrap_or_default(),
        asked_at: event.created_at,
    })
}

pub fn has_pending_question(db: &AppDb, remote_thread_id: &str) -> bool {
    pending_question(db, remote_thread_id).is_some()
}

pub fn process_user_answer(
    db: &AppDb,
    remote_thread_id: &str,
    answer: &str,
    source: &str,
) -> Result<LoopDriverAction, String> {
    let Some(loop_record) = active_loop_for_remote_thread(db, remote_thread_id) else {
        return Err("No active Enzim Agent loop for this thread.".to_string());
    };
    if loop_record.status != "waiting_user" {
        return Err("Enzim Agent is not waiting for a user answer on this thread.".to_string());
    }
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("Answer is empty.".to_string());
    }
    db.append_enzim_agent_loop_event(
        loop_record.id,
        "user_answer",
        source,
        None,
        Some(answer),
        None,
        None,
    )
    .map_err(|err| err.to_string())?;
    let action = evaluate_loop_from_history(db, &loop_record, None).map_err(|err| {
        let _ = db.update_enzim_agent_loop_progress(
            loop_record.id,
            "paused_error",
            None,
            0,
            1,
            Some(&err),
            None,
        );
        err
    })?;
    stop_telegram_question_session_for_loop(db, loop_record.id)?;
    Ok(action)
}

fn latest_completed_turn(
    loop_record: &EnzimAgentLoopRecord,
    turns: &[LocalChatTurnRecord],
) -> Option<LocalChatTurnRecord> {
    turns
        .iter()
        .rev()
        .find(|turn| {
            if turn.created_at < loop_record.created_at {
                return false;
            }
            turn.external_turn_id
                != loop_record
                    .last_seen_external_turn_id
                    .clone()
                    .unwrap_or_default()
                && (turn.status == "failed"
                    || turn.status == "completed"
                    || turn.completed_at.is_some())
                && (!effective_assistant_text(turn).trim().is_empty()
                    || turn.status == "failed"
                    || turn.completed_at.is_some())
        })
        .cloned()
}

pub fn process_waiting_runtime_turn(
    db: &AppDb,
    remote_thread_id: &str,
) -> Result<Option<ProcessedAssistantTurn>, String> {
    let Some(loop_record) = active_loop_for_remote_thread(db, remote_thread_id) else {
        return Ok(None);
    };
    if loop_record.status != "waiting_runtime" && loop_record.status != "active" {
        return Ok(None);
    }

    let turns = db
        .list_local_chat_turns_for_local_thread(loop_record.local_thread_id)
        .map_err(|err| err.to_string())?;
    let Some(turn) = latest_completed_turn(&loop_record, &turns) else {
        return Ok(None);
    };

    if turn.status == "failed" {
        let detailed_error = build_failed_turn_diagnostics(db, remote_thread_id, &turn);
        let short_error = cached_turn_error_message(db, remote_thread_id, &turn.external_turn_id)
            .map(|message| format!("The coding agent turn failed: {message}"))
            .unwrap_or_else(|| {
                "The coding agent turn failed. Open Turn Details for diagnostics.".to_string()
            });
        db.append_enzim_agent_loop_event(
            loop_record.id,
            "runtime_error",
            "runtime",
            Some(&turn.external_turn_id),
            Some(&detailed_error),
            None,
            None,
        )
        .map_err(|err| err.to_string())?;
        db.update_enzim_agent_loop_progress(
            loop_record.id,
            "paused_error",
            Some(&turn.external_turn_id),
            0,
            1,
            Some(&short_error),
            None,
        )
        .map_err(|err| err.to_string())?;
        return Err(format!("{short_error} Enzim Agent paused the loop."));
    }

    let assistant_text = effective_assistant_text(&turn);
    let assistant_text = if assistant_text.trim().is_empty() {
        "The coding agent completed a turn without a plain-text summary.".to_string()
    } else {
        assistant_text
    };

    let event = db
        .append_enzim_agent_loop_event(
            loop_record.id,
            "assistant_reply",
            "assistant",
            Some(&turn.external_turn_id),
            Some(&assistant_text),
            None,
            None,
        )
        .map_err(|err| err.to_string())?;
    db.update_enzim_agent_loop_progress(
        loop_record.id,
        "waiting_runtime",
        Some(&turn.external_turn_id),
        0,
        0,
        None,
        None,
    )
    .map_err(|err| err.to_string())?;

    let action = evaluate_loop_from_history(db, &loop_record, None).map_err(|err| {
        let _ = db.update_enzim_agent_loop_progress(
            loop_record.id,
            "paused_error",
            Some(&turn.external_turn_id),
            0,
            1,
            Some(&err),
            None,
        );
        err
    })?;
    Ok(Some(ProcessedAssistantTurn {
        loop_id: loop_record.id,
        turn_id: turn.external_turn_id,
        event_id: event.id,
        action,
    }))
}

pub fn process_pending_request(
    db: &AppDb,
    remote_thread_id: &str,
    pending_request: &LoopPendingRequest,
) -> Result<LoopDriverAction, String> {
    let Some(loop_record) = active_loop_for_remote_thread(db, remote_thread_id) else {
        return Err("No active Enzim Agent loop for this thread.".to_string());
    };
    if loop_record.status != "waiting_runtime" && loop_record.status != "active" {
        return Err("Enzim Agent is not waiting on the coding agent for this thread.".to_string());
    }

    let request_key = format!("request:{}", pending_request.request_id);
    let events = db
        .list_enzim_agent_loop_events(loop_record.id)
        .map_err(|err| err.to_string())?;
    if !events.iter().any(|event| {
        event.event_kind == "runtime_request"
            && event.external_turn_id.as_deref() == Some(request_key.as_str())
    }) {
        let option_lines = pending_request
            .options
            .iter()
            .map(|option| format!("- {} ({})", option.label, option.id))
            .collect::<Vec<_>>()
            .join("\n");
        let full_text = if option_lines.is_empty() {
            format!("{}\n{}", pending_request.title, pending_request.details)
        } else {
            format!(
                "{}\n{}\nOptions:\n{}",
                pending_request.title, pending_request.details, option_lines
            )
        };
        db.append_enzim_agent_loop_event(
            loop_record.id,
            "runtime_request",
            "runtime",
            Some(&request_key),
            Some(&full_text),
            None,
            None,
        )
        .map_err(|err| err.to_string())?;
    }

    evaluate_loop_from_history(db, &loop_record, Some(pending_request)).map_err(|err| {
        let _ = db.update_enzim_agent_loop_progress(
            loop_record.id,
            "paused_error",
            None,
            0,
            1,
            Some(&err),
            None,
        );
        err
    })
}

pub fn mark_followup_dispatched(
    db: &AppDb,
    _loop_id: i64,
    event_id: i64,
    remote_thread_id: &str,
    turn_id: &str,
) -> Result<(), String> {
    db.update_enzim_agent_loop_event(event_id, Some(turn_id), None)
        .map_err(|err| err.to_string())?;
    db.mark_enzim_agent_turn_origin(remote_thread_id, turn_id, "enzim_agent")
        .map_err(|err| err.to_string())
}

pub fn record_dispatch_error(db: &AppDb, loop_id: i64, error: &str) -> Result<(), String> {
    let message = error.trim();
    if message.is_empty() {
        return Ok(());
    }
    db.update_enzim_agent_loop_progress(loop_id, "paused_error", None, 0, 1, Some(message), None)
        .map_err(|err| err.to_string())
}

pub fn latest_finished_summary(db: &AppDb, local_thread_id: i64) -> Option<String> {
    let conn = db.connection().borrow();
    let mut stmt = conn
        .prepare(
            "SELECT final_summary_text
             FROM enzim_agent_loops
             WHERE local_thread_id = ?1
               AND status = 'finished'
               AND final_summary_text IS NOT NULL
             ORDER BY finished_at DESC, id DESC
             LIMIT 1",
        )
        .ok()?;
    let mut rows = stmt.query([local_thread_id]).ok()?;
    rows.next().ok().flatten().and_then(|row| row.get(0).ok())
}
