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
const DEFAULT_LOOP_PROMPT_KEY: &str = "enzim_agent:default_loop_prompt";
const DEFAULT_LOOP_INSTRUCTIONS_KEY: &str = "enzim_agent:default_loop_instructions";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnzimAgentModelOption {
    pub id: String,
    pub display_name: String,
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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct LoopDecision {
    action: String,
    message: Option<String>,
    compacted_last_message: Option<String>,
    reason: Option<String>,
    summary_for_user: Option<String>,
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

pub fn default_system_prompt() -> &'static str {
    "You are Enzim Agent, a loop supervisor for another coding agent.\n\
You are not allowed to inspect files, builds, git state, tools, or any outside context.\n\
You must decide only from the loop prompt, looping instructions, and the loop history provided.\n\
Your job is to stop the coding agent from ending too early.\n\
Prefer action=continue unless the work is clearly finished or you clearly need missing user input.\n\
If you are unsure and the missing information can only come from the human, return action=ask_user.\n\
Never invent validation that is not visible in the history.\n\
Return strict JSON with keys: action, message, compacted_last_message, reason, summary_for_user.\n\
For action=continue, message must be a short user-style follow-up prompt.\n\
For action=ask_user, message must be the exact question for the human.\n\
For action=finish, summary_for_user must clearly explain what was completed."
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

pub fn save_loop_draft_defaults(db: &AppDb, defaults: &LoopDraftDefaults) -> Result<(), String> {
    db.set_setting(DEFAULT_LOOP_PROMPT_KEY, defaults.prompt_text.trim())
        .map_err(|err| err.to_string())?;
    db.set_setting(
        DEFAULT_LOOP_INSTRUCTIONS_KEY,
        defaults.instructions_text.trim(),
    )
    .map_err(|err| err.to_string())
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
    format!("{}/{}", normalized_base_url(base_url), path.trim_start_matches('/'))
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
    value.get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| value.get("message").and_then(Value::as_str).map(ToOwned::to_owned))
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
    out.sort_by(|left, right| left.display_name.cmp(&right.display_name).then_with(|| left.id.cmp(&right.id)));
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
    let json_slice = extract_json_object(raw).ok_or_else(|| "Agent response did not contain JSON.".to_string())?;
    let decision = serde_json::from_str::<LoopDecision>(&json_slice).map_err(|err| err.to_string())?;
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
        other => {
            return Err(format!("Unsupported Enzim Agent action `{other}`."));
        }
    }
    Ok(decision)
}

fn render_history(events: &[EnzimAgentLoopEventRecord]) -> Vec<Value> {
    events
        .iter()
        .map(|event| {
            json!({
                "sequence": event.sequence_no,
                "event_kind": event.event_kind,
                "author_kind": event.author_kind,
                "text": event.compact_text.as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .or(event.full_text.as_deref())
                    .unwrap_or(""),
            })
        })
        .collect()
}

fn call_loop_model(
    config: &EnzimAgentConfig,
    system_prompt: &str,
    loop_record: &EnzimAgentLoopRecord,
    events: &[EnzimAgentLoopEventRecord],
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
            }).to_string() }
        ]
    });

    let client = build_client(config.api_key.as_deref())?;
    let url = endpoint(&base_url, "/chat/completions");
    let response = client.post(url).json(&body).send().map_err(|err| err.to_string())?;
    let status = response.status();
    let value = response.json::<Value>().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(
            parse_error_body(&value).unwrap_or_else(|| format!("Loop model request failed: {status}"))
        );
    }
    let raw = extract_response_text(&value)
        .ok_or_else(|| "Loop model response missing content.".to_string())?;
    parse_decision(&raw)
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
    compact_event_id: Option<i64>,
) -> Result<LoopDriverAction, String> {
    if let Some(event_id) = compact_event_id {
        if let Some(compacted) = decision
            .compacted_last_message
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let json_raw = serde_json::to_string(decision).ok();
            db.update_enzim_agent_loop_event(event_id, None, Some(compacted), json_raw.as_deref())
                .map_err(|err| err.to_string())?;
        }
    }

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
            if recent_agent_followup(&db.list_enzim_agent_loop_events(loop_record.id).map_err(|err| err.to_string())?)
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
                return Err("Enzim Agent generated a duplicate follow-up and paused the loop.".to_string());
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
        _ => Err("Unsupported Enzim Agent action.".to_string()),
    }
}

fn evaluate_loop_from_history(
    db: &AppDb,
    loop_record: &EnzimAgentLoopRecord,
    compact_event_id: Option<i64>,
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
    let decision = call_loop_model(&config, &system_prompt, loop_record, &events)?;
    apply_decision(db, loop_record, &decision, compact_event_id)
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
    if instructions.is_empty() {
        return Err("Looping instructions are empty.".to_string());
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

pub fn active_loop_for_remote_thread(db: &AppDb, remote_thread_id: &str) -> Option<EnzimAgentLoopRecord> {
    let local_thread_id = db
        .get_thread_record_by_remote_thread_id(remote_thread_id)
        .ok()
        .flatten()?
        .id;
    db.active_enzim_agent_loop_for_local_thread(local_thread_id)
        .ok()
        .flatten()
}

pub fn cancel_active_loop_for_remote_thread(db: &AppDb, remote_thread_id: &str) -> Result<(), String> {
    let Some(loop_record) = active_loop_for_remote_thread(db, remote_thread_id) else {
        return Ok(());
    };
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
    db.update_enzim_agent_loop_status(loop_record.id, "cancelled", None, None, Some(now()))
        .map_err(|err| err.to_string())
}

pub fn pending_question(db: &AppDb, remote_thread_id: &str) -> Option<PendingUserQuestion> {
    let loop_record = active_loop_for_remote_thread(db, remote_thread_id)?;
    if loop_record.status != "waiting_user" {
        return None;
    }
    let events = db.list_enzim_agent_loop_events(loop_record.id).ok()?;
    let event = events.into_iter().rev().find(|event| event.event_kind == "agent_question")?;
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
    let event = db
        .append_enzim_agent_loop_event(
            loop_record.id,
            "user_answer",
            source,
            None,
            Some(answer),
            None,
            None,
        )
        .map_err(|err| err.to_string())?;
    evaluate_loop_from_history(db, &loop_record, Some(event.id)).map_err(|err| {
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

fn latest_completed_turn(loop_record: &EnzimAgentLoopRecord, turns: &[LocalChatTurnRecord]) -> Option<LocalChatTurnRecord> {
    turns
        .iter()
        .rev()
        .find(|turn| {
            turn.completed_at.is_some()
                && turn.external_turn_id != loop_record.last_seen_external_turn_id.clone().unwrap_or_default()
                && (!turn.assistant_text.trim().is_empty() || turn.status == "failed")
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
        db.append_enzim_agent_loop_event(
            loop_record.id,
            "runtime_error",
            "runtime",
            Some(&turn.external_turn_id),
            Some("The coding agent turn failed."),
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
            Some("The coding agent turn failed."),
            None,
        )
        .map_err(|err| err.to_string())?;
        return Err("The coding agent turn failed. Enzim Agent paused the loop.".to_string());
    }

    let event = db
        .append_enzim_agent_loop_event(
            loop_record.id,
            "assistant_reply",
            "assistant",
            Some(&turn.external_turn_id),
            Some(&turn.assistant_text),
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

    let action = evaluate_loop_from_history(db, &loop_record, Some(event.id)).map_err(|err| {
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

pub fn mark_followup_dispatched(
    db: &AppDb,
    _loop_id: i64,
    event_id: i64,
    remote_thread_id: &str,
    turn_id: &str,
) -> Result<(), String> {
    db.update_enzim_agent_loop_event(event_id, Some(turn_id), None, None)
        .map_err(|err| err.to_string())?;
    db.mark_enzim_agent_turn_origin(remote_thread_id, turn_id, "enzim_agent")
        .map_err(|err| err.to_string())
}

pub fn record_dispatch_error(db: &AppDb, loop_id: i64, error: &str) -> Result<(), String> {
    let message = error.trim();
    if message.is_empty() {
        return Ok(());
    }
    db.update_enzim_agent_loop_progress(
        loop_id,
        "paused_error",
        None,
        0,
        1,
        Some(message),
        None,
    )
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
