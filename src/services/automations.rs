use crate::data::unix_now;
use crate::services::app::chat::AppDb;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const AUTOMATIONS_KEY: &str = "automations:v1";
const AUTOMATION_RUNS_KEY: &str = "automation_runs:v1";
const MAX_RUN_HISTORY: usize = 80;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutomationDefinition {
    pub id: String,
    pub name: String,
    pub workspace_path: String,
    pub profile_id: i64,
    pub prompt: String,
    pub skill_hints: String,
    pub interval_minutes: i64,
    pub enabled: bool,
    pub access_mode: String,
    pub model_id: Option<String>,
    pub effort: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutomationRunRecord {
    pub id: String,
    pub automation_id: String,
    pub automation_name: String,
    pub workspace_path: String,
    pub local_thread_id: i64,
    pub remote_thread_id: Option<String>,
    pub started_at: i64,
    pub status: String,
    pub summary: String,
    pub error: Option<String>,
}

fn unix_now_millis() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i128
}

fn default_next_run_at(enabled: bool, interval_minutes: i64, now: i64) -> Option<i64> {
    if enabled && interval_minutes > 0 {
        Some(now + interval_minutes.saturating_mul(60))
    } else {
        None
    }
}

pub fn new_automation_id() -> String {
    format!("automation-{}", unix_now_millis())
}

pub fn new_run_id() -> String {
    format!("run-{}", unix_now_millis())
}

pub fn load_automations(db: &AppDb) -> Vec<AutomationDefinition> {
    db.get_setting(AUTOMATIONS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<AutomationDefinition>>(&raw).ok())
        .unwrap_or_default()
}

pub fn save_automations(db: &AppDb, items: &[AutomationDefinition]) -> Result<(), String> {
    let raw = serde_json::to_string(items).map_err(|err| err.to_string())?;
    db.set_setting(AUTOMATIONS_KEY, &raw)
        .map_err(|err| err.to_string())
}

pub fn load_runs(db: &AppDb) -> Vec<AutomationRunRecord> {
    db.get_setting(AUTOMATION_RUNS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<AutomationRunRecord>>(&raw).ok())
        .unwrap_or_default()
}

pub fn save_runs(db: &AppDb, items: &[AutomationRunRecord]) -> Result<(), String> {
    let raw = serde_json::to_string(items).map_err(|err| err.to_string())?;
    db.set_setting(AUTOMATION_RUNS_KEY, &raw)
        .map_err(|err| err.to_string())
}

pub fn upsert_automation(db: &AppDb, automation: AutomationDefinition) -> Result<(), String> {
    let mut items = load_automations(db);
    if let Some(existing) = items.iter_mut().find(|item| item.id == automation.id) {
        *existing = automation;
    } else {
        items.push(automation);
    }
    items.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    save_automations(db, &items)
}

pub fn delete_automation(db: &AppDb, automation_id: &str) -> Result<(), String> {
    let mut items = load_automations(db);
    items.retain(|item| item.id != automation_id);
    save_automations(db, &items)
}

pub fn list_due_automations(db: &AppDb, now: i64) -> Vec<AutomationDefinition> {
    let mut items = load_automations(db)
        .into_iter()
        .filter(|item| item.enabled)
        .filter(|item| item.interval_minutes > 0)
        .filter(|item| item.next_run_at.unwrap_or(0) <= now)
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        a.next_run_at
            .unwrap_or(0)
            .cmp(&b.next_run_at.unwrap_or(0))
            .then_with(|| a.name.cmp(&b.name))
    });
    items
}

pub fn mark_automation_scheduled(
    db: &AppDb,
    automation_id: &str,
    error: Option<String>,
) -> Result<Option<AutomationDefinition>, String> {
    let now = unix_now();
    let mut items = load_automations(db);
    let Some(item) = items.iter_mut().find(|item| item.id == automation_id) else {
        return Ok(None);
    };
    item.updated_at = now;
    item.last_run_at = Some(now);
    item.next_run_at = default_next_run_at(item.enabled, item.interval_minutes, now);
    item.last_error = error;
    let snapshot = item.clone();
    save_automations(db, &items)?;
    Ok(Some(snapshot))
}

pub fn push_run(db: &AppDb, run: AutomationRunRecord) -> Result<(), String> {
    let mut items = load_runs(db);
    items.retain(|item| item.id != run.id);
    items.push(run);
    items.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.automation_name.cmp(&b.automation_name))
            .then_with(|| a.id.cmp(&b.id))
    });
    if items.len() > MAX_RUN_HISTORY {
        items.truncate(MAX_RUN_HISTORY);
    }
    save_runs(db, &items)
}

pub fn update_run_status(
    db: &AppDb,
    run_id: &str,
    status: &str,
    summary: Option<String>,
    error: Option<String>,
    remote_thread_id: Option<String>,
) -> Result<(), String> {
    let mut items = load_runs(db);
    let Some(item) = items.iter_mut().find(|item| item.id == run_id) else {
        return Ok(());
    };
    item.status = status.to_string();
    if let Some(summary) = summary {
        item.summary = summary;
    }
    item.error = error;
    if remote_thread_id.is_some() {
        item.remote_thread_id = remote_thread_id;
    }
    save_runs(db, &items)
}

pub fn build_prompt(prompt: &str, skill_hints: &str) -> String {
    let prompt = prompt.trim();
    let skill_hints = skill_hints.trim();
    if skill_hints.is_empty() {
        prompt.to_string()
    } else {
        format!("{prompt}\n\nPreferred skills or references to use if relevant:\n{skill_hints}")
    }
}

pub fn normalize_definition(mut automation: AutomationDefinition) -> AutomationDefinition {
    let now = unix_now();
    automation.name = automation.name.trim().to_string();
    automation.workspace_path = automation.workspace_path.trim().to_string();
    automation.prompt = automation.prompt.trim().to_string();
    automation.skill_hints = automation.skill_hints.trim().to_string();
    automation.access_mode = automation.access_mode.trim().to_string();
    automation.model_id = automation
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    automation.effort = automation
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    automation.interval_minutes = automation.interval_minutes.max(0);
    automation.updated_at = now;
    if automation.created_at <= 0 {
        automation.created_at = now;
    }
    if automation.id.trim().is_empty() {
        automation.id = new_automation_id();
    }
    if automation.next_run_at.is_none() {
        automation.next_run_at = default_next_run_at(
            automation.enabled,
            automation.interval_minutes,
            automation.last_run_at.unwrap_or(now),
        );
    }
    automation
}
