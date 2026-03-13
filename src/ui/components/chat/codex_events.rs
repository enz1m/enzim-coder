use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn extract_agent_message_text(item: &Value) -> Option<String> {
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
            .and_then(|v| v.get("value"))
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
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str)
        {
            out.push_str(text);
        }
    }

    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

pub(super) fn extract_thread_id(params: &Value, fallback: Option<String>) -> Option<String> {
    params
        .get("threadId")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            params
                .get("thread")
                .and_then(|thread| thread.get("threadId"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            params
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("threadId"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("thread"))
                .and_then(|thread| thread.get("threadId"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("thread"))
                .and_then(|thread| thread.get("id"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            params
                .get("item")
                .and_then(|item| item.get("threadId"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or(fallback)
}

pub(super) fn extract_item_id(params: &Value) -> Option<String> {
    params
        .get("itemId")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            params
                .get("item")
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
}

pub(super) fn extract_turn_id(params: &Value) -> Option<String> {
    params
        .get("turnId")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
}

pub(super) fn extract_delta_text(params: &Value) -> Option<String> {
    params
        .get("delta")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            params
                .get("text")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            params
                .get("textDelta")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
}

pub(super) fn sanitize_stream_status_text(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    if text.len() >= 4 && text.starts_with("**") && text.ends_with("**") {
        text = text[2..(text.len() - 2)].trim().to_string();
    }
    text = text.replace("**", "");
    text = text.replace("__", "");
    text = text.replace('`', "");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn item_kind_for_delta_method(method: &str) -> Option<&'static str> {
    match method {
        "item/agentMessage/delta" => Some("agentMessage"),
        "item/plan/delta" => Some("plan"),
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => Some("reasoning"),
        "item/commandExecution/outputDelta" => Some("commandExecution"),
        "item/fileChange/outputDelta" => Some("fileChange"),
        "item/dynamicToolCall/outputDelta" | "item/dynamicToolCall/textDelta" => {
            Some("dynamicToolCall")
        }
        "item/webSearch/outputDelta" | "item/webSearch/textDelta" => Some("webSearch"),
        "item/mcpToolCall/outputDelta" | "item/mcpToolCall/textDelta" => Some("mcpToolCall"),
        "item/collabToolCall/outputDelta" | "item/collabToolCall/textDelta" => {
            Some("collabToolCall")
        }
        "item/imageView/outputDelta" | "item/imageView/textDelta" => Some("imageView"),
        "item/contextCompaction/outputDelta" | "item/contextCompaction/textDelta" => {
            Some("contextCompaction")
        }
        _ => None,
    }
}

fn value_to_compact_string(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    serde_json::to_string(value).unwrap_or_else(|_| "<unavailable>".to_string())
}

pub(super) fn extract_dynamic_tool_call_fields(item: &Value) -> (String, String, String, String) {
    let tool_name = item
        .get("toolName")
        .and_then(Value::as_str)
        .or_else(|| item.get("name").and_then(Value::as_str))
        .or_else(|| {
            item.get("toolCall")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or("tool")
        .to_string();

    let arguments = item
        .get("arguments")
        .map(value_to_compact_string)
        .or_else(|| item.get("input").map(value_to_compact_string))
        .or_else(|| {
            item.get("toolCall")
                .and_then(|v| v.get("arguments"))
                .map(value_to_compact_string)
        })
        .unwrap_or_else(|| "{}".to_string());

    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_string();

    let output = item
        .get("output")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            item.get("result")
                .and_then(|v| v.get("text"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            item.get("content")
                .and_then(Value::as_array)
                .and_then(|parts| {
                    let mut out = String::new();
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            out.push_str(text);
                        }
                    }
                    if out.trim().is_empty() {
                        None
                    } else {
                        Some(out)
                    }
                })
        })
        .unwrap_or_default();

    (tool_name, arguments, status, output)
}

pub(super) fn extract_tool_request_call_fields(params: &Value) -> (String, String, String) {
    let tool_name = params
        .get("toolName")
        .and_then(Value::as_str)
        .or_else(|| params.get("name").and_then(Value::as_str))
        .or_else(|| {
            params
                .get("toolCall")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or("tool")
        .to_string();

    let arguments = params
        .get("arguments")
        .map(value_to_compact_string)
        .or_else(|| params.get("input").map(value_to_compact_string))
        .or_else(|| {
            params
                .get("toolCall")
                .and_then(|v| v.get("arguments"))
                .map(value_to_compact_string)
        })
        .unwrap_or_else(|| "{}".to_string());

    let prompt = params
        .get("prompt")
        .and_then(Value::as_str)
        .or_else(|| params.get("reason").and_then(Value::as_str))
        .unwrap_or("The tool call needs a client response.")
        .to_string();

    (tool_name, arguments, prompt)
}

fn extract_item_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        if !text.trim().is_empty() {
            return Some(text.to_string());
        }
    }
    if let Some(parts) = value.as_array() {
        let mut out = String::new();
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                out.push_str(text);
            } else if let Some(text) = part
                .get("text")
                .and_then(|v| v.get("value"))
                .and_then(Value::as_str)
            {
                out.push_str(text);
            }
        }
        if !out.trim().is_empty() {
            return Some(out);
        }
    }
    Some(value_to_compact_string(value))
}

pub(super) fn extract_generic_item_fields(
    item: &Value,
) -> (String, String, String, String, String) {
    let kind = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("item")
        .to_string();
    let cached_title = item
        .get("title")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let cached_summary = item
        .get("summary")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_string();
    let output = extract_item_text(item.get("output"))
        .or_else(|| extract_item_text(item.get("result")))
        .or_else(|| extract_item_text(item.get("content")))
        .unwrap_or_default();

    match kind.as_str() {
        "webSearch" => {
            let query = item
                .get("query")
                .and_then(Value::as_str)
                .or_else(|| item.get("searchQuery").and_then(Value::as_str))
                .or_else(|| {
                    item.get("input")
                        .and_then(|v| v.get("query"))
                        .and_then(Value::as_str)
                })
                .unwrap_or("Search query unavailable")
                .to_string();
            let mut summary = item
                .get("provider")
                .and_then(Value::as_str)
                .map(|provider| format!("Provider: {provider}"))
                .or(cached_summary.clone())
                .unwrap_or_default();
            let title = cached_title.unwrap_or_else(|| query.clone());
            if summary.trim() == title.trim() {
                summary.clear();
            }
            ("Web Search".to_string(), title, summary, status, output)
        }
        "mcpToolCall" => {
            let name = cached_title.unwrap_or_else(|| {
                item.get("toolName")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("name").and_then(Value::as_str))
                    .unwrap_or("MCP tool")
                    .to_string()
            });
            let summary = cached_summary.unwrap_or_else(|| {
                item.get("arguments")
                    .map(value_to_compact_string)
                    .or_else(|| item.get("input").map(value_to_compact_string))
                    .unwrap_or_else(|| "{}".to_string())
            });
            ("MCP Tool".to_string(), name, summary, status, output)
        }
        "collabToolCall" => {
            let name = cached_title.unwrap_or_else(|| {
                item.get("toolName")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("name").and_then(Value::as_str))
                    .unwrap_or("Collab tool")
                    .to_string()
            });
            let summary = cached_summary.unwrap_or_else(|| {
                item.get("arguments")
                    .map(value_to_compact_string)
                    .or_else(|| item.get("input").map(value_to_compact_string))
                    .unwrap_or_else(|| "{}".to_string())
            });
            ("Collab Tool".to_string(), name, summary, status, output)
        }
        "imageView" => {
            let source = cached_title.unwrap_or_else(|| {
                item.get("url")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("path").and_then(Value::as_str))
                    .unwrap_or("Image viewed")
                    .to_string()
            });
            let summary = cached_summary.unwrap_or_else(|| source.clone());
            ("Image View".to_string(), source, summary, status, output)
        }
        "enteredReviewMode" => (
            "Review Mode".to_string(),
            "Entered review mode".to_string(),
            "Review mode is active for this turn.".to_string(),
            status,
            output,
        ),
        "exitedReviewMode" => (
            "Review Mode".to_string(),
            "Exited review mode".to_string(),
            "Review mode ended.".to_string(),
            status,
            output,
        ),
        "contextCompaction" => {
            let title = if status == "running" {
                "Context compaction".to_string()
            } else if status == "failed" {
                "Failed".to_string()
            } else {
                "Completed".to_string()
            };
            (
                "Context Compaction".to_string(),
                title,
                String::new(),
                status,
                output,
            )
        }
        "fileChangeOutput" => {
            let title = cached_title.unwrap_or_else(|| "File change output".to_string());
            let summary = cached_summary.unwrap_or_else(|| "Tool output details".to_string());
            ("File Change".to_string(), title, summary, status, output)
        }
        _ => (
            "Tool".to_string(),
            kind.clone(),
            value_to_compact_string(item),
            status,
            output,
        ),
    }
}

pub(super) fn format_command_status_label(
    status: &str,
    exit_code: Option<i64>,
    duration_ms: Option<i64>,
) -> String {
    let mut parts = Vec::new();
    parts.push(if status == "failed" {
        "Failed".to_string()
    } else if status == "running" {
        "Running".to_string()
    } else {
        "Completed".to_string()
    });
    if let Some(code) = exit_code {
        parts.push(format!("exit {code}"));
    }
    if let Some(ms) = duration_ms {
        parts.push(format!("{ms}ms"));
    }
    parts.join(" · ")
}

pub(super) fn format_command_actions_markdown(item: &Value) -> Option<String> {
    let actions = item.get("commandActions")?.as_array()?;
    if actions.is_empty() {
        return None;
    }

    let mut out = String::from("Command actions:\n");
    for action in actions {
        let action_type = action
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("action");
        let label = action
            .get("label")
            .and_then(Value::as_str)
            .or_else(|| action.get("title").and_then(Value::as_str))
            .unwrap_or(action_type);
        let details = action
            .get("description")
            .and_then(Value::as_str)
            .or_else(|| action.get("command").and_then(Value::as_str))
            .or_else(|| action.get("path").and_then(Value::as_str))
            .unwrap_or("");

        if details.is_empty() {
            out.push_str(&format!("- `{action_type}`: {label}\n"));
        } else {
            out.push_str(&format!("- `{action_type}`: {label} ({details})\n"));
        }
    }

    Some(out.trim_end().to_string())
}

pub(super) fn turn_failed_message(params: &Value) -> Option<String> {
    let status = params
        .get("turn")
        .and_then(|turn| turn.get("status"))
        .and_then(Value::as_str);
    if status != Some("failed") {
        return None;
    }
    params
        .get("turn")
        .and_then(|turn| turn.get("error"))
        .and_then(|error| format_error_block(error, "Turn failed"))
}

pub(super) fn turn_error_message(params: &Value) -> Option<String> {
    let error = params.get("turn")?.get("error")?;
    format_error_block(error, "Turn failed")
}

pub(super) fn error_event_message(params: &Value) -> Option<String> {
    params
        .get("error")
        .and_then(|error| format_error_block(error, "Error"))
}

fn format_error_block(error: &Value, prefix: &str) -> Option<String> {
    let message = error.get("message").and_then(Value::as_str)?;
    let mut lines = vec![format!("{prefix}: {message}")];

    if let Some(kind) = extract_codex_error_kind(error) {
        if kind == "UsageLimitExceeded" {
            lines.push("You've reached your Codex usage limit.".to_string());
        }
    }

    if let Some(retry_hint) = extract_retry_hint(error) {
        lines.push(retry_hint);
    }

    Some(lines.join("\n"))
}

fn extract_codex_error_kind(error: &Value) -> Option<&str> {
    if let Some(kind) = error.get("codexErrorInfo").and_then(Value::as_str) {
        return Some(kind);
    }
    error
        .get("codexErrorInfo")
        .and_then(|info| info.get("type"))
        .and_then(Value::as_str)
}

fn extract_retry_hint(error: &Value) -> Option<String> {
    if let Some(seconds) = first_i64(
        error,
        &[
            &["retryAfterSeconds"],
            &["additionalDetails", "retryAfterSeconds"],
            &["data", "retryAfterSeconds"],
        ],
    ) {
        if seconds > 0 {
            return Some(format!("Try again in about {}.", format_duration(seconds)));
        }
    }

    if let Some(ms) = first_i64(
        error,
        &[
            &["retryAfterMs"],
            &["additionalDetails", "retryAfterMs"],
            &["data", "retryAfterMs"],
        ],
    ) {
        if ms > 0 {
            let seconds = (ms + 999) / 1000;
            return Some(format!("Try again in about {}.", format_duration(seconds)));
        }
    }

    if let Some(unix_ts) = first_i64(
        error,
        &[
            &["resetsAt"],
            &["resetAt"],
            &["retryAt"],
            &["additionalDetails", "resetsAt"],
            &["additionalDetails", "resetAt"],
            &["additionalDetails", "retryAt"],
            &["data", "resetsAt"],
            &["data", "resetAt"],
            &["data", "retryAt"],
        ],
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if unix_ts > now {
            let seconds = unix_ts - now;
            return Some(format!(
                "Try again in about {} (reset at unix {} ).",
                format_duration(seconds),
                unix_ts
            ));
        }
    }

    first_str(
        error,
        &[
            &["retryAfter"],
            &["additionalDetails", "retryAfter"],
            &["data", "retryAfter"],
        ],
    )
    .map(|value| format!("Try again after {value}."))
}

fn first_i64(root: &Value, paths: &[&[&str]]) -> Option<i64> {
    for path in paths {
        if let Some(value) = get_path(root, path).and_then(Value::as_i64) {
            return Some(value);
        }
    }
    None
}

fn first_str<'a>(root: &'a Value, paths: &[&[&str]]) -> Option<&'a str> {
    for path in paths {
        if let Some(value) = get_path(root, path).and_then(Value::as_str) {
            return Some(value);
        }
    }
    None
}

fn get_path<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = root;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn format_duration(total_seconds: i64) -> String {
    if total_seconds < 60 {
        return format!("{total_seconds}s");
    }
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if seconds == 0 {
        format!("{minutes}m")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

pub(super) fn should_render_for_active(
    resolved_thread_id: Option<&str>,
    active_thread_id: Option<&str>,
) -> bool {
    match (resolved_thread_id, active_thread_id) {
        (Some(a), Some(b)) => a == b,
        (None, Some(_)) => true,
        _ => false,
    }
}

pub(super) fn format_plan_update(params: &Value) -> Option<String> {
    let plan = params.get("plan")?.as_array()?;
    if plan.is_empty() {
        return None;
    }

    let mut out = String::new();
    if let Some(explanation) = params.get("explanation").and_then(Value::as_str) {
        out.push_str(explanation.trim());
        out.push('\n');
    }

    for step in plan {
        let step_text = step.get("step").and_then(Value::as_str).unwrap_or("Step");
        let status = step
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let status_label = match status {
            "completed" => "done",
            "inProgress" => "in progress",
            _ => "pending",
        };
        out.push_str(&format!("- [{status_label}] {step_text}\n"));
    }

    Some(out.trim_end().to_string())
}
