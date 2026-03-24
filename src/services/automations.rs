use crate::data::unix_now;
use crate::services::app::chat::AppDb;
use crate::services::app::skills::load_catalog;
use chrono::{Datelike, Duration, Local, LocalResult, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const AUTOMATIONS_KEY: &str = "automations:v1";
const AUTOMATION_RUNS_KEY: &str = "automation_runs:v1";
const MAX_RUN_HISTORY: usize = 80;
const DEFAULT_SCHEDULE_MODE: &str = "interval";
const DEFAULT_INTERVAL_UNIT: &str = "hour";

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
    #[serde(default = "default_schedule_mode")]
    pub schedule_mode: String,
    #[serde(default = "default_interval_value")]
    pub interval_value: i64,
    #[serde(default = "default_interval_unit")]
    pub interval_unit: String,
    #[serde(default)]
    pub weekly_days: Vec<String>,
    #[serde(default)]
    pub weekly_times: Vec<String>,
    #[serde(default)]
    pub selected_skill_keys: Vec<String>,
    #[serde(default)]
    pub selected_mcp_keys: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutomationRunEvent {
    pub at: i64,
    pub status: String,
    pub title: String,
    pub detail: String,
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
    #[serde(default)]
    pub timeline: Vec<AutomationRunEvent>,
}

fn default_schedule_mode() -> String {
    DEFAULT_SCHEDULE_MODE.to_string()
}

fn default_interval_value() -> i64 {
    1
}

fn default_interval_unit() -> String {
    DEFAULT_INTERVAL_UNIT.to_string()
}

fn unix_now_millis() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i128
}

fn normalize_schedule_mode(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "manual" => "manual".to_string(),
        "weekly" => "weekly".to_string(),
        _ => DEFAULT_SCHEDULE_MODE.to_string(),
    }
}

fn normalize_interval_unit_value(unit: &str) -> String {
    match unit.trim().to_ascii_lowercase().as_str() {
        "minute" | "minutes" | "min" | "mins" => "minute".to_string(),
        "day" | "days" => "day".to_string(),
        _ => DEFAULT_INTERVAL_UNIT.to_string(),
    }
}

fn interval_minutes_for(value: i64, unit: &str) -> i64 {
    let amount = value.max(0);
    match normalize_interval_unit_value(unit).as_str() {
        "minute" => amount,
        "day" => amount.saturating_mul(1_440),
        _ => amount.saturating_mul(60),
    }
}

fn derive_interval_schedule(interval_minutes: i64) -> (String, i64, String) {
    if interval_minutes <= 0 {
        return ("manual".to_string(), 1, DEFAULT_INTERVAL_UNIT.to_string());
    }
    if interval_minutes % 1_440 == 0 {
        return (
            DEFAULT_SCHEDULE_MODE.to_string(),
            (interval_minutes / 1_440).max(1),
            "day".to_string(),
        );
    }
    if interval_minutes % 60 == 0 {
        return (
            DEFAULT_SCHEDULE_MODE.to_string(),
            (interval_minutes / 60).max(1),
            DEFAULT_INTERVAL_UNIT.to_string(),
        );
    }
    (
        DEFAULT_SCHEDULE_MODE.to_string(),
        interval_minutes.max(1),
        "minute".to_string(),
    )
}

fn normalize_weekday_key(input: &str) -> Option<String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some("mon".to_string()),
        "tue" | "tues" | "tuesday" => Some("tue".to_string()),
        "wed" | "wednesday" => Some("wed".to_string()),
        "thu" | "thur" | "thurs" | "thursday" => Some("thu".to_string()),
        "fri" | "friday" => Some("fri".to_string()),
        "sat" | "saturday" => Some("sat".to_string()),
        "sun" | "sunday" => Some("sun".to_string()),
        _ => None,
    }
}

fn weekday_sort_rank(input: &str) -> usize {
    match input {
        "mon" => 0,
        "tue" => 1,
        "wed" => 2,
        "thu" => 3,
        "fri" => 4,
        "sat" => 5,
        "sun" => 6,
        _ => usize::MAX,
    }
}

fn weekday_from_key(input: &str) -> Option<Weekday> {
    match input {
        "mon" => Some(Weekday::Mon),
        "tue" => Some(Weekday::Tue),
        "wed" => Some(Weekday::Wed),
        "thu" => Some(Weekday::Thu),
        "fri" => Some(Weekday::Fri),
        "sat" => Some(Weekday::Sat),
        "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

fn normalize_time_value(input: &str) -> Option<String> {
    let parsed = NaiveTime::parse_from_str(input.trim(), "%H:%M").ok()?;
    Some(parsed.format("%H:%M").to_string())
}

fn next_weekly_run_at(days: &[String], times: &[String], after: i64) -> Option<i64> {
    let mut selected_days = days
        .iter()
        .filter_map(|value| normalize_weekday_key(value))
        .filter_map(|value| weekday_from_key(&value))
        .collect::<Vec<_>>();
    selected_days.sort_by_key(|weekday| weekday.num_days_from_monday());
    selected_days.dedup();
    if selected_days.is_empty() {
        return None;
    }

    let mut selected_times = times
        .iter()
        .filter_map(|value| normalize_time_value(value))
        .filter_map(|value| NaiveTime::parse_from_str(&value, "%H:%M").ok())
        .collect::<Vec<_>>();
    selected_times.sort();
    selected_times.dedup();
    if selected_times.is_empty() {
        return None;
    }

    let after_local = match Local.timestamp_opt(after, 0) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => Local::now(),
    };
    let start_date = after_local.date_naive();
    for day_offset in 0..14 {
        let date = start_date + Duration::days(day_offset);
        if !selected_days.contains(&date.weekday()) {
            continue;
        }
        for time in &selected_times {
            let naive = date.and_time(*time);
            let candidate = match Local.from_local_datetime(&naive) {
                LocalResult::Single(value) => Some(value.timestamp()),
                LocalResult::Ambiguous(first, second) => [first.timestamp(), second.timestamp()]
                    .into_iter()
                    .filter(|timestamp| *timestamp > after)
                    .min(),
                LocalResult::None => None,
            };
            if let Some(next) = candidate.filter(|timestamp| *timestamp > after) {
                return Some(next);
            }
        }
    }
    None
}

fn compute_next_run_at(automation: &AutomationDefinition, now: i64) -> Option<i64> {
    if !automation.enabled {
        return None;
    }
    match automation.schedule_mode.as_str() {
        "manual" => None,
        "weekly" => next_weekly_run_at(&automation.weekly_days, &automation.weekly_times, now),
        _ => {
            let interval_minutes =
                interval_minutes_for(automation.interval_value, &automation.interval_unit);
            if interval_minutes <= 0 {
                None
            } else {
                Some(now + interval_minutes.saturating_mul(60))
            }
        }
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
        .filter(|item| item.next_run_at.is_some_and(|next| next <= now))
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        a.next_run_at
            .unwrap_or(i64::MAX)
            .cmp(&b.next_run_at.unwrap_or(i64::MAX))
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
    item.next_run_at = compute_next_run_at(item, now);
    item.last_error = error;
    let snapshot = item.clone();
    save_automations(db, &items)?;
    Ok(Some(snapshot))
}

pub fn set_automation_error(
    db: &AppDb,
    automation_id: &str,
    error: Option<String>,
) -> Result<(), String> {
    let mut items = load_automations(db);
    let Some(item) = items.iter_mut().find(|item| item.id == automation_id) else {
        return Ok(());
    };
    item.updated_at = unix_now();
    item.last_error = error;
    save_automations(db, &items)
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
    event_title: &str,
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
    let detail = item
        .error
        .clone()
        .or_else(|| Some(item.summary.clone()))
        .unwrap_or_default();
    item.timeline.push(AutomationRunEvent {
        at: unix_now(),
        status: status.to_string(),
        title: event_title.to_string(),
        detail,
    });
    save_runs(db, &items)
}

pub fn build_prompt(
    db: &AppDb,
    prompt: &str,
    skill_hints: &str,
    selected_skill_keys: &[String],
    selected_mcp_keys: &[String],
) -> String {
    let prompt = prompt.trim().to_string();
    let notes = skill_hints.trim();
    let catalog = load_catalog(db);
    let selected_skills = catalog
        .skills
        .iter()
        .filter(|item| selected_skill_keys.contains(&item.key))
        .map(|item| item.name.clone())
        .collect::<Vec<_>>();
    let selected_mcps = catalog
        .mcps
        .iter()
        .filter(|item| selected_mcp_keys.contains(&item.key))
        .map(|item| item.name.clone())
        .collect::<Vec<_>>();

    let mut sections = Vec::new();
    if !selected_skills.is_empty() {
        sections.push(format!(
            "Preferred skills to use if relevant:\n- {}",
            selected_skills.join("\n- ")
        ));
    }
    if !selected_mcps.is_empty() {
        sections.push(format!(
            "Preferred MCP servers to use if relevant:\n- {}",
            selected_mcps.join("\n- ")
        ));
    }
    if !notes.is_empty() {
        sections.push(format!("Additional tool guidance:\n{notes}"));
    }

    if sections.is_empty() {
        prompt
    } else {
        format!("{prompt}\n\n{}", sections.join("\n\n"))
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
    automation.selected_skill_keys = automation
        .selected_skill_keys
        .into_iter()
        .filter_map(|value| {
            let trimmed = value.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .collect::<Vec<_>>();
    automation.selected_skill_keys.sort();
    automation.selected_skill_keys.dedup();
    automation.selected_mcp_keys = automation
        .selected_mcp_keys
        .into_iter()
        .filter_map(|value| {
            let trimmed = value.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .collect::<Vec<_>>();
    automation.selected_mcp_keys.sort();
    automation.selected_mcp_keys.dedup();

    let legacy_schedule = derive_interval_schedule(automation.interval_minutes);
    if automation.schedule_mode.trim().is_empty() {
        automation.schedule_mode = legacy_schedule.0.clone();
    }
    automation.schedule_mode = normalize_schedule_mode(&automation.schedule_mode);

    if automation.interval_value <= 0 {
        automation.interval_value = legacy_schedule.1;
    }
    automation.interval_value = automation.interval_value.max(1);
    if automation.interval_unit.trim().is_empty() {
        automation.interval_unit = legacy_schedule.2.clone();
    }
    automation.interval_unit = normalize_interval_unit_value(&automation.interval_unit);

    automation.weekly_days = automation
        .weekly_days
        .into_iter()
        .filter_map(|value| normalize_weekday_key(&value))
        .collect::<Vec<_>>();
    automation
        .weekly_days
        .sort_by_key(|value| weekday_sort_rank(value));
    automation.weekly_days.dedup();

    automation.weekly_times = automation
        .weekly_times
        .into_iter()
        .filter_map(|value| normalize_time_value(&value))
        .collect::<Vec<_>>();
    automation.weekly_times.sort();
    automation.weekly_times.dedup();

    automation.interval_minutes = if automation.schedule_mode == "interval" {
        interval_minutes_for(automation.interval_value, &automation.interval_unit)
    } else {
        0
    };
    automation.updated_at = now;
    if automation.created_at <= 0 {
        automation.created_at = now;
    }
    if automation.id.trim().is_empty() {
        automation.id = new_automation_id();
    }
    automation.next_run_at =
        compute_next_run_at(&automation, automation.last_run_at.unwrap_or(now));
    automation
}
