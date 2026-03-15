use crate::codex_appserver::{
    AccountInfo, AppServerNotification, McpServerInfo, ModelInfo, SkillInfo,
};
use crate::backend::{AccountProviderInfo, OAuthFlowInfo};
use crate::data::CodexProfileRecord;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn opencode_command() -> Command {
    Command::new("opencode")
}

fn os_user_home_dir() -> Option<PathBuf> {
    unsafe {
        let uid = libc::geteuid();
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let buf_len = libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX);
        let buf_len = if buf_len <= 0 { 16_384 } else { buf_len as usize };
        let mut buf = vec![0u8; buf_len];
        let status = libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        );
        if status != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let home = CStr::from_ptr(pwd.pw_dir).to_string_lossy().trim().to_string();
        if home.is_empty() {
            None
        } else {
            Some(PathBuf::from(home))
        }
    }
}

fn configure_opencode_env(command: &mut Command, profile: &CodexProfileRecord) {
    let home_dir = os_user_home_dir().unwrap_or_else(|| PathBuf::from(&profile.home_dir));
    let xdg_data_home = home_dir.join(".local").join("share");
    let xdg_config_home = home_dir.join(".config");
    let xdg_cache_home = home_dir.join(".cache");
    let xdg_state_home = home_dir.join(".local").join("state");

    let _ = std::fs::create_dir_all(&home_dir);
    let _ = std::fs::create_dir_all(&xdg_data_home);
    let _ = std::fs::create_dir_all(&xdg_config_home);
    let _ = std::fs::create_dir_all(&xdg_cache_home);
    let _ = std::fs::create_dir_all(&xdg_state_home);

    command
        .env("HOME", &home_dir)
        .env("XDG_DATA_HOME", &xdg_data_home)
        .env("XDG_CONFIG_HOME", &xdg_config_home)
        .env("XDG_CACHE_HOME", &xdg_cache_home)
        .env("XDG_STATE_HOME", &xdg_state_home)
        .env_remove(crate::data::PROFILE_HOME_OVERRIDE_ENV);
}

pub fn opencode_cli_available() -> bool {
    opencode_command()
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn profile_port(profile_id: i64) -> u16 {
    4400u16.saturating_add((profile_id as u16).min(999))
}

fn profile_base_url(profile_id: i64) -> String {
    format!("http://127.0.0.1:{}", profile_port(profile_id))
}

fn map_model_id(value: &str) -> Option<(String, String)> {
    let (provider_id, model_id) = value.split_once(':')?;
    Some((provider_id.to_string(), model_id.to_string()))
}

fn opencode_agent_for_collaboration_mode(mode: Option<&Value>) -> Option<&'static str> {
    match mode
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("default")
    {
        "plan" => Some("plan"),
        "default" | "agent" => Some("build"),
        _ => None,
    }
}

fn opencode_variant_for_effort(effort: Option<&str>) -> Option<String> {
    match effort.map(str::trim).filter(|value| !value.is_empty()) {
        Some("low") | Some("medium") | Some("high") => effort.map(ToOwned::to_owned),
        Some(other) => Some(other.to_string()),
        None => None,
    }
}

fn prompt_body_log_summary(body: &Value) -> String {
    let provider_id = body
        .get("model")
        .and_then(|value| value.get("providerID"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let model_id = body
        .get("model")
        .and_then(|value| value.get("modelID"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let agent = body.get("agent").and_then(Value::as_str).unwrap_or("");
    let variant = body.get("variant").and_then(Value::as_str).unwrap_or("");
    let part_types = body
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts.iter()
                .filter_map(|part| part.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    format!(
        "model={}/{} agent={} variant={} parts=[{}]",
        provider_id, model_id, agent, variant, part_types
    )
}

fn opencode_permissions_for_sandbox_policy(sandbox_policy: Option<&Value>) -> Option<Value> {
    let opencode_policy = sandbox_policy.and_then(|value| value.get("opencode"));
    let access_mode = opencode_policy
        .and_then(|value| value.get("access_mode"))
        .and_then(Value::as_str)
        .or_else(|| {
            sandbox_policy
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
        })?;
    let mut rules = Vec::new();

    match access_mode {
        "dangerFullAccess" => {
            rules.push(json!({
                "permission": "external_directory",
                "pattern": "*",
                "action": "allow"
            }));
        }
        "workspaceWrite" => {
            rules.push(json!({
                "permission": "external_directory",
                "pattern": "*",
                "action": "ask"
            }));
        }
        "readOnly" => {
            rules.push(json!({
                "permission": "external_directory",
                "pattern": "*",
                "action": "ask"
            }));
            rules.push(json!({
                "permission": "edit",
                "pattern": "*",
                "action": "deny"
            }));
            rules.push(json!({
                "permission": "todowrite",
                "pattern": "*",
                "action": "deny"
            }));
        }
        _ => return None,
    }

    rules.push(json!({
        "permission": "bash",
        "pattern": "*",
        "action": "ask"
    }));

    Some(Value::Array(rules))
}

fn opencode_command_mode_from_sandbox_policy(sandbox_policy: Option<&Value>) -> Option<String> {
    sandbox_policy
        .and_then(|value| value.get("opencode"))
        .and_then(|value| value.get("command_mode"))
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

fn format_opencode_error(error: &Value) -> String {
    error
        .get("data")
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .or_else(|| error.get("message").and_then(Value::as_str))
        .unwrap_or("OpenCode request failed")
        .to_string()
}

fn opencode_device_code_from_instructions(instructions: Option<&str>) -> Option<String> {
    instructions
        .map(str::trim)
        .and_then(|value| value.strip_prefix("Enter code:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn push_subscriber_event(
    subscribers: &Arc<Mutex<Vec<mpsc::Sender<AppServerNotification>>>>,
    event: AppServerNotification,
) {
    if let Ok(mut subs) = subscribers.lock() {
        subs.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

fn parse_part_text(part: &Value) -> Option<String> {
    match part.get("type").and_then(Value::as_str) {
        Some("text") | Some("reasoning") => part
            .get("text")
            .and_then(Value::as_str)
            .map(|text| text.to_string()),
        _ => None,
    }
}

fn build_user_message_item(message: &Value, parts: &[Value]) -> Value {
    let content = parts
        .iter()
        .filter_map(|part| match part.get("type").and_then(Value::as_str) {
            Some("text") => part
                .get("text")
                .and_then(Value::as_str)
                .map(|text| json!({"type":"text","text": text})),
            Some("file") => {
                let url = part.get("url").and_then(Value::as_str)?;
                Some(json!({"type":"image","url": url}))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    json!({
        "id": message.get("id").and_then(Value::as_str).unwrap_or("user"),
        "type": "userMessage",
        "content": content,
    })
}

fn compact_json_string(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}

fn normalize_opencode_tool_status(raw_status: Option<&str>, completed: bool) -> &'static str {
    if !completed {
        return "running";
    }
    match raw_status {
        Some("error") | Some("failed") => "failed",
        Some("running") => "running",
        _ => "completed",
    }
}

fn opencode_tool_item_kind(tool_name: &str) -> &'static str {
    match tool_name {
        "bash" => "commandExecution",
        "edit" | "write" | "patch" => "fileChange",
        "read" => "fileRead",
        "glob" | "grep" | "codesearch" => "fileSearch",
        "list" => "directoryList",
        "lsp" => "codeSearch",
        "websearch" => "webSearch",
        "webfetch" => "webFetch",
        "skill" => "skillCall",
        "todoread" | "todowrite" => "todoList",
        "question" => "questionTool",
        _ => "dynamicToolCall",
    }
}

fn opencode_delta_method_for_item_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "agentMessage" => Some("item/agentMessage/delta"),
        "reasoning" => Some("item/reasoning/textDelta"),
        "commandExecution" => Some("item/commandExecution/outputDelta"),
        "fileRead" => Some("item/fileRead/outputDelta"),
        "fileSearch" => Some("item/fileSearch/outputDelta"),
        "directoryList" => Some("item/directoryList/outputDelta"),
        "codeSearch" => Some("item/codeSearch/outputDelta"),
        "webSearch" => Some("item/webSearch/outputDelta"),
        "webFetch" => Some("item/webFetch/outputDelta"),
        "skillCall" => Some("item/skillCall/outputDelta"),
        "todoList" => Some("item/todoList/outputDelta"),
        "questionTool" => Some("item/questionTool/outputDelta"),
        "dynamicToolCall" => Some("item/dynamicToolCall/outputDelta"),
        _ => None,
    }
}

fn tool_duration_ms(state: &Value) -> Option<i64> {
    let start = state
        .get("time")
        .and_then(|time| time.get("start"))
        .and_then(Value::as_i64)?;
    let end = state
        .get("time")
        .and_then(|time| time.get("end"))
        .and_then(Value::as_i64)
        .unwrap_or(start);
    Some(end.saturating_sub(start))
}

fn tool_output_text(state: &Value, completed: bool) -> String {
    if !completed {
        return String::new();
    }
    state
        .get("output")
        .and_then(Value::as_str)
        .or_else(|| {
            state
                .get("metadata")
                .and_then(|metadata| metadata.get("output"))
                .and_then(Value::as_str)
        })
        .or_else(|| state.get("error").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn tool_primary_string(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
    })
}

fn value_display_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(value_display_string)
                .collect::<Vec<_>>()
                .join(" ");
            if joined.trim().is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    }
}

fn nested_string_for_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(value_display_string) {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|nested| nested_string_for_keys(nested, keys))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|nested| nested_string_for_keys(nested, keys)),
        _ => None,
    }
}

fn collect_named_paths(
    value: &Value,
    keys: &[&str],
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if keys.iter().any(|candidate| candidate == &key.as_str()) {
                    if let Some(path) = nested
                        .as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let path = path.to_string();
                        if seen.insert(path.clone()) {
                            out.push(path);
                        }
                    }
                }
                collect_named_paths(nested, keys, out, seen);
            }
        }
        Value::Array(items) => {
            for nested in items {
                collect_named_paths(nested, keys, out, seen);
            }
        }
        _ => {}
    }
}

fn tool_file_paths(input: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    collect_named_paths(
        input,
        &[
            "filepath",
            "filePath",
            "path",
            "oldPath",
            "newPath",
            "sourcePath",
            "destinationPath",
            "targetPath",
        ],
        &mut out,
        &mut seen,
    );
    out
}

fn file_change_item_from_paths(
    item_id: &str,
    status: &str,
    paths: &[String],
    change_kind: &str,
    operation: Option<&str>,
) -> Value {
    let changes = paths
        .iter()
        .map(|path| {
            json!({
                "path": path,
                "kind": change_kind
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": item_id,
        "type": "fileChange",
        "status": status,
        "changes": changes,
        "operation": operation,
    })
}

fn patch_item_from_part(part: &Value, completed: bool) -> Option<Value> {
    let item_id = part_id(part)?;
    let status = if completed { "completed" } else { "running" };
    let paths = tool_file_paths(part);
    if paths.is_empty() {
        return None;
    }
    Some(file_change_item_from_paths(
        &item_id,
        status,
        &paths,
        "updated",
        Some("edit"),
    ))
}

fn opencode_tool_item_from_part(part: &Value, completed: bool) -> Option<Value> {
    let item_id = part_id(part)?;
    let tool_name = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
    let state = part.get("state").cloned().unwrap_or(Value::Null);
    let input = state
        .get("raw")
        .cloned()
        .unwrap_or_else(|| state.get("input").cloned().unwrap_or_else(|| json!({})));
    let status =
        normalize_opencode_tool_status(state.get("status").and_then(Value::as_str), completed);
    let output = tool_output_text(&state, completed);
    let item_kind = opencode_tool_item_kind(tool_name);

    match item_kind {
        "commandExecution" => Some(json!({
            "id": item_id,
            "type": "commandExecution",
            "command": nested_string_for_keys(&input, &["command", "cmd", "argv", "arguments", "script"])
                .or_else(|| nested_string_for_keys(part, &["command", "cmd", "argv", "arguments", "script"]))
                .unwrap_or_else(|| "command".to_string()),
            "status": status,
            "exitCode": state
                .get("metadata")
                .and_then(|metadata| metadata.get("exit"))
                .and_then(Value::as_i64),
            "durationMs": tool_duration_ms(&state),
            "aggregatedOutput": output,
        })),
        "fileChange" => {
            let mut paths = tool_file_paths(&input);
            if paths.is_empty() {
                paths = tool_file_paths(&state);
            }
            if paths.is_empty() {
                paths = tool_file_paths(part);
            }
            if paths.is_empty() {
                if let Some(title) = state
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    paths.push(title.to_string());
                }
            }
            if paths.is_empty() {
                return None;
            }
            Some(file_change_item_from_paths(
                &item_id,
                status,
                &paths,
                if tool_name == "write"
                    && state
                        .get("metadata")
                        .and_then(|metadata| metadata.get("exists"))
                        .and_then(Value::as_bool)
                        == Some(false)
                {
                    "created"
                } else if tool_name == "edit"
                    && input.get("oldString").and_then(Value::as_str) == Some("")
                {
                    "created"
                } else {
                    "updated"
                },
                Some(
                    if tool_name == "write"
                        && state
                            .get("metadata")
                            .and_then(|metadata| metadata.get("exists"))
                            .and_then(Value::as_bool)
                            == Some(false)
                    {
                        "create"
                    } else if tool_name == "write" {
                        "write"
                    } else {
                        "edit"
                    },
                ),
            ))
        }
        "fileRead" => {
            let path = tool_primary_string(&input, &["filePath", "path"]).unwrap_or_else(|| {
                state
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("File")
                    .to_string()
            });
            let title = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path.as_str())
                .to_string();
            Some(json!({
                "id": item_id,
                "type": "fileRead",
                "title": title,
                "summary": path,
                "status": status,
                "output": output,
            }))
        }
        "fileSearch" => {
            let title = tool_primary_string(&input, &["pattern", "query", "term", "text"])
                .unwrap_or_else(|| {
                    state
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Search files")
                        .to_string()
                });
            let summary = tool_primary_string(&input, &["path", "filePath", "directory", "dir"])
                .unwrap_or_else(|| {
                    state
                        .get("metadata")
                        .and_then(|metadata| metadata.get("count"))
                        .and_then(Value::as_i64)
                        .map(|count| format!("{count} match(es)"))
                        .unwrap_or_default()
                });
            Some(json!({
                "id": item_id,
                "type": "fileSearch",
                "title": title,
                "summary": summary,
                "status": status,
                "output": output,
            }))
        }
        "directoryList" => {
            let title = tool_primary_string(&input, &["path", "directory", "dir"])
                .unwrap_or_else(|| ".".to_string());
            let summary = state
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("List directory contents")
                .to_string();
            Some(json!({
                "id": item_id,
                "type": "directoryList",
                "title": title,
                "summary": summary,
                "status": status,
                "output": output,
            }))
        }
        "codeSearch" => {
            let title =
                tool_primary_string(&input, &["symbol", "query", "name", "filePath", "path"])
                    .unwrap_or_else(|| {
                        state
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("Code search")
                            .to_string()
                    });
            Some(json!({
                "id": item_id,
                "type": "codeSearch",
                "title": title,
                "summary": compact_json_string(&input),
                "status": status,
                "output": output,
            }))
        }
        "webSearch" => {
            let query = tool_primary_string(&input, &["query", "searchQuery", "prompt"])
                .unwrap_or_else(|| {
                    state
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Web search")
                        .to_string()
                });
            Some(json!({
                "id": item_id,
                "type": "webSearch",
                "query": query,
                "title": query,
                "summary": "Provider: Exa AI",
                "provider": "Exa AI",
                "status": status,
                "output": output,
            }))
        }
        "webFetch" => {
            let url = tool_primary_string(&input, &["url"]).unwrap_or_else(|| {
                state
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Web fetch")
                    .to_string()
            });
            Some(json!({
                "id": item_id,
                "type": "webFetch",
                "title": url,
                "summary": "Fetched web content",
                "status": status,
                "output": output,
            }))
        }
        "skillCall" => {
            let title =
                tool_primary_string(&input, &["name", "skill", "path"]).unwrap_or_else(|| {
                    state
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Skill")
                        .to_string()
                });
            Some(json!({
                "id": item_id,
                "type": "skillCall",
                "title": title,
                "summary": compact_json_string(&input),
                "status": status,
                "output": output,
            }))
        }
        "todoList" => {
            let title = if tool_name == "todowrite" {
                "Updated todo list".to_string()
            } else {
                "Read todo list".to_string()
            };
            Some(json!({
                "id": item_id,
                "type": "todoList",
                "title": title,
                "summary": compact_json_string(&input),
                "status": status,
                "output": output,
            }))
        }
        "questionTool" => {
            let title = tool_primary_string(&input, &["question", "prompt", "message"])
                .unwrap_or_else(|| "Question".to_string());
            Some(json!({
                "id": item_id,
                "type": "questionTool",
                "title": title,
                "summary": compact_json_string(&input),
                "status": status,
                "output": output,
            }))
        }
        _ => Some(json!({
            "id": item_id,
            "type": "dynamicToolCall",
            "toolName": tool_name,
            "arguments": input,
            "status": status,
            "title": state
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(tool_name),
            "output": output,
        })),
    }
}

fn started_item_from_part(part: &Value) -> Option<Value> {
    let item_id = part_id(part)?;
    match part.get("type").and_then(Value::as_str) {
        Some("text") => Some(json!({
            "id": item_id,
            "type": "agentMessage",
        })),
        Some("reasoning") => Some(json!({
            "id": item_id,
            "type": "reasoning",
        })),
        Some("tool") => opencode_tool_item_from_part(part, false),
        Some("patch") => patch_item_from_part(part, false),
        Some("file") => Some(json!({
            "id": item_id,
            "type": "imageView",
            "title": part.get("filename").and_then(Value::as_str).unwrap_or("file"),
            "status": "running",
        })),
        _ => None,
    }
}

fn build_assistant_items(parts: &[Value]) -> Vec<Value> {
    let mut items = Vec::new();
    for part in parts {
        let Some(item_id) = part.get("id").and_then(Value::as_str) else {
            continue;
        };
        match part.get("type").and_then(Value::as_str) {
            Some("text") => items.push(json!({
                "id": item_id,
                "type": "agentMessage",
                "text": part.get("text").and_then(Value::as_str).unwrap_or("")
            })),
            Some("reasoning") => items.push(json!({
                "id": item_id,
                "type": "reasoning",
                "text": part.get("text").and_then(Value::as_str).unwrap_or(""),
                "startedAt": part.get("time").and_then(|time| time.get("start")).cloned().unwrap_or(Value::Null),
                "completedAt": part.get("time").and_then(|time| time.get("end")).cloned().unwrap_or(Value::Null),
            })),
            Some("tool") => {
                if let Some(item) = opencode_tool_item_from_part(part, true) {
                    items.push(item);
                }
            }
            Some("patch") => {
                if let Some(item) = patch_item_from_part(part, true) {
                    items.push(item);
                }
            }
            Some("file") => items.push(json!({
                "id": item_id,
                "type": "imageView",
                "title": part.get("filename").and_then(Value::as_str).unwrap_or("file"),
                "output": part.get("url").and_then(Value::as_str).unwrap_or(""),
                "status": "completed"
            })),
            _ => {}
        }
    }
    items
}

fn assistant_message_text(parts: &[Value]) -> String {
    let mut sections = Vec::new();
    for part in parts {
        if let Some(text) = parse_part_text(part) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                sections.push(trimmed.to_string());
            }
        }
    }
    sections.join("\n\n")
}

fn part_id(part: &Value) -> Option<String> {
    part.get("id")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

fn message_id(entry: &Value) -> Option<String> {
    entry
        .get("info")
        .and_then(|info| info.get("id"))
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

fn message_is_completed(entry: &Value) -> bool {
    entry
        .get("info")
        .and_then(|info| info.get("time"))
        .and_then(|time| time.get("completed"))
        .is_some_and(|value| !value.is_null())
}

fn message_is_failed(entry: &Value) -> bool {
    entry
        .get("info")
        .and_then(|info| info.get("error"))
        .is_some_and(|value| !value.is_null())
}

fn message_completes_turn(entry: &Value) -> bool {
    if message_is_failed(entry) {
        return true;
    }
    if !message_is_completed(entry) {
        return false;
    }
    entry
        .get("info")
        .and_then(|info| info.get("finish"))
        .and_then(Value::as_str)
        != Some("tool-calls")
}

fn item_kind_for_part(part: &Value) -> Option<&'static str> {
    match part.get("type").and_then(Value::as_str) {
        Some("text") => Some("agentMessage"),
        Some("reasoning") => Some("reasoning"),
        Some("tool") => Some(opencode_tool_item_kind(
            part.get("tool").and_then(Value::as_str).unwrap_or("tool"),
        )),
        Some("patch") => Some("fileChange"),
        Some("file") => Some("imageView"),
        _ => None,
    }
}

fn delta_method_for_part(part: &Value) -> Option<&'static str> {
    item_kind_for_part(part).and_then(opencode_delta_method_for_item_kind)
}

fn part_stream_text(part: &Value) -> String {
    match part.get("type").and_then(Value::as_str) {
        Some("text") | Some("reasoning") => part
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some("tool") => {
            let tool_name = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
            if opencode_tool_item_kind(tool_name) == "fileChange" {
                String::new()
            } else {
                tool_output_text(part.get("state").unwrap_or(&Value::Null), true)
            }
        }
        _ => String::new(),
    }
}

fn completed_item_from_part(part: &Value) -> Option<Value> {
    let item_id = part_id(part)?;
    match part.get("type").and_then(Value::as_str) {
        Some("text") => Some(json!({
            "id": item_id,
            "type": "agentMessage",
            "text": part.get("text").and_then(Value::as_str).unwrap_or(""),
        })),
        Some("reasoning") => Some(json!({
            "id": item_id,
            "type": "reasoning",
            "text": part.get("text").and_then(Value::as_str).unwrap_or(""),
            "startedAt": part.get("time").and_then(|time| time.get("start")).cloned().unwrap_or(Value::Null),
            "completedAt": part.get("time").and_then(|time| time.get("end")).cloned().unwrap_or(Value::Null),
        })),
        Some("tool") => opencode_tool_item_from_part(part, true),
        Some("patch") => patch_item_from_part(part, true),
        Some("file") => Some(json!({
            "id": item_id,
            "type": "imageView",
            "title": part.get("filename").and_then(Value::as_str).unwrap_or("file"),
            "output": part.get("url").and_then(Value::as_str).unwrap_or(""),
            "status": "completed",
        })),
        _ => None,
    }
}

#[derive(Default)]
struct TurnWatchState {
    active_parent_id: Option<String>,
    baseline_messages: HashSet<String>,
    started_turn: bool,
    seen_messages: HashSet<String>,
    started_items: HashSet<String>,
    completed_items: HashSet<String>,
    item_lengths: HashMap<String, usize>,
    item_kinds: HashMap<String, String>,
}

#[derive(Clone, Debug)]
enum OpenCodePendingRequestKind {
    Permission,
    Question,
}

#[derive(Clone, Debug)]
struct OpenCodePendingRequest {
    kind: OpenCodePendingRequestKind,
    remote_id: String,
    session_id: Option<String>,
    directory: Option<String>,
    question_ids: Vec<String>,
}

#[derive(Clone)]
struct OpenCodeMessageRecord {
    id: String,
    role: String,
    parent_id: Option<String>,
    created_at: i64,
    completed_at: Option<Value>,
    error: Option<Value>,
    info: Value,
    parts: Vec<Value>,
}

fn parse_message_record(entry: &Value) -> Option<OpenCodeMessageRecord> {
    let info = entry.get("info")?.clone();
    let id = info.get("id").and_then(Value::as_str)?.to_string();
    let role = info.get("role").and_then(Value::as_str)?.to_string();
    let parent_id = info
        .get("parentID")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let created_at = info
        .get("time")
        .and_then(|value| value.get("created"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let completed_at = info
        .get("time")
        .and_then(|value| value.get("completed"))
        .filter(|value| !value.is_null())
        .cloned();
    let error = info.get("error").filter(|value| !value.is_null()).cloned();
    let parts = entry
        .get("parts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Some(OpenCodeMessageRecord {
        id,
        role,
        parent_id,
        created_at,
        completed_at,
        error,
        info,
        parts,
    })
}

fn grouped_turn_records(
    messages: &[Value],
) -> Vec<(OpenCodeMessageRecord, Vec<OpenCodeMessageRecord>)> {
    let mut users_by_id: HashMap<String, OpenCodeMessageRecord> = HashMap::new();
    let mut assistants_by_parent: HashMap<String, Vec<OpenCodeMessageRecord>> = HashMap::new();

    for entry in messages {
        let Some(record) = parse_message_record(entry) else {
            continue;
        };
        match record.role.as_str() {
            "user" => {
                users_by_id.insert(record.id.clone(), record);
            }
            "assistant" => {
                let Some(parent_id) = record.parent_id.clone() else {
                    continue;
                };
                assistants_by_parent
                    .entry(parent_id)
                    .or_default()
                    .push(record);
            }
            _ => {}
        }
    }

    let mut turns = users_by_id
        .into_iter()
        .filter_map(|(user_id, user)| {
            let mut assistants = assistants_by_parent.remove(&user_id)?;
            assistants.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
            Some((user, assistants))
        })
        .collect::<Vec<_>>();
    turns.sort_by(|(user_a, assistants_a), (user_b, assistants_b)| {
        let key_a = assistants_a
            .first()
            .map(|assistant| assistant.created_at)
            .unwrap_or(user_a.created_at);
        let key_b = assistants_b
            .first()
            .map(|assistant| assistant.created_at)
            .unwrap_or(user_b.created_at);
        key_a
            .cmp(&key_b)
            .then_with(|| user_a.created_at.cmp(&user_b.created_at))
            .then_with(|| user_a.id.cmp(&user_b.id))
    });
    turns
}

fn build_turns_from_messages(messages: &[Value]) -> Vec<Value> {
    let mut turns = Vec::new();
    for (user, assistants) in grouped_turn_records(messages) {
        let mut items = vec![build_user_message_item(&user.info, &user.parts)];
        let mut combined_text = Vec::new();
        for assistant in &assistants {
            items.extend(build_assistant_items(&assistant.parts));
            let text = assistant_message_text(&assistant.parts);
            if !text.trim().is_empty() {
                combined_text.push(text);
            }
        }
        if items.is_empty() {
            let text = combined_text.join("\n\n");
            if !text.trim().is_empty() {
                let turn_id = assistants
                    .last()
                    .map(|assistant| assistant.id.as_str())
                    .unwrap_or(user.id.as_str());
                items.push(json!({
                    "id": turn_id,
                    "type": "agentMessage",
                    "text": text,
                }));
            }
        }
        let turn_id = assistants
            .last()
            .map(|assistant| assistant.id.as_str())
            .unwrap_or(user.id.as_str());
        let created_at = assistants
            .first()
            .map(|assistant| assistant.created_at)
            .unwrap_or(user.created_at);
        let completed_at = assistants
            .last()
            .and_then(|assistant| assistant.completed_at.clone())
            .unwrap_or(Value::Null);
        let failed = assistants
            .last()
            .and_then(|assistant| assistant.error.as_ref())
            .is_some();
        turns.push(json!({
            "id": turn_id,
            "createdAt": created_at,
            "completedAt": completed_at,
            "status": if failed { "failed" } else { "completed" },
            "error": assistants
                .last()
                .and_then(|assistant| assistant.error.clone())
                .unwrap_or(Value::Null),
            "items": items,
        }));
    }

    turns.sort_by_key(|turn| turn.get("createdAt").and_then(Value::as_i64).unwrap_or(0));
    turns
}

pub struct OpenCodeAppServer {
    client: Client,
    base_url: String,
    subscribers: Arc<Mutex<Vec<mpsc::Sender<AppServerNotification>>>>,
    child: Arc<Mutex<Option<std::process::Child>>>,
    profile_id: i64,
    next_turn_id: AtomicI64,
    next_request_id: AtomicI64,
    active_turns: Arc<Mutex<HashMap<String, String>>>,
    session_directories: Arc<Mutex<HashMap<String, String>>>,
    session_command_modes: Arc<Mutex<HashMap<String, String>>>,
    pending_requests: Arc<Mutex<HashMap<i64, OpenCodePendingRequest>>>,
    pending_request_ids_by_remote: Arc<Mutex<HashMap<String, i64>>>,
    log_label: String,
}

impl OpenCodeAppServer {
    pub fn profile_id(&self) -> i64 {
        self.profile_id
    }

    fn auth_file_keys(path: &Path) -> HashSet<String> {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return HashSet::new();
        };
        let Ok(value) = serde_json::from_str::<Value>(&contents) else {
            return HashSet::new();
        };
        value.as_object()
            .map(|items| items.keys().cloned().collect::<HashSet<_>>())
            .unwrap_or_default()
    }

    fn saved_auth_keys(&self) -> HashSet<String> {
        let Ok(paths) = self.get_json("/path", None) else {
            return HashSet::new();
        };
        let Some(home) = paths.get("home").and_then(Value::as_str) else {
            return HashSet::new();
        };
        let mut out = HashSet::new();
        let home_path = PathBuf::from(home);
        let mut candidates = vec![
            home_path.join(".local").join("share").join("opencode").join("auth.json"),
        ];
        if let Some(state) = paths.get("state").and_then(Value::as_str) {
            candidates.push(PathBuf::from(state).join("auth.json"));
        }
        for path in candidates {
            out.extend(Self::auth_file_keys(&path));
        }
        out
    }

    pub fn connect_profile(
        profile: &CodexProfileRecord,
        log_label: &str,
    ) -> Result<Arc<Self>, String> {
        let base_url = profile_base_url(profile.id);
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|err| format!("failed to build OpenCode HTTP client: {err}"))?;

        let mut child = None;
        if !Self::health_check_with_client(&client, &base_url) {
            let home = Path::new(&profile.home_dir);
            let _ = std::fs::create_dir_all(home);
            let mut command = opencode_command();
            configure_opencode_env(&mut command, profile);
            let env_value = |name: &str| {
                use std::ffi::OsStr;
                command
                    .get_envs()
                    .find(|(key, _)| *key == OsStr::new(name))
                    .and_then(|(_, value)| value)
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default()
            };
            eprintln!(
                "[opencode:{}] spawning runtime profile={} port={} env_home={} xdg_data={} xdg_config={} xdg_state={}",
                log_label,
                profile.id,
                profile_port(profile.id),
                env_value("HOME"),
                env_value("XDG_DATA_HOME"),
                env_value("XDG_CONFIG_HOME"),
                env_value("XDG_STATE_HOME"),
            );
            let spawned = command
                .arg("serve")
                .arg("--hostname")
                .arg("127.0.0.1")
                .arg("--port")
                .arg(profile_port(profile.id).to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|err| format!("failed to spawn `opencode serve`: {err}"))?;
            child = Some(spawned);
            let mut healthy = false;
            for _ in 0..60 {
                thread::sleep(Duration::from_millis(250));
                if Self::health_check_with_client(&client, &base_url) {
                    healthy = true;
                    break;
                }
            }
            if !healthy {
                return Err("OpenCode server did not become healthy in time".to_string());
            }
        }

        if let Ok(path_info) = client
            .get(format!("{base_url}/path"))
            .send()
            .and_then(|response| response.json::<Value>())
        {
            eprintln!(
                "[opencode:{}] runtime_ready profile={} home={} config={} state={}",
                log_label,
                profile.id,
                path_info
                    .get("home")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                path_info
                    .get("config")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                path_info
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
        }
        if let Ok(provider_info) = client
            .get(format!("{base_url}/provider"))
            .send()
            .and_then(|response| response.json::<Value>())
        {
            let connected = provider_info
                .get("connected")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            eprintln!(
                "[opencode:{}] runtime_providers profile={} connected=[{}]",
                log_label, profile.id, connected
            );
        }

        Ok(Arc::new(Self {
            client,
            base_url,
            subscribers: Arc::new(Mutex::new(Vec::new())),
            child: Arc::new(Mutex::new(child)),
            profile_id: profile.id,
            next_turn_id: AtomicI64::new(1),
            next_request_id: AtomicI64::new(1),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
            session_directories: Arc::new(Mutex::new(HashMap::new())),
            session_command_modes: Arc::new(Mutex::new(HashMap::new())),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            pending_request_ids_by_remote: Arc::new(Mutex::new(HashMap::new())),
            log_label: log_label.to_string(),
        }))
    }

    fn health_check_with_client(client: &Client, base_url: &str) -> bool {
        client
            .get(format!("{base_url}/global/health"))
            .send()
            .ok()
            .filter(|response| response.status().is_success())
            .is_some()
    }

    fn json_request(&self, request: RequestBuilder) -> Result<Value, String> {
        let response = request.send().map_err(|err| err.to_string())?;
        let status = response.status();
        let body: Value = response.json().map_err(|err| err.to_string())?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(format_opencode_error(&body))
        }
    }

    fn get_json(&self, path: &str, directory: Option<&str>) -> Result<Value, String> {
        let mut request = self.client.get(format!("{}{}", self.base_url, path));
        if let Some(directory) = directory {
            request = request.query(&[("directory", directory)]);
        }
        self.json_request(request)
    }

    fn post_json(&self, path: &str, body: Value, directory: Option<&str>) -> Result<Value, String> {
        let mut request = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        if let Some(directory) = directory {
            request = request.query(&[("directory", directory)]);
        }
        self.json_request(request)
    }

    fn put_json(&self, path: &str, body: Value, directory: Option<&str>) -> Result<Value, String> {
        let mut request = self
            .client
            .put(format!("{}{}", self.base_url, path))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        if let Some(directory) = directory {
            request = request.query(&[("directory", directory)]);
        }
        self.json_request(request)
    }

    fn delete_json(&self, path: &str, directory: Option<&str>) -> Result<Value, String> {
        let mut request = self.client.delete(format!("{}{}", self.base_url, path));
        if let Some(directory) = directory {
            request = request.query(&[("directory", directory)]);
        }
        self.json_request(request)
    }

    fn patch_json(
        &self,
        path: &str,
        body: Value,
        directory: Option<&str>,
    ) -> Result<Value, String> {
        let mut request = self
            .client
            .patch(format!("{}{}", self.base_url, path))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        if let Some(directory) = directory {
            request = request.query(&[("directory", directory)]);
        }
        self.json_request(request)
    }

    fn post_no_content(
        &self,
        path: &str,
        body: Value,
        directory: Option<&str>,
    ) -> Result<(), String> {
        let mut request = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        if let Some(directory) = directory {
            request = request.query(&[("directory", directory)]);
        }
        let response = request.send().map_err(|err| err.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            let body = response.json::<Value>().map_err(|err| err.to_string())?;
            Err(format_opencode_error(&body))
        }
    }

    fn event_stream(
        &self,
        path: &str,
        directory: Option<&str>,
    ) -> Result<reqwest::blocking::Response, String> {
        let client = Client::builder()
            .build()
            .map_err(|err| format!("failed to build OpenCode SSE client: {err}"))?;
        let mut request = client
            .get(format!("{}{}", self.base_url, path))
            .header(ACCEPT, "text/event-stream");
        if let Some(directory) = directory {
            request = request.query(&[("directory", directory)]);
        }
        let response = request.send().map_err(|err| err.to_string())?;
        if response.status().is_success() {
            Ok(response)
        } else {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            Err(format!(
                "OpenCode event stream request failed ({status}): {}",
                body.trim()
            ))
        }
    }

    fn active_turn_id_for_thread(&self, thread_id: &str) -> Option<String> {
        self.active_turns
            .lock()
            .ok()
            .and_then(|map| map.get(thread_id).cloned())
    }

    pub fn active_turn_count(&self) -> usize {
        self.active_turns
            .lock()
            .map(|map| map.len())
            .unwrap_or(0)
    }

    fn remember_session_directory(&self, session_id: &str, directory: Option<&str>) {
        let Some(directory) = directory.filter(|value| !value.is_empty()) else {
            return;
        };
        if let Ok(mut map) = self.session_directories.lock() {
            map.insert(session_id.to_string(), directory.to_string());
        }
    }

    fn session_directory(&self, session_id: &str) -> Option<String> {
        self.session_directories
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).cloned())
    }

    fn remember_session_command_mode(&self, session_id: &str, mode: Option<&str>) {
        let mode = mode.unwrap_or("allowAll").trim();
        if let Ok(mut map) = self.session_command_modes.lock() {
            map.insert(session_id.to_string(), mode.to_string());
        }
    }

    fn session_command_mode(&self, session_id: &str) -> Option<String> {
        self.session_command_modes
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).cloned())
    }

    fn reply_permission_request(
        &self,
        remote_id: &str,
        session_id: Option<&str>,
        directory: Option<&str>,
        reply: &str,
    ) -> Result<(), String> {
        if let Some(session_id) = session_id {
            match self.post_json(
                &format!("/permission/{remote_id}/reply"),
                json!({ "reply": reply }),
                directory,
            ) {
                Ok(_) => Ok(()),
                Err(_) => self
                    .post_json(
                        &format!("/session/{session_id}/permissions/{remote_id}"),
                        json!({ "response": reply }),
                        directory,
                    )
                    .map(|_| ()),
            }
        } else {
            self.post_json(
                &format!("/permission/{remote_id}/reply"),
                json!({ "reply": reply }),
                directory,
            )
            .map(|_| ())
        }
    }

    fn register_pending_request(&self, request: OpenCodePendingRequest) -> i64 {
        let remote_key = match request.kind {
            OpenCodePendingRequestKind::Permission => format!("permission:{}", request.remote_id),
            OpenCodePendingRequestKind::Question => format!("question:{}", request.remote_id),
        };
        if let Some(existing) = self
            .pending_request_ids_by_remote
            .lock()
            .ok()
            .and_then(|map| map.get(&remote_key).copied())
        {
            if let Ok(mut pending) = self.pending_requests.lock() {
                pending.insert(existing, request);
            }
            return existing;
        }
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut by_remote) = self.pending_request_ids_by_remote.lock() {
            by_remote.insert(remote_key, request_id);
        }
        if let Ok(mut pending) = self.pending_requests.lock() {
            pending.insert(request_id, request);
        }
        request_id
    }

    fn resolve_pending_request_id(
        &self,
        kind: OpenCodePendingRequestKind,
        remote_id: &str,
    ) -> Option<i64> {
        let remote_key = match kind {
            OpenCodePendingRequestKind::Permission => format!("permission:{remote_id}"),
            OpenCodePendingRequestKind::Question => format!("question:{remote_id}"),
        };
        self.pending_request_ids_by_remote
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&remote_key))
    }

    fn clear_pending_request(&self, request_id: i64) -> Option<OpenCodePendingRequest> {
        let pending = if let Ok(mut pending) = self.pending_requests.lock() {
            pending.remove(&request_id)
        } else {
            None
        };
        if let Some(pending_request) = pending.as_ref() {
            let remote_key = match pending_request.kind {
                OpenCodePendingRequestKind::Permission => {
                    format!("permission:{}", pending_request.remote_id)
                }
                OpenCodePendingRequestKind::Question => {
                    format!("question:{}", pending_request.remote_id)
                }
            };
            if let Ok(mut by_remote) = self.pending_request_ids_by_remote.lock() {
                by_remote.remove(&remote_key);
            }
        }
        pending
    }

    fn pending_request(&self, request_id: i64) -> Option<OpenCodePendingRequest> {
        self.pending_requests
            .lock()
            .ok()
            .and_then(|pending| pending.get(&request_id).cloned())
    }

    fn mcp_server_name_from_key_path(key_path: &str) -> Option<&str> {
        key_path
            .strip_prefix("mcp_servers.")
            .or_else(|| key_path.strip_prefix("mcp."))
            .filter(|value| !value.trim().is_empty())
    }

    fn opencode_mcp_config_from_generic(server_name: &str, config: &Value) -> Result<Value, String> {
        let Some(config_object) = config.as_object() else {
            return Err(format!(
                "OpenCode MCP config for `{server_name}` must be an object."
            ));
        };

        if config_object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "local" | "remote"))
        {
            return Ok(config.clone());
        }

        let transport = config_object
            .get("transport")
            .and_then(Value::as_str)
            .unwrap_or("");

        let mut out = serde_json::Map::new();
        if let Some(enabled) = config_object.get("enabled").and_then(Value::as_bool) {
            out.insert("enabled".to_string(), Value::Bool(enabled));
        }
        if let Some(timeout) = config_object.get("timeout").cloned() {
            out.insert("timeout".to_string(), timeout);
        }

        let lower_transport = transport.trim().to_ascii_lowercase();
        let is_remote = matches!(
            lower_transport.as_str(),
            "streamable_http" | "streamable-http" | "http" | "https" | "sse" | "remote"
        ) || config_object.get("url").is_some();
        if is_remote {
            let url = config_object
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!("OpenCode remote MCP config for `{server_name}` is missing `url`.")
                })?;
            out.insert("type".to_string(), Value::String("remote".to_string()));
            out.insert("url".to_string(), Value::String(url.to_string()));
            if let Some(headers) = config_object.get("headers").cloned() {
                out.insert("headers".to_string(), headers);
            }
            if let Some(oauth) = config_object.get("oauth").cloned() {
                out.insert("oauth".to_string(), oauth);
            }
            return Ok(Value::Object(out));
        }

        let command_segments = match config_object.get("command") {
            Some(Value::String(command)) => {
                let trimmed = command.trim();
                if trimmed.is_empty() {
                    Vec::new()
                } else {
                    vec![trimmed.to_string()]
                }
            }
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let args = config_object
            .get("args")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut command = command_segments;
        command.extend(args);
        if command.is_empty() {
            return Err(format!(
                "OpenCode local MCP config for `{server_name}` is missing `command`."
            ));
        }

        out.insert("type".to_string(), Value::String("local".to_string()));
        out.insert(
            "command".to_string(),
            Value::Array(command.into_iter().map(Value::String).collect()),
        );
        if let Some(environment) = config_object
            .get("environment")
            .cloned()
            .or_else(|| config_object.get("env").cloned())
        {
            out.insert("environment".to_string(), environment);
        }
        Ok(Value::Object(out))
    }

    fn dispose_global_instances(&self) -> Result<(), String> {
        self.post_no_content("/global/dispose", Value::Null, None)
    }

    fn enable_mcp_server(&self, server_name: &str, config: Value) -> Result<(), String> {
        let mut config_object =
            Self::opencode_mcp_config_from_generic(server_name, &config)?
                .as_object()
                .cloned()
                .ok_or_else(|| {
                    format!("OpenCode MCP config for `{server_name}` must be an object.")
                })?;
        config_object.insert("enabled".to_string(), Value::Bool(true));

        let _ = self.patch_json(
            "/global/config",
            json!({
                "mcp": { server_name: Value::Object(config_object.clone()) },
                "tools": { server_name: true },
            }),
            None,
        )?;

        if let Err(err) = self.post_json(
            "/mcp",
            json!({
                "name": server_name,
                "config": Value::Object(config_object),
            }),
            None,
        ) {
            eprintln!(
                "[opencode:{}] failed to hot-load MCP server `{server_name}`: {err}",
                self.log_label
            );
        }

        Ok(())
    }

    fn disable_mcp_server(&self, server_name: &str) -> Result<(), String> {
        let _ = self.patch_json(
            "/global/config",
            json!({
                "mcp": { server_name: {
                    "enabled": false
                }},
                "tools": { server_name: false },
            }),
            None,
        )?;
        if let Err(err) = self.post_no_content(&format!("/mcp/{server_name}/disconnect"), Value::Null, None) {
            eprintln!(
                "[opencode:{}] failed to disconnect MCP server `{server_name}` after disable: {err}",
                self.log_label
            );
        }
        Ok(())
    }

    fn session_messages(
        &self,
        thread_id: &str,
        directory: Option<&str>,
    ) -> Result<Vec<Value>, String> {
        self.get_json(&format!("/session/{thread_id}/message"), directory)?
            .as_array()
            .cloned()
            .ok_or_else(|| "OpenCode message list response was not an array".to_string())
    }

    fn permission_tool_part(&self, request: &Value) -> Option<Value> {
        let session_id = request.get("sessionID").and_then(Value::as_str)?;
        let tool = request.get("tool")?;
        let message_id = tool.get("messageID").and_then(Value::as_str)?;
        let call_id = tool.get("callID").and_then(Value::as_str);
        let directory = self.session_directory(session_id);
        let message = self
            .get_json(
                &format!("/session/{session_id}/message/{message_id}"),
                directory.as_deref(),
            )
            .ok()?;
        let parts = message.get("parts").and_then(Value::as_array)?;
        let part = call_id
            .and_then(|expected| {
                parts.iter().find(|part| {
                    part.get("id").and_then(Value::as_str) == Some(expected)
                        || part.get("callID").and_then(Value::as_str) == Some(expected)
                })
            })
            .or_else(|| parts.iter().find(|part| part.get("tool").is_some()))?;
        Some(part.clone())
    }

    fn permission_prompt_params(&self, request: &Value) -> (String, Value) {
        let permission = request
            .get("permission")
            .and_then(Value::as_str)
            .unwrap_or("permission");
        let metadata = request.get("metadata").cloned().unwrap_or(Value::Null);
        let tool_part = self.permission_tool_part(request);
        let session_id = request
            .get("sessionID")
            .and_then(Value::as_str)
            .unwrap_or("session");
        let turn_id = self
            .active_turn_id_for_thread(session_id)
            .unwrap_or_else(|| format!("opencode-pending:{session_id}"));

        let mut params = serde_json::Map::new();
        params.insert("turnId".to_string(), json!(turn_id));
        if let Some(item_id) = tool_part.as_ref().and_then(part_id) {
            params.insert("itemId".to_string(), json!(item_id));
        }

        let method = match permission {
            "bash" => {
                let command = nested_string_for_keys(
                    &metadata,
                    &["command", "cmd", "argv", "arguments", "script"],
                )
                .or_else(|| {
                    tool_part.as_ref().and_then(|part| {
                        nested_string_for_keys(part, &["command", "cmd", "argv", "arguments", "script"])
                    })
                })
                .or_else(|| {
                    nested_string_for_keys(
                        request,
                        &["command", "cmd", "argv", "arguments", "script"],
                    )
                })
                .unwrap_or_else(|| "<command unavailable>".to_string());
                let cwd = nested_string_for_keys(&metadata, &["cwd", "directory", "workdir", "worktree"])
                    .or_else(|| {
                        tool_part.as_ref().and_then(|part| {
                            nested_string_for_keys(part, &["cwd", "directory", "workdir", "worktree"])
                        })
                    })
                    .or_else(|| {
                        nested_string_for_keys(request, &["cwd", "directory", "workdir", "worktree"])
                    })
                    .unwrap_or_else(|| "<cwd unavailable>".to_string());
                params.insert(
                    "command".to_string(),
                    json!(command),
                );
                params.insert(
                    "cwd".to_string(),
                    json!(cwd),
                );
                params.insert(
                    "reason".to_string(),
                    json!(format!("OpenCode requested `{permission}` permission.")),
                );
                "item/commandExecution/requestApproval"
            }
            "edit" | "write" | "patch" | "external_directory" => {
                params.insert(
                    "reason".to_string(),
                    json!(format!("OpenCode requested `{permission}` permission.")),
                );
                if let Some(filepath) = nested_string_for_keys(
                    &metadata,
                    &["filepath", "filePath", "path", "directory", "root"],
                )
                .or_else(|| {
                    tool_part.as_ref().and_then(|part| {
                        nested_string_for_keys(
                            part,
                            &["filepath", "filePath", "path", "directory", "root"],
                        )
                    })
                }) {
                    params.insert("grantRoot".to_string(), json!(filepath));
                }
                "item/fileChange/requestApproval"
            }
            _ => {
                params.insert(
                    "prompt".to_string(),
                    json!(format!(
                        "OpenCode requested `{permission}` permission with patterns: {}",
                        request
                            .get("patterns")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .filter(|value| !value.is_empty())
                            .unwrap_or_else(|| "none".to_string())
                    )),
                );
                "item/tool/requestUserInput"
            }
        };

        if !matches!(method, "item/tool/requestUserInput") {
            params.insert(
                "options".to_string(),
                json!(["accept", "acceptForSession", "decline", "cancel"]),
            );
        }

        (method.to_string(), Value::Object(params))
    }

    fn question_prompt_params(&self, request: &Value) -> Value {
        let session_id = request
            .get("sessionID")
            .and_then(Value::as_str)
            .unwrap_or("session");
        let turn_id = self
            .active_turn_id_for_thread(session_id)
            .unwrap_or_else(|| format!("opencode-pending:{session_id}"));

        let mut params = serde_json::Map::new();
        params.insert("turnId".to_string(), json!(turn_id));
        params.insert(
            "prompt".to_string(),
            json!("OpenCode is waiting for additional input."),
        );
        let questions = request
            .get("questions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, question)| {
                        let mut question = question.as_object().cloned().unwrap_or_default();
                        question
                            .entry("id".to_string())
                            .or_insert_with(|| json!(format!("question_{index}")));
                        Value::Object(question)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        params.insert("questions".to_string(), Value::Array(questions));
        params.insert(
            "options".to_string(),
            json!(["accept", "decline", "cancel"]),
        );
        Value::Object(params)
    }

    fn emit_server_request(&self, request_id: i64, method: &str, params: Value) {
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: Some(request_id),
                method: method.to_string(),
                params,
            },
        );
    }

    fn pending_request_entry_for_permission(&self, request: &Value) -> Option<Value> {
        let request_id = request.get("id").and_then(Value::as_str)?;
        let session_id = request
            .get("sessionID")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let synthetic_id = self.register_pending_request(OpenCodePendingRequest {
            kind: OpenCodePendingRequestKind::Permission,
            remote_id: request_id.to_string(),
            directory: session_id
                .as_deref()
                .and_then(|session_id| self.session_directory(session_id)),
            session_id,
            question_ids: Vec::new(),
        });
        let (method, params) = self.permission_prompt_params(request);
        let turn_id = params.get("turnId").and_then(Value::as_str)?.to_string();
        Some(json!({
            "requestId": synthetic_id,
            "turnId": turn_id,
            "method": method,
            "params": params,
        }))
    }

    fn pending_request_entry_for_question(&self, request: &Value) -> Option<Value> {
        let request_id = request.get("id").and_then(Value::as_str)?;
        let session_id = request
            .get("sessionID")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let question_ids = request
            .get("questions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, question)| {
                        question
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| format!("question_{index}"))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let synthetic_id = self.register_pending_request(OpenCodePendingRequest {
            kind: OpenCodePendingRequestKind::Question,
            remote_id: request_id.to_string(),
            directory: session_id
                .as_deref()
                .and_then(|session_id| self.session_directory(session_id)),
            session_id,
            question_ids,
        });
        let params = self.question_prompt_params(request);
        let turn_id = params.get("turnId").and_then(Value::as_str)?.to_string();
        Some(json!({
            "requestId": synthetic_id,
            "turnId": turn_id,
            "method": "item/tool/requestUserInput",
            "params": params,
        }))
    }

    fn emit_server_request_resolved(&self, request_id: i64) {
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "serverRequest/resolved".to_string(),
                params: json!({ "requestId": request_id }),
            },
        );
    }

    fn synthesize_turn_notifications(
        subscribers: &Arc<Mutex<Vec<mpsc::Sender<AppServerNotification>>>>,
        thread_id: &str,
        turn_id: &str,
        assistant: &Value,
        parts: &[Value],
    ) {
        push_subscriber_event(
            subscribers,
            AppServerNotification {
                request_id: None,
                method: "turn/started".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turn": {"id": turn_id, "threadId": thread_id}
                }),
            },
        );

        let text = assistant_message_text(parts);
        let item_id = format!("{turn_id}:message");
        push_subscriber_event(
            subscribers,
            AppServerNotification {
                request_id: None,
                method: "item/started".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "item": {"id": item_id, "type": "agentMessage"}
                }),
            },
        );
        if !text.trim().is_empty() {
            push_subscriber_event(
                subscribers,
                AppServerNotification {
                    request_id: None,
                    method: "item/agentMessage/delta".to_string(),
                    params: json!({
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "itemId": item_id,
                        "delta": text,
                    }),
                },
            );
        }
        push_subscriber_event(
            subscribers,
            AppServerNotification {
                request_id: None,
                method: "item/completed".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "item": {
                        "id": item_id,
                        "type": "agentMessage",
                        "text": text,
                    }
                }),
            },
        );
        push_subscriber_event(
            subscribers,
            AppServerNotification {
                request_id: None,
                method: "turn/completed".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turn": {
                        "id": turn_id,
                        "threadId": thread_id,
                        "status": if assistant.get("error").is_some_and(|value| !value.is_null()) { "failed" } else { "completed" },
                        "completedAt": assistant.get("time").and_then(|value| value.get("completed")).cloned().unwrap_or_else(|| json!(now_secs())),
                        "error": assistant.get("error").cloned().unwrap_or(Value::Null),
                    }
                }),
            },
        );
    }

    fn emit_turn_started(&self, thread_id: &str, turn_id: &str) {
        if let Ok(mut active_turns) = self.active_turns.lock() {
            active_turns.insert(thread_id.to_string(), turn_id.to_string());
        }
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "turn/started".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turn": {"id": turn_id, "threadId": thread_id}
                }),
            },
        );
    }

    fn emit_turn_status(&self, thread_id: &str, turn_id: &str, status: &Value, status_text: &str) {
        let status_text = status_text.trim();
        if status_text.is_empty() {
            return;
        }
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "turn/status".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "statusText": status_text,
                    "status": status,
                }),
            },
        );
    }

    fn emit_turn_error(&self, thread_id: &str, turn_id: &str, error: Value) {
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "error".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "error": error,
                }),
            },
        );
    }

    fn emit_item_started(&self, thread_id: &str, turn_id: &str, item: Value) {
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "item/started".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "item": item
                }),
            },
        );
    }

    fn emit_item_delta(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        method: &str,
        delta: &str,
    ) {
        if delta.trim().is_empty() {
            return;
        }
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: method.to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "itemId": item_id,
                    "delta": delta,
                }),
            },
        );
    }

    fn emit_item_completed(&self, thread_id: &str, turn_id: &str, item: Value) {
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "item/completed".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "item": item,
                }),
            },
        );
    }

    fn emit_turn_completed(&self, thread_id: &str, turn_id: &str, assistant_entry: &Value) {
        if let Ok(mut active_turns) = self.active_turns.lock() {
            if active_turns
                .get(thread_id)
                .is_some_and(|value| value == turn_id)
            {
                active_turns.remove(thread_id);
            }
        }
        let info = assistant_entry.get("info").cloned().unwrap_or(Value::Null);
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "turn/completed".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turn": {
                        "id": turn_id,
                        "threadId": thread_id,
                        "status": if message_is_failed(assistant_entry) { "failed" } else { "completed" },
                        "completedAt": info
                            .get("time")
                            .and_then(|value| value.get("completed"))
                            .cloned()
                            .unwrap_or_else(|| json!(now_secs())),
                        "error": info.get("error").cloned().unwrap_or(Value::Null),
                    }
                }),
            },
        );
        eprintln!(
            "[opencode:{}] turn_completed thread={} turn={} status={}",
            self.log_label,
            thread_id,
            turn_id,
            if message_is_failed(assistant_entry) {
                "failed"
            } else {
                "completed"
            }
        );
    }

    fn process_session_turn_messages(
        &self,
        thread_id: &str,
        turn_id: &str,
        state: &mut TurnWatchState,
        messages: &[Value],
    ) -> bool {
        let mut completed_message: Option<Value> = None;
        for entry in messages.iter().filter(|entry| {
            entry
                .get("info")
                .and_then(|info| info.get("role"))
                .and_then(Value::as_str)
                == Some("assistant")
        }) {
            let Some(message_id) = message_id(entry) else {
                continue;
            };
            let parent_id = entry
                .get("info")
                .and_then(|info| info.get("parentID"))
                .and_then(Value::as_str)
                .map(|value| value.to_string());
            if state.active_parent_id.is_none() {
                let Some(parent_id) = parent_id.clone() else {
                    continue;
                };
                if state.baseline_messages.contains(&message_id) {
                    continue;
                }
                state.active_parent_id = Some(parent_id);
            }
            if parent_id.as_deref() != state.active_parent_id.as_deref() {
                continue;
            }
            let is_new_message = !state.seen_messages.contains(&message_id);
            if !is_new_message && !state.started_turn {
                continue;
            }
            state.seen_messages.insert(message_id.clone());
            if !state.started_turn {
                self.emit_turn_started(thread_id, turn_id);
                state.started_turn = true;
            }

            let parts = entry
                .get("parts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for part in parts {
                let Some(item_id) = part_id(&part) else {
                    continue;
                };
                let Some(item_kind) = item_kind_for_part(&part) else {
                    continue;
                };
                state
                    .item_kinds
                    .insert(item_id.clone(), item_kind.to_string());
                if state.started_items.insert(item_id.clone()) {
                    let item = started_item_from_part(&part).unwrap_or_else(|| {
                        json!({
                            "id": item_id,
                            "type": item_kind,
                        })
                    });
                    self.emit_item_started(thread_id, turn_id, item);
                }

                let current_text = part_stream_text(&part);
                let previous_len = state.item_lengths.get(&item_id).copied().unwrap_or(0);
                if current_text.len() > previous_len {
                    if let Some(method) = delta_method_for_part(&part) {
                        self.emit_item_delta(
                            thread_id,
                            turn_id,
                            &item_id,
                            method,
                            &current_text[previous_len..],
                        );
                    }
                    state
                        .item_lengths
                        .insert(item_id.clone(), current_text.len());
                }

                let should_complete_part = match part.get("type").and_then(Value::as_str) {
                    Some("text") | Some("reasoning") => message_is_completed(entry),
                    Some("tool") => part
                        .get("state")
                        .and_then(|state| state.get("status"))
                        .and_then(Value::as_str)
                        .is_some_and(|status| matches!(status, "completed" | "error" | "failed")),
                    Some("patch") | Some("file") => true,
                    _ => false,
                };
                if should_complete_part && state.completed_items.insert(item_id.clone()) {
                    if let Some(item) = completed_item_from_part(&part) {
                        self.emit_item_completed(thread_id, turn_id, item);
                    }
                }
            }

            if message_completes_turn(entry) {
                completed_message = Some(entry.clone());
            }
        }

        if let Some(completed_message) = completed_message {
            self.emit_turn_completed(thread_id, turn_id, &completed_message);
            true
        } else {
            false
        }
    }

    fn handle_message_updated_event(
        &self,
        thread_id: &str,
        turn_id: &str,
        state: &mut TurnWatchState,
        payload: &Value,
    ) -> Result<bool, String> {
        let info = payload
            .get("properties")
            .and_then(|value| value.get("info"))
            .cloned()
            .unwrap_or(Value::Null);
        if info.get("sessionID").and_then(Value::as_str) != Some(thread_id) {
            return Ok(false);
        }
        if info.get("role").and_then(Value::as_str) != Some("assistant") {
            return Ok(false);
        }
        let message_id = info
            .get("id")
            .and_then(Value::as_str)
            .map(|value| value.to_string());
        let parent_id = info
            .get("parentID")
            .and_then(Value::as_str)
            .map(|value| value.to_string());
        if state.active_parent_id.is_none() {
            let Some(message_id) = message_id.as_deref() else {
                return Ok(false);
            };
            let Some(parent_id) = parent_id.clone() else {
                return Ok(false);
            };
            if state.baseline_messages.contains(message_id) {
                return Ok(false);
            }
            state.active_parent_id = Some(parent_id);
        }
        if parent_id.as_deref() != state.active_parent_id.as_deref() {
            return Ok(false);
        }
        if let Some(message_id) = message_id {
            state.seen_messages.insert(message_id);
        }
        if !state.started_turn {
            self.emit_turn_started(thread_id, turn_id);
            state.started_turn = true;
        }
        let entry = json!({ "info": info, "parts": [] });
        if message_is_completed(&entry) || message_is_failed(&entry) {
            let messages = self.session_messages(thread_id, None)?;
            if self.process_session_turn_messages(thread_id, turn_id, state, &messages) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn handle_message_part_updated_event(
        &self,
        thread_id: &str,
        turn_id: &str,
        state: &mut TurnWatchState,
        payload: &Value,
    ) -> bool {
        let Some(part) = payload
            .get("properties")
            .and_then(|value| value.get("part"))
        else {
            return false;
        };
        if part.get("sessionID").and_then(Value::as_str) != Some(thread_id) {
            return false;
        }
        let Some(item_id) = part_id(part) else {
            return false;
        };
        let Some(item_kind) = item_kind_for_part(part) else {
            return false;
        };

        if !state.started_turn {
            self.emit_turn_started(thread_id, turn_id);
            state.started_turn = true;
        }
        state
            .item_kinds
            .insert(item_id.clone(), item_kind.to_string());
        if state.started_items.insert(item_id.clone()) {
            let item = started_item_from_part(part).unwrap_or_else(|| {
                json!({
                    "id": item_id,
                    "type": item_kind,
                })
            });
            self.emit_item_started(thread_id, turn_id, item);
        }

        let current_text = part_stream_text(part);
        let previous_len = state.item_lengths.get(&item_id).copied().unwrap_or(0);
        if current_text.len() > previous_len {
            if let Some(method) = delta_method_for_part(part) {
                self.emit_item_delta(
                    thread_id,
                    turn_id,
                    &item_id,
                    method,
                    &current_text[previous_len..],
                );
            }
            state
                .item_lengths
                .insert(item_id.clone(), current_text.len());
        }

        let should_complete_part = match part.get("type").and_then(Value::as_str) {
            Some("text") | Some("reasoning") => part
                .get("time")
                .and_then(|time| time.get("end"))
                .is_some_and(|value| !value.is_null()),
            Some("tool") => part
                .get("state")
                .and_then(|state| state.get("status"))
                .and_then(Value::as_str)
                .is_some_and(|status| matches!(status, "completed" | "error" | "failed")),
            Some("patch") | Some("file") => true,
            _ => false,
        };
        if should_complete_part && state.completed_items.insert(item_id.clone()) {
            if let Some(item) = completed_item_from_part(part) {
                self.emit_item_completed(thread_id, turn_id, item);
            }
        }
        false
    }

    fn handle_message_part_delta_event(
        &self,
        thread_id: &str,
        turn_id: &str,
        state: &mut TurnWatchState,
        payload: &Value,
    ) -> bool {
        let props = payload.get("properties").cloned().unwrap_or(Value::Null);
        if props.get("sessionID").and_then(Value::as_str) != Some(thread_id) {
            return false;
        }
        let Some(item_id) = props.get("partID").and_then(Value::as_str) else {
            return false;
        };
        let Some(delta) = props.get("delta").and_then(Value::as_str) else {
            return false;
        };
        if !state.started_turn {
            self.emit_turn_started(thread_id, turn_id);
            state.started_turn = true;
        }
        let method = state
            .item_kinds
            .get(item_id)
            .and_then(|kind| opencode_delta_method_for_item_kind(kind))
            .or_else(|| match props.get("field").and_then(Value::as_str) {
                Some("text") => Some("item/agentMessage/delta"),
                Some("state.output") | Some("output") => Some("item/dynamicToolCall/outputDelta"),
                _ => None,
            });
        if let Some(method) = method {
            if state.started_items.insert(item_id.to_string()) {
                let item_kind = state
                    .item_kinds
                    .get(item_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        if method == "item/reasoning/textDelta" {
                            "reasoning".to_string()
                        } else if method == "item/dynamicToolCall/outputDelta" {
                            "dynamicToolCall".to_string()
                        } else {
                            "agentMessage".to_string()
                        }
                    });
                state
                    .item_kinds
                    .entry(item_id.to_string())
                    .or_insert_with(|| item_kind.clone());
                self.emit_item_started(
                    thread_id,
                    turn_id,
                    json!({
                        "id": item_id,
                        "type": item_kind,
                    }),
                );
            }
            self.emit_item_delta(thread_id, turn_id, item_id, method, delta);
            let total = state.item_lengths.get(item_id).copied().unwrap_or(0) + delta.len();
            state.item_lengths.insert(item_id.to_string(), total);
        }
        false
    }

    fn handle_permission_event(&self, payload: &Value) {
        let request = payload.get("properties").cloned().unwrap_or(Value::Null);
        let Some(request_id) = request.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(session_id) = request.get("sessionID").and_then(Value::as_str) else {
            return;
        };
        let session_directory = self.session_directory(session_id);
        let permission_kind = request
            .get("permission")
            .and_then(Value::as_str)
            .unwrap_or("permission");
        let (method, params) = self.permission_prompt_params(&request);
        let command_preview = params
            .get("command")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let approval_item_id = params
            .get("itemId")
            .and_then(Value::as_str)
            .map(|value| value.to_string());
        eprintln!(
            "[opencode:{}] permission.asked session={} request={} permission={} patterns={} always={} metadata={} tool={}",
            self.log_label,
            session_id,
            request_id,
            permission_kind,
            request
                .get("patterns")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))
                .unwrap_or_default(),
            request
                .get("always")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))
                .unwrap_or_default(),
            compact_json_string(&request.get("metadata").cloned().unwrap_or(Value::Null)),
            compact_json_string(&request.get("tool").cloned().unwrap_or(Value::Null)),
        );
        if permission_kind == "bash"
            && self.session_command_mode(session_id).as_deref() == Some("allowAll")
        {
            eprintln!(
                "[opencode:{}] permission.auto_reply session={} request={} reply=once mode=allowAll cwd={:?}",
                self.log_label, session_id, request_id, session_directory
            );
            match self.reply_permission_request(
                request_id,
                Some(session_id),
                session_directory.as_deref(),
                "once",
            ) {
                Ok(()) => {
                    return;
                }
                Err(err) => {
                    eprintln!(
                        "[opencode:{}] permission.auto_reply failed session={} request={} err={}",
                        self.log_label, session_id, request_id, err
                    );
                }
            }
        }
        let synthetic_id = self.register_pending_request(OpenCodePendingRequest {
            kind: OpenCodePendingRequestKind::Permission,
            remote_id: request_id.to_string(),
            directory: session_directory.clone(),
            session_id: Some(session_id.to_string()),
            question_ids: Vec::new(),
        });
        eprintln!(
            "[opencode:{}] permission.track session={} request={} cwd={:?}",
            self.log_label, session_id, request_id, session_directory
        );
        self.emit_server_request(synthetic_id, &method, params);
        if request.get("permission").and_then(Value::as_str) == Some("bash") {
            if let (Some(turn_id), Some(item_id), Some(command)) = (
                self.active_turn_id_for_thread(session_id),
                approval_item_id.as_deref().or_else(|| {
                        request
                            .get("tool")
                            .and_then(|tool| tool.get("callID"))
                            .and_then(Value::as_str)
                    }),
                command_preview.as_deref(),
            ) {
                self.emit_item_started(
                    session_id,
                    &turn_id,
                    json!({
                        "id": item_id,
                        "type": "commandExecution",
                        "command": command,
                        "status": "pendingApproval",
                    }),
                );
            }
        }
    }

    fn handle_question_event(&self, payload: &Value) {
        let request = payload.get("properties").cloned().unwrap_or(Value::Null);
        let Some(request_id) = request.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(_session_id) = request.get("sessionID").and_then(Value::as_str) else {
            return;
        };
        let session_directory = request
            .get("sessionID")
            .and_then(Value::as_str)
            .and_then(|session_id| self.session_directory(session_id));
        let question_ids = request
            .get("questions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, question)| {
                        question
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| format!("question_{index}"))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let synthetic_id = self.register_pending_request(OpenCodePendingRequest {
            kind: OpenCodePendingRequestKind::Question,
            remote_id: request_id.to_string(),
            directory: session_directory,
            session_id: request
                .get("sessionID")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            question_ids,
        });
        self.emit_server_request(
            synthetic_id,
            "item/tool/requestUserInput",
            self.question_prompt_params(&request),
        );
    }

    fn watch_session_turn_sse(
        &self,
        thread_id: &str,
        turn_id: &str,
        state: &mut TurnWatchState,
        directory: Option<&str>,
        ready_tx: Option<mpsc::Sender<Result<(), String>>>,
    ) -> Result<(), String> {
        let response = match self.event_stream("/event", directory) {
            Ok(response) => {
                if let Some(tx) = ready_tx {
                    let _ = tx.send(Ok(()));
                }
                response
            }
            Err(err) => {
                if let Some(tx) = ready_tx {
                    let _ = tx.send(Err(err.clone()));
                }
                return Err(err);
            }
        };
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        let mut data_lines = Vec::new();

        loop {
            line.clear();
            let read = reader.read_line(&mut line).map_err(|err| err.to_string())?;
            if read == 0 {
                return Err("OpenCode event stream closed".to_string());
            }

            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                let event_payload = data_lines.join("\n");
                data_lines.clear();
                if event_payload.is_empty() {
                    continue;
                }
                let parsed = serde_json::from_str::<Value>(&event_payload);
                let Ok(event) = parsed else {
                    continue;
                };
                let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
                match event_type {
                    "server.connected" => {}
                    "message.part.delta" => {
                        let _ =
                            self.handle_message_part_delta_event(thread_id, turn_id, state, &event);
                    }
                    "message.part.updated" => {
                        let _ = self
                            .handle_message_part_updated_event(thread_id, turn_id, state, &event);
                    }
                    "message.updated" => {
                        if self.handle_message_updated_event(thread_id, turn_id, state, &event)? {
                            return Ok(());
                        }
                    }
                    "session.idle" => {
                        let session_id = event
                            .get("properties")
                            .and_then(|value| value.get("sessionID"))
                            .and_then(Value::as_str);
                        if session_id == Some(thread_id) {
                            let messages = self.session_messages(thread_id, directory)?;
                            if self
                                .process_session_turn_messages(thread_id, turn_id, state, &messages)
                            {
                                return Ok(());
                            }
                        }
                    }
                    "session.status" => {
                        let session_id = event
                            .get("properties")
                            .and_then(|value| value.get("sessionID"))
                            .and_then(Value::as_str);
                        if session_id == Some(thread_id) {
                            let status = event
                                .get("properties")
                                .and_then(|value| value.get("status"))
                                .cloned()
                                .unwrap_or(Value::Null);
                            if let Some(active_turn_id) = self.active_turn_id_for_thread(thread_id) {
                                let status_text = status
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .or_else(|| status.get("type").and_then(Value::as_str))
                                    .unwrap_or("");
                                self.emit_turn_status(
                                    thread_id,
                                    &active_turn_id,
                                    &status,
                                    status_text,
                                );
                            }
                            eprintln!(
                                "[opencode:{}] session.status thread={} status={}",
                                self.log_label,
                                thread_id,
                                compact_json_string(&status)
                            );
                        }
                    }
                    "session.error" => {
                        let session_id = event
                            .get("properties")
                            .and_then(|value| value.get("sessionID"))
                            .and_then(Value::as_str);
                        if session_id == Some(thread_id) || session_id.is_none() {
                            let error = event
                                .get("properties")
                                .and_then(|value| value.get("error"))
                                .cloned()
                                .unwrap_or(Value::Null);
                            if let Some(active_turn_id) = self.active_turn_id_for_thread(thread_id) {
                                self.emit_turn_error(thread_id, &active_turn_id, error.clone());
                            }
                            eprintln!(
                                "[opencode:{}] session.error thread={} error={}",
                                self.log_label,
                                thread_id,
                                compact_json_string(&error)
                            );
                            let message = error
                                .get("data")
                                .and_then(|value| value.get("message"))
                                .and_then(Value::as_str)
                                .or_else(|| error.get("message").and_then(Value::as_str))
                                .unwrap_or("OpenCode session failed.");
                            return Err(message.to_string());
                        }
                    }
                    "permission.asked" => self.handle_permission_event(&event),
                    "permission.replied" => {
                        if let Some(remote_id) = event
                            .get("properties")
                            .and_then(|value| value.get("requestID"))
                            .and_then(Value::as_str)
                        {
                            eprintln!(
                                "[opencode:{}] permission.replied request={} reply={}",
                                self.log_label,
                                remote_id,
                                event
                                    .get("properties")
                                    .and_then(|value| value.get("reply"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown"),
                            );
                        }
                        if let Some(request_id) = event
                            .get("properties")
                            .and_then(|value| value.get("requestID"))
                            .and_then(Value::as_str)
                            .and_then(|remote_id| {
                                self.resolve_pending_request_id(
                                    OpenCodePendingRequestKind::Permission,
                                    remote_id,
                                )
                            })
                        {
                            let _ = self.clear_pending_request(request_id);
                            self.emit_server_request_resolved(request_id);
                        }
                    }
                    "question.asked" => self.handle_question_event(&event),
                    "question.replied" | "question.rejected" => {
                        if let Some(request_id) = event
                            .get("properties")
                            .and_then(|value| value.get("requestID"))
                            .and_then(Value::as_str)
                            .and_then(|remote_id| {
                                self.resolve_pending_request_id(
                                    OpenCodePendingRequestKind::Question,
                                    remote_id,
                                )
                            })
                        {
                            let _ = self.clear_pending_request(request_id);
                            self.emit_server_request_resolved(request_id);
                        }
                    }
                    _ => {
                        let should_refresh = event_payload.contains(thread_id)
                            || event_payload.contains("\"sessionID\"")
                            || event_payload.contains("\"sessionId\"");
                        if should_refresh {
                            let messages = self.session_messages(thread_id, directory)?;
                            if self
                                .process_session_turn_messages(thread_id, turn_id, state, &messages)
                            {
                                return Ok(());
                            }
                        }
                    }
                }
                continue;
            }

            if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start().to_string());
            }
        }
    }

    fn watch_session_turn(
        self: Arc<Self>,
        thread_id: String,
        turn_id: String,
        baseline_message_ids: HashSet<String>,
        directory: Option<String>,
        ready_tx: Option<mpsc::Sender<Result<(), String>>>,
    ) {
        self.remember_session_directory(&thread_id, directory.as_deref());
        eprintln!(
            "[opencode:{}] watch_turn start thread={} turn={} baseline_messages={} cwd={:?}",
            self.log_label,
            thread_id,
            turn_id,
            baseline_message_ids.len(),
            directory,
        );
        let mut state = TurnWatchState {
            baseline_messages: baseline_message_ids.clone(),
            seen_messages: baseline_message_ids,
            ..TurnWatchState::default()
        };

        if let Ok(messages) = self.session_messages(&thread_id, directory.as_deref()) {
            if self.process_session_turn_messages(&thread_id, &turn_id, &mut state, &messages) {
                return;
            }
        }

        let watch_result = self
            .watch_session_turn_sse(
                &thread_id,
                &turn_id,
                &mut state,
                directory.as_deref(),
                ready_tx,
            );

        if let Err(err) = watch_result {
            eprintln!(
                "[opencode:{}] SSE turn watch failed for {}: {}",
                self.log_label, thread_id, err
            );
            self.emit_turn_error(&thread_id, &turn_id, json!({ "message": err.clone() }));
            if let Ok(mut active_turns) = self.active_turns.lock() {
                if active_turns
                    .get(&thread_id)
                    .is_some_and(|value| value == &turn_id)
                {
                    active_turns.remove(&thread_id);
                }
            }
            push_subscriber_event(
                &self.subscribers,
                AppServerNotification {
                    request_id: None,
                    method: "turn/completed".to_string(),
                    params: json!({
                        "threadId": thread_id,
                        "turn": {
                            "id": turn_id,
                            "threadId": thread_id,
                            "status": "failed",
                            "completedAt": now_millis(),
                            "error": { "message": err },
                        }
                    }),
                },
            );
        }
    }

    pub fn subscribe_notifications(&self) -> mpsc::Receiver<AppServerNotification> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.push(tx);
        }
        rx
    }

    pub fn model_list(
        &self,
        _include_hidden: bool,
        limit: usize,
    ) -> Result<Vec<ModelInfo>, String> {
        let provider_response = self.get_json("/provider", None)?;
        let config_response = self.get_json("/config/providers", None).unwrap_or(Value::Null);
        let mut providers = provider_response
            .get("all")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if providers.is_empty() {
            providers = config_response
                .get("providers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        let defaults = config_response
            .get("default")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let preferred_providers = self
            .account_provider_list()
            .ok()
            .unwrap_or_default()
            .into_iter()
            .filter(|provider| provider.connected || provider.has_saved_auth)
            .map(|provider| provider.provider_id)
            .collect::<HashSet<_>>();
        eprintln!(
            "[opencode:{}] model.list providers={} preferred={} source={}",
            self.log_label,
            providers.len(),
            preferred_providers.len(),
            if provider_response
                .get("all")
                .and_then(Value::as_array)
                .is_some()
            {
                "/provider"
            } else {
                "/config/providers"
            }
        );
        providers.sort_by(|left, right| {
            let left_id = left.get("id").and_then(Value::as_str).unwrap_or_default();
            let right_id = right.get("id").and_then(Value::as_str).unwrap_or_default();
            preferred_providers
                .contains(right_id)
                .cmp(&preferred_providers.contains(left_id))
                .then_with(|| left_id.cmp(right_id))
        });
        let mut out = Vec::new();
        for provider in providers {
            let provider_id = provider
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("provider");
            let provider_name = provider
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(provider_id);
            let mut model_entries = Vec::<(String, Value)>::new();
            if let Some(models) = provider.get("models").and_then(Value::as_object) {
                for (model_id, model) in models {
                    model_entries.push((model_id.clone(), model.clone()));
                }
            } else if let Some(models) = provider.get("models").and_then(Value::as_array) {
                for model in models {
                    let model_id = model
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("model")
                        .to_string();
                    model_entries.push((model_id, model.clone()));
                }
            }
            for (model_id_key, model) in model_entries {
                let model_id = model
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(model_id_key.as_str());
                let display_name = model
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(model_id);
                let mut variants = model
                    .get("variants")
                    .and_then(Value::as_object)
                    .map(|variants| variants.keys().cloned().collect::<Vec<_>>())
                    .or_else(|| {
                        model
                            .get("variants")
                            .and_then(Value::as_array)
                            .map(|variants| {
                                variants
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(ToOwned::to_owned)
                                    .collect::<Vec<_>>()
                            })
                    })
                    .unwrap_or_default();
                variants.sort();
                let combined_id = format!("{provider_id}:{model_id}");
                let is_default =
                    defaults.get(provider_id).and_then(Value::as_str) == Some(model_id);
                out.push(ModelInfo {
                    id: combined_id,
                    display_name: format!("{provider_name} / {display_name}"),
                    is_default,
                    variants,
                    default_reasoning_effort: None,
                    reasoning_efforts: Vec::new(),
                });
                if out.len() >= limit.max(1) {
                    break;
                }
            }
            if out.len() >= limit.max(1) {
                break;
            }
        }
        if out.is_empty() {
            out.push(ModelInfo {
                id: "opencode:default".to_string(),
                display_name: "OpenCode default".to_string(),
                is_default: true,
                variants: Vec::new(),
                default_reasoning_effort: None,
                reasoning_efforts: Vec::new(),
            });
        }
        Ok(out)
    }

    pub fn account_read(&self, _refresh_token: bool) -> Result<Option<AccountInfo>, String> {
        let response = self.get_json("/provider", None)?;
        let connected = response
            .get("connected")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let label = if connected.is_empty() {
            format!("OpenCode @ {}", self.base_url)
        } else {
            format!(
                "OpenCode [{}]",
                connected
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Ok(Some(AccountInfo {
            account_type: "opencode".to_string(),
            email: Some(label),
        }))
    }

    pub fn account_login_start_chatgpt(&self) -> Result<(String, String), String> {
        let provider_id = self
            .account_provider_list()?
            .into_iter()
            .find(|provider| provider.supports_oauth)
            .map(|provider| provider.provider_id)
            .ok_or_else(|| {
                "OpenCode provider OAuth flow is unavailable. Configure provider auth in OpenCode first.".to_string()
            })?;
        self.account_login_start_oauth_for_provider(&provider_id)
    }

    pub fn account_provider_list(&self) -> Result<Vec<AccountProviderInfo>, String> {
        let methods = self.get_json("/provider/auth", None)?;
        let methods = methods
            .as_object()
            .ok_or_else(|| "OpenCode provider/auth response was not an object".to_string())?;
        let providers = self.get_json("/provider", None)?;
        let provider_meta = providers
            .get("all")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|provider| {
                        let id = provider.get("id").and_then(Value::as_str)?;
                        let name = provider
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .to_string();
                        let env_keys = provider
                            .get("env")
                            .and_then(Value::as_array)
                            .map(|envs| {
                                envs.iter()
                                    .filter_map(Value::as_str)
                                    .map(|env| env.trim().to_ascii_uppercase())
                                    .filter(|env| !env.is_empty())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let has_api_key_hint = provider
                            .get("options")
                            .and_then(Value::as_object)
                            .map(|options| options.contains_key("apiKey"))
                            .unwrap_or(false)
                            || env_keys.iter().any(|env| {
                                env.contains("API_KEY")
                                    || (env.ends_with("_TOKEN") && !env.contains("ACCESS_TOKEN"))
                            });
                        Some((id.to_string(), (name, has_api_key_hint, env_keys)))
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let connected = providers
            .get("connected")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let saved_auth = self.saved_auth_keys();
        let saved_auth_upper = saved_auth
            .iter()
            .map(|item| item.trim().to_ascii_uppercase())
            .collect::<HashSet<_>>();

        let mut provider_ids = provider_meta.keys().cloned().collect::<HashSet<_>>();
        provider_ids.extend(methods.keys().cloned());
        provider_ids.extend(connected.iter().cloned());

        let mut out = provider_ids
            .into_iter()
            .map(|provider_id| {
                let auth_methods = methods
                    .get(&provider_id)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let supports_oauth = auth_methods.iter().any(|method| {
                    method.get("type").and_then(Value::as_str) == Some("oauth")
                });
                let supports_api_key = auth_methods.iter().any(|method| {
                    matches!(
                        method.get("type").and_then(Value::as_str),
                        Some("api" | "wellknown")
                    )
                }) || provider_meta
                    .get(&provider_id)
                    .map(|(_, has_api_key_hint, _)| *has_api_key_hint)
                    .unwrap_or(false);
                let has_saved_auth = saved_auth.contains(&provider_id)
                    || provider_meta
                        .get(&provider_id)
                        .map(|(_, _, env_keys)| {
                            env_keys.iter().any(|env| saved_auth_upper.contains(env))
                        })
                        .unwrap_or(false);
                AccountProviderInfo {
                    provider_name: provider_meta
                        .get(&provider_id)
                        .map(|(name, _, _)| name.clone())
                        .unwrap_or_else(|| provider_id.clone()),
                    connected: connected.contains(&provider_id),
                    has_saved_auth,
                    provider_id,
                    supports_oauth,
                    supports_api_key,
                }
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| a.provider_name.cmp(&b.provider_name))
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });
        Ok(out)
    }

    pub fn account_login_start_oauth_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<(String, String), String> {
        let info = self.account_login_start_oauth_for_provider_info(provider_id)?;
        Ok((info.provider_id, info.url))
    }

    pub fn account_login_start_oauth_for_provider_info(
        &self,
        provider_id: &str,
    ) -> Result<OAuthFlowInfo, String> {
        let methods = self.get_json("/provider/auth", None)?;
        let object = methods
            .as_object()
            .ok_or_else(|| "OpenCode provider/auth response was not an object".to_string())?;
        let methods = object
            .get(provider_id)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("OpenCode provider `{provider_id}` was not found."))?;
        let (method_idx, _) = methods
            .iter()
            .enumerate()
            .find(|(_, method)| method.get("type").and_then(Value::as_str) == Some("oauth"))
            .ok_or_else(|| {
                format!("OpenCode provider `{provider_id}` does not support OAuth login.")
            })?;
        let auth = self.post_json(
            &format!("/provider/{provider_id}/oauth/authorize"),
            json!({ "method": method_idx }),
            None,
        )?;
        let url = auth
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "OpenCode OAuth response missing URL".to_string())?;
        Ok(OAuthFlowInfo {
            provider_id: provider_id.to_string(),
            url: url.to_string(),
            method: auth
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("auto")
                .to_string(),
            instructions: auth
                .get("instructions")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            device_code: opencode_device_code_from_instructions(
                auth.get("instructions").and_then(Value::as_str),
            ),
            method_index: method_idx as u32,
        })
    }

    pub fn account_complete_oauth_for_provider(
        &self,
        provider_id: &str,
        method_index: u32,
        code: Option<&str>,
    ) -> Result<Option<AccountInfo>, String> {
        let mut body = serde_json::Map::new();
        body.insert("method".to_string(), json!(method_index));
        if let Some(code) = code.map(str::trim).filter(|value| !value.is_empty()) {
            body.insert("code".to_string(), json!(code));
        }
        let _ = self.post_json(
            &format!("/provider/{provider_id}/oauth/callback"),
            Value::Object(body),
            None,
        )?;
        self.account_read(true)
    }

    pub fn account_api_key_provider_options(&self) -> Result<Vec<(String, String)>, String> {
        let methods = self.get_json("/provider/auth", None)?;
        let methods = methods
            .as_object()
            .ok_or_else(|| "OpenCode provider/auth response was not an object".to_string())?;
        let providers = self.get_json("/provider", None)?;
        let provider_names = providers
            .get("all")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|provider| {
                        let id = provider.get("id").and_then(Value::as_str)?;
                        let name = provider
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .to_string();
                        Some((id.to_string(), name))
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let mut out = methods
            .iter()
            .filter_map(|(provider_id, methods)| {
                methods.as_array().and_then(|items| {
                    items
                        .iter()
                        .any(|method| {
                            matches!(
                                method.get("type").and_then(Value::as_str),
                                Some("api" | "wellknown")
                            )
                        })
                        .then(|| {
                            (
                                provider_id.clone(),
                                provider_names
                                    .get(provider_id)
                                    .cloned()
                                    .unwrap_or_else(|| provider_id.clone()),
                            )
                        })
                })
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn account_login_start_api_key(&self, api_key: &str) -> Result<(), String> {
        let options = self.account_api_key_provider_options()?;
        let provider_id = options
            .into_iter()
            .find(|(provider_id, _)| provider_id.eq_ignore_ascii_case("openai"))
            .or_else(|| {
                self.account_api_key_provider_options()
                    .ok()?
                    .into_iter()
                    .next()
            })
            .map(|(provider_id, _)| provider_id)
            .ok_or_else(|| {
                "OpenCode did not report any provider with API-key authentication.".to_string()
            })?;
        self.account_login_start_api_key_for_provider(&provider_id, api_key)
    }

    pub fn account_login_start_api_key_for_provider(
        &self,
        provider_id: &str,
        api_key: &str,
    ) -> Result<(), String> {
        let _ = self.put_json(
            &format!("/auth/{provider_id}"),
            json!({ "type": "api", "key": api_key }),
            None,
        )?;
        Ok(())
    }

    pub fn account_logout(&self) -> Result<(), String> {
        for provider in self.account_provider_list()? {
            if provider.connected || provider.has_saved_auth {
                let _ = self.account_logout_provider(&provider.provider_id);
            }
        }
        Ok(())
    }

    pub fn account_logout_provider(&self, provider_id: &str) -> Result<(), String> {
        let _ = self.delete_json(&format!("/auth/{provider_id}"), None)?;
        Ok(())
    }

    pub fn skills_list(
        &self,
        _cwds: &[String],
        force_reload: bool,
    ) -> Result<Vec<SkillInfo>, String> {
        if force_reload {
            let _ = self.dispose_global_instances();
        }
        let response = self.get_json("/skill", None)?;
        let mut out = response
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| SkillInfo {
                        name: name.to_string(),
                    })
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        eprintln!(
            "[opencode:{}] skills.list count={} force_reload={}",
            self.log_label,
            out.len(),
            force_reload
        );
        Ok(out)
    }

    pub fn mcp_server_status_list(&self, _limit: usize) -> Result<Vec<McpServerInfo>, String> {
        let response = self.get_json("/mcp", None)?;
        let object = response
            .as_object()
            .ok_or_else(|| "OpenCode MCP response was not an object".to_string())?;
        let mut out = Vec::new();
        for (name, status) in object {
            let status_name = status
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            out.push(McpServerInfo {
                name: name.to_string(),
                authenticated: matches!(status_name, "connected"),
                auth_label: status_name.replace('_', " "),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn mcp_server_oauth_login(&self, server_name: &str) -> Result<String, String> {
        let response = self.post_json(&format!("/mcp/{server_name}/auth"), Value::Null, None)?;
        response
            .get("authorizationUrl")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .ok_or_else(|| "OpenCode MCP auth response missing authorizationUrl".to_string())
    }

    pub fn config_mcp_server_reload(&self) -> Result<(), String> {
        self.dispose_global_instances()
    }

    pub fn config_value_write(
        &self,
        key_path: &str,
        value: Value,
        _merge_strategy: &str,
    ) -> Result<(), String> {
        if let Some(server_name) = Self::mcp_server_name_from_key_path(key_path) {
            if value.is_null() {
                return self.disable_mcp_server(server_name);
            }
            return self.enable_mcp_server(server_name, value);
        }

        Err(format!(
            "OpenCode config key `{key_path}` is not supported from Enzim yet."
        ))
    }

    pub fn config_batch_write(&self, edits: Vec<(String, Value, String)>) -> Result<(), String> {
        for (key_path, value, merge_strategy) in edits {
            self.config_value_write(&key_path, value, &merge_strategy)?;
        }
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), String> {
        eprintln!("[opencode:{}] shutting down", self.log_label);
        if let Ok(mut child_guard) = self.child.lock() {
            if let Some(mut child) = child_guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(())
    }

    pub fn thread_set_command_mode(&self, thread_id: &str, command_mode: &str) -> Result<(), String> {
        self.remember_session_command_mode(thread_id, Some(command_mode));
        eprintln!(
            "[opencode:{}] thread.command_mode thread={} mode={}",
            self.log_label, thread_id, command_mode
        );
        Ok(())
    }

    pub fn thread_start(
        &self,
        cwd: Option<&str>,
        model: Option<&str>,
        sandbox_policy: Option<Value>,
    ) -> Result<String, String> {
        let mut body = serde_json::Map::new();
        if let Some(permission) = opencode_permissions_for_sandbox_policy(sandbox_policy.as_ref()) {
            eprintln!(
                "[opencode:{}] thread_start permissions cwd={:?} rules={}",
                self.log_label,
                cwd,
                compact_json_string(&permission),
            );
            body.insert("permission".to_string(), permission);
        }
        let body = Value::Object(body);
        let session = self.post_json("/session", body, cwd)?;
        let thread_id = session
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "OpenCode session/create response missing id".to_string())?
            .to_string();
        self.remember_session_directory(
            &thread_id,
            session
                .get("directory")
                .and_then(Value::as_str)
                .or(cwd),
        );
        self.remember_session_command_mode(
            &thread_id,
            opencode_command_mode_from_sandbox_policy(sandbox_policy.as_ref()).as_deref(),
        );
        if let Some(model) = model {
            let _ = self.thread_resume(&thread_id, cwd, Some(model));
        }
        Ok(thread_id)
    }

    pub fn thread_resume(
        &self,
        thread_id: &str,
        cwd: Option<&str>,
        _model: Option<&str>,
    ) -> Result<String, String> {
        let session = self.get_json(&format!("/session/{thread_id}"), cwd)?;
        self.remember_session_directory(
            thread_id,
            session
                .get("directory")
                .and_then(Value::as_str)
                .or(cwd),
        );
        session
            .get("id")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .ok_or_else(|| "OpenCode session/get response missing id".to_string())
    }

    pub fn thread_read(&self, thread_id: &str, include_turns: bool) -> Result<Value, String> {
        let directory = self.session_directory(thread_id);
        let session = self.get_json(&format!("/session/{thread_id}"), directory.as_deref())?;
        let turns = if include_turns {
            let messages = self.session_messages(thread_id, directory.as_deref())?;
            build_turns_from_messages(&messages)
        } else {
            Vec::new()
        };
        Ok(json!({
            "id": session.get("id").and_then(Value::as_str).unwrap_or(thread_id),
            "threadId": session.get("id").and_then(Value::as_str).unwrap_or(thread_id),
            "title": session.get("title").cloned().unwrap_or_else(|| json!("OpenCode session")),
            "turns": turns,
        }))
    }

    pub fn thread_fork(&self, thread_id: &str) -> Result<String, String> {
        let directory = self.session_directory(thread_id);
        let session = self.post_json(
            &format!("/session/{thread_id}/fork"),
            json!({}),
            directory.as_deref(),
        )?;
        self.remember_session_directory(
            session.get("id").and_then(Value::as_str).unwrap_or_default(),
            session
                .get("directory")
                .and_then(Value::as_str)
                .or(directory.as_deref()),
        );
        session
            .get("id")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .ok_or_else(|| "OpenCode session/fork response missing id".to_string())
    }

    pub fn thread_rollback(&self, thread_id: &str, count: usize) -> Result<Value, String> {
        if count == 0 {
            return self.thread_read(thread_id, true);
        }
        let directory = self.session_directory(thread_id);
        let messages = self.session_messages(thread_id, directory.as_deref())?;
        let turn_bounds = grouped_turn_records(&messages)
            .into_iter()
            .filter_map(|(_, assistants)| {
                let first_id = assistants.first().map(|assistant| assistant.id.clone())?;
                let last_id = assistants.last().map(|assistant| assistant.id.clone())?;
                Some((first_id, last_id))
            })
            .collect::<Vec<_>>();
        if turn_bounds.len() < count {
            return Err("OpenCode rollback target was not found".to_string());
        }
        let Some((target_message_id, target_turn_id)) = turn_bounds
            .get(turn_bounds.len() - count)
            .cloned()
        else {
            return Err("OpenCode rollback target was not found".to_string());
        };
        eprintln!(
            "[opencode:{}] thread.rollback thread={} count={} target_turn_id={} target_message_id={}",
            self.log_label, thread_id, count, target_turn_id, target_message_id
        );
        let _ = self.post_json(
            &format!("/session/{thread_id}/revert"),
            json!({ "messageID": target_message_id }),
            directory.as_deref(),
        )?;
        self.thread_read(thread_id, true)
    }

    pub fn thread_unrollback(&self, thread_id: &str) -> Result<Value, String> {
        let directory = self.session_directory(thread_id);
        let _ = self.post_json(
            &format!("/session/{thread_id}/unrevert"),
            json!({}),
            directory.as_deref(),
        )?;
        self.thread_read(thread_id, true)
    }

    pub fn thread_native_restore_info(
        &self,
        thread_id: &str,
        target_turn_id: &str,
    ) -> Result<Value, String> {
        let directory = self.session_directory(thread_id);
        let messages = self.session_messages(thread_id, directory.as_deref())?;
        let turns = grouped_turn_records(&messages);
        let Some(target_index) = turns.iter().position(|(_, assistants)| {
            assistants
                .last()
                .map(|assistant| assistant.id.as_str())
                == Some(target_turn_id)
        }) else {
            return Err("OpenCode restore target was not found".to_string());
        };

        let mut patch_files = HashSet::new();
        let mut tool_file_change_count = 0usize;
        for (_, assistants) in turns.into_iter().skip(target_index) {
            for assistant in assistants {
                for part in assistant.parts {
                    match part.get("type").and_then(Value::as_str) {
                        Some("patch") => {
                            if let Some(files) = part.get("files").and_then(Value::as_array) {
                                for file in files.iter().filter_map(Value::as_str) {
                                    patch_files.insert(file.to_string());
                                }
                            }
                        }
                        Some("tool")
                            if opencode_tool_item_kind(
                                part.get("tool").and_then(Value::as_str).unwrap_or("tool"),
                            ) == "fileChange" =>
                        {
                            tool_file_change_count += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut patch_files = patch_files.into_iter().collect::<Vec<_>>();
        patch_files.sort();

        Ok(json!({
            "hasNativeFileRestore": !patch_files.is_empty(),
            "patchFileCount": patch_files.len(),
            "patchFiles": patch_files,
            "toolFileChangeCount": tool_file_change_count,
        }))
    }

    pub fn thread_archive(&self, thread_id: &str) -> Result<(), String> {
        let directory = self.session_directory(thread_id);
        let _ = self.delete_json(&format!("/session/{thread_id}"), directory.as_deref())?;
        Ok(())
    }

    pub fn pending_server_requests_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Vec<Value>, String> {
        let directory = self.session_directory(thread_id);
        let permissions = self.get_json("/permission", directory.as_deref())?;
        let questions = self.get_json("/question", directory.as_deref())?;
        let mut entries = permissions
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.get("sessionID").and_then(Value::as_str) == Some(thread_id))
            .filter_map(|request| self.pending_request_entry_for_permission(&request))
            .collect::<Vec<_>>();
        entries.extend(
            questions
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|request| {
                    request.get("sessionID").and_then(Value::as_str) == Some(thread_id)
                })
                .filter_map(|request| self.pending_request_entry_for_question(&request)),
        );
        entries.sort_by_key(|entry| entry.get("requestId").and_then(Value::as_i64).unwrap_or(0));
        Ok(entries)
    }

    fn build_prompt_text(
        &self,
        text: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
    ) -> String {
        let mut full = text.trim().to_string();
        if !mentions.is_empty() {
            let mention_lines = mentions
                .iter()
                .map(|(name, path)| format!("@{name}: {path}"))
                .collect::<Vec<_>>()
                .join("\n");
            if !mention_lines.is_empty() {
                if !full.is_empty() {
                    full.push_str("\n\n");
                }
                full.push_str("Attached mentions:\n");
                full.push_str(&mention_lines);
            }
        }
        if !local_image_paths.is_empty() {
            let image_lines = local_image_paths.join("\n");
            if !image_lines.is_empty() {
                if !full.is_empty() {
                    full.push_str("\n\n");
                }
                full.push_str("Attached local images:\n");
                full.push_str(&image_lines);
            }
        }
        full
    }

    fn build_prompt_body(
        &self,
        text: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
        model: Option<&str>,
        effort: Option<&str>,
        collaboration_mode: Option<&Value>,
    ) -> Value {
        let prompt_text = self.build_prompt_text(text, local_image_paths, mentions);
        let parts = vec![json!({ "type": "text", "text": prompt_text })];
        let model_payload = model.and_then(map_model_id).map(
            |(provider_id, model_id)| json!({ "providerID": provider_id, "modelID": model_id }),
        );
        let mut body = serde_json::Map::new();
        body.insert("parts".to_string(), Value::Array(parts));
        body.insert("model".to_string(), model_payload.unwrap_or(Value::Null));
        if let Some(agent) = opencode_agent_for_collaboration_mode(collaboration_mode) {
            body.insert("agent".to_string(), json!(agent));
        }
        if let Some(variant) = opencode_variant_for_effort(effort) {
            body.insert("variant".to_string(), json!(variant));
        }
        Value::Object(body)
    }

    fn dispatch_prompt(
        self: &Arc<Self>,
        thread_id: &str,
        turn_id: String,
        body: Value,
        cwd: Option<&str>,
    ) {
        let baseline_message_ids = self
            .session_messages(thread_id, cwd)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| message_id(&entry))
            .collect::<HashSet<_>>();
        let body_summary = prompt_body_log_summary(&body);

        let client = Arc::clone(self);
        let thread_id_owned = thread_id.to_string();
        let turn_id_owned = turn_id;
        let cwd_owned = cwd.map(|value| value.to_string());
        thread::spawn(move || {
            let (watch_ready_tx, watch_ready_rx) = mpsc::channel::<Result<(), String>>();
            let watcher = Arc::clone(&client);
            let watch_thread_id = thread_id_owned.clone();
            let watch_turn_id = turn_id_owned.clone();
            let watch_baseline_message_ids = baseline_message_ids.clone();
            let watch_cwd = cwd_owned.clone();
            thread::spawn(move || {
                watcher.watch_session_turn(
                    watch_thread_id,
                    watch_turn_id,
                    watch_baseline_message_ids,
                    watch_cwd,
                    Some(watch_ready_tx),
                );
            });

            match watch_ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => {
                    eprintln!(
                        "[opencode:{}] watch_turn connected thread={} turn={}",
                        client.log_label, thread_id_owned, turn_id_owned
                    );
                }
                Ok(Err(err)) => {
                    eprintln!(
                        "[opencode:{}] watch_turn connect failed thread={} turn={} error={}",
                        client.log_label, thread_id_owned, turn_id_owned, err
                    );
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!(
                        "[opencode:{}] watch_turn connect timed out thread={} turn={}, continuing",
                        client.log_label, thread_id_owned, turn_id_owned
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    eprintln!(
                        "[opencode:{}] watch_turn connect channel closed thread={} turn={}, continuing",
                        client.log_label, thread_id_owned, turn_id_owned
                    );
                }
            }

            let async_result = client.post_no_content(
                &format!("/session/{thread_id_owned}/prompt_async"),
                body.clone(),
                cwd_owned.as_deref(),
            );
            match async_result {
                Ok(()) => {
                    eprintln!(
                        "[opencode:{}] prompt_async accepted thread={} turn={} {}",
                        client.log_label, thread_id_owned, turn_id_owned, body_summary
                    );
                }
                Err(async_err) => {
                    eprintln!(
                        "[opencode:{}] prompt_async failed thread={} turn={} error={} {}",
                        client.log_label, thread_id_owned, turn_id_owned, async_err, body_summary
                    );
                    let response = client.post_no_content(
                        &format!("/session/{thread_id_owned}/message"),
                        body,
                        cwd_owned.as_deref(),
                    );
                    match response {
                        Ok(()) => {
                            eprintln!(
                                "[opencode:{}] message fallback accepted thread={} turn={} {}",
                                client.log_label, thread_id_owned, turn_id_owned, body_summary
                            );
                        }
                        Err(err) => {
                            let combined = if async_err == err {
                                err
                            } else {
                                format!("{async_err}; fallback send failed: {err}")
                            };
                            client.emit_turn_error(
                                &thread_id_owned,
                                &turn_id_owned,
                                json!({ "message": combined.clone() }),
                            );
                            push_subscriber_event(
                                &client.subscribers,
                                AppServerNotification {
                                    request_id: None,
                                    method: "turn/completed".to_string(),
                                    params: json!({
                                        "threadId": thread_id_owned,
                                        "turn": {
                                            "id": turn_id_owned,
                                            "threadId": thread_id_owned,
                                            "status": "failed",
                                            "completedAt": now_millis(),
                                            "error": { "message": combined },
                                        }
                                    }),
                                },
                            );
                        }
                    }
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn turn_start(
        self: &Arc<Self>,
        thread_id: &str,
        text: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
        model: Option<&str>,
        effort: Option<&str>,
        _sandbox_policy: Option<Value>,
        _approval_policy: Option<&str>,
        collaboration_mode: Option<Value>,
        cwd: Option<&str>,
    ) -> Result<String, String> {
        let turn_id = format!(
            "opencode-turn-{}-{}",
            self.profile_id,
            self.next_turn_id.fetch_add(1, Ordering::Relaxed)
        );
        eprintln!(
            "[opencode:{}] turn_start thread={} turn={} model={:?} effort={:?} mode={} text_len={} cwd={:?}",
            self.log_label,
            thread_id,
            turn_id,
            model,
            effort,
            collaboration_mode
                .as_ref()
                .and_then(|value| value.get("mode"))
                .and_then(Value::as_str)
                .unwrap_or("default"),
            text.len(),
            cwd,
        );
        let body = self.build_prompt_body(
            text,
            local_image_paths,
            mentions,
            model,
            effort,
            collaboration_mode.as_ref(),
        );
        self.dispatch_prompt(thread_id, turn_id.clone(), body, cwd);

        Ok(turn_id)
    }

    pub fn turn_interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), String> {
        let directory = self.session_directory(thread_id);
        eprintln!(
            "[opencode:{}] turn_interrupt thread={} turn={} cwd={:?}",
            self.log_label, thread_id, turn_id, directory
        );
        let _ = self.post_json(
            &format!("/session/{thread_id}/abort"),
            Value::Null,
            directory.as_deref(),
        )?;
        if let Ok(mut active_turns) = self.active_turns.lock() {
            if active_turns
                .get(thread_id)
                .is_some_and(|active_turn_id| active_turn_id == turn_id)
            {
                active_turns.remove(thread_id);
            }
        }
        push_subscriber_event(
            &self.subscribers,
            AppServerNotification {
                request_id: None,
                method: "turn/completed".to_string(),
                params: json!({
                    "threadId": thread_id,
                    "turn": {
                        "id": turn_id,
                        "threadId": thread_id,
                        "status": "cancelled",
                        "completedAt": now_millis(),
                        "error": Value::Null,
                    }
                }),
            },
        );
        Ok(())
    }

    pub fn turn_steer(
        self: &Arc<Self>,
        thread_id: &str,
        turn_id: &str,
        prompt: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
    ) -> Result<String, String> {
        let trimmed = prompt.trim();
        if trimmed.is_empty() && local_image_paths.is_empty() && mentions.is_empty() {
            return Err("OpenCode steer prompt was empty.".to_string());
        }
        let steer_turn_id = format!(
            "opencode-steer-{}-{}",
            self.profile_id,
            self.next_turn_id.fetch_add(1, Ordering::Relaxed)
        );
        self.turn_interrupt(thread_id, turn_id)?;
        thread::sleep(Duration::from_millis(150));
        let steer_prompt = format!(
            "Previous response was interrupted. Follow this updated instruction instead:\n\n{}",
            trimmed
        );
        let body = self.build_prompt_body(&steer_prompt, local_image_paths, mentions, None, None, None);
        self.dispatch_prompt(thread_id, steer_turn_id.clone(), body, None);
        Ok(steer_turn_id)
    }

    pub fn respond_to_server_request(&self, request_id: i64, result: Value) -> Result<(), String> {
        let Some(pending) = self.pending_request(request_id) else {
            return Err(format!("OpenCode request {request_id} was not found"));
        };

        match pending.kind {
            OpenCodePendingRequestKind::Permission => {
                let reply = result
                    .get("response")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        let decision = result.get("decision")?;
                        if let Some(value) = decision.as_str() {
                            return Some(match value {
                                "accept" => "once".to_string(),
                                "acceptForSession" | "always" => "always".to_string(),
                                "decline" | "cancel" | "reject" => "reject".to_string(),
                                other => other.to_string(),
                            });
                        }
                        None
                    })
                    .unwrap_or_else(|| "once".to_string());
                eprintln!(
                    "[opencode:{}] permission.reply request_id={} remote_id={} session_id={:?} cwd={:?} reply={}",
                    self.log_label,
                    request_id,
                    pending.remote_id,
                    pending.session_id,
                    pending.directory,
                    reply,
                );
                self.reply_permission_request(
                    &pending.remote_id,
                    pending.session_id.as_deref(),
                    pending.directory.as_deref(),
                    &reply,
                )?;
                let _ = self.clear_pending_request(request_id);
                self.emit_server_request_resolved(request_id);
                Ok(())
            }
            OpenCodePendingRequestKind::Question => {
                if result.get("answers").is_none() {
                    self.post_json(
                        &format!("/question/{}/reject", pending.remote_id),
                        Value::Null,
                        pending.directory.as_deref(),
                    )?;
                    let _ = self.clear_pending_request(request_id);
                    self.emit_server_request_resolved(request_id);
                    return Ok(());
                }

                let answers_object = result
                    .get("answers")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        format!("OpenCode question request {request_id} is missing answers")
                    })?;
                let answers = pending
                    .question_ids
                    .iter()
                    .map(|question_id| {
                        answers_object
                            .get(question_id)
                            .and_then(|value| value.get("answers"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>();
                self.post_json(
                    &format!("/question/{}/reply", pending.remote_id),
                    json!({ "answers": answers }),
                    pending.directory.as_deref(),
                )?;
                let _ = self.clear_pending_request(request_id);
                self.emit_server_request_resolved(request_id);
                Ok(())
            }
        }
    }
}
