use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

pub use enzim_core::appserver_types::{
    AccountInfo, AppServerNotification, McpServerInfo, ModelInfo, SkillInfo,
};
pub use enzim_core::codex_support::cli_available;
use enzim_core::codex_support::{build_codex_command, format_rpc_error, running_in_flatpak};

type PendingMap = Arc<Mutex<HashMap<i64, mpsc::Sender<Result<Value, String>>>>>;

#[derive(Clone)]
pub struct CodexAppServer {
    writer_tx: mpsc::Sender<String>,
    pending: PendingMap,
    next_id: Arc<AtomicI64>,
    subscribers: Arc<Mutex<Vec<mpsc::Sender<AppServerNotification>>>>,
    child: Arc<Mutex<Option<std::process::Child>>>,
    model_list_cache: Arc<Mutex<HashMap<(bool, usize), Result<Vec<ModelInfo>, String>>>>,
    log_label: String,
}

impl CodexAppServer {
    fn build_turn_input_items(
        text: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
    ) -> Vec<Value> {
        let mut input_items = Vec::new();
        if !text.trim().is_empty() {
            input_items.push(json!({ "type": "text", "text": text }));
        }
        for path in local_image_paths {
            if !path.trim().is_empty() {
                input_items.push(json!({
                    "type": "localImage",
                    "path": path
                }));
            }
        }
        for (name, path) in mentions {
            input_items.push(json!({
                "type": "mention",
                "name": name,
                "path": path
            }));
        }
        input_items
    }

    #[allow(dead_code)]
    pub fn connect() -> Result<Arc<Self>, String> {
        Self::connect_with_home_and_label(None, "system")
    }

    #[allow(dead_code)]
    pub fn connect_with_home(home_dir: Option<&Path>) -> Result<Arc<Self>, String> {
        Self::connect_with_home_and_label(home_dir, "system")
    }

    pub fn connect_with_home_and_label(
        home_dir: Option<&Path>,
        log_label: &str,
    ) -> Result<Arc<Self>, String> {
        let via_flatpak_host = running_in_flatpak();
        let mut command = build_codex_command(home_dir)?;
        command
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().map_err(|err| {
            if via_flatpak_host {
                format!("failed to spawn `flatpak-spawn --host codex app-server`: {err}")
            } else {
                format!("failed to spawn `codex app-server`: {err}")
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to capture app-server stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture app-server stdout".to_string())?;

        let (writer_tx, writer_rx) = mpsc::channel::<String>();
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let subscribers: Arc<Mutex<Vec<mpsc::Sender<AppServerNotification>>>> =
            Arc::new(Mutex::new(Vec::new()));

        thread::spawn(move || {
            let mut writer = BufWriter::new(stdin);
            while let Ok(line) = writer_rx.recv() {
                if writer.write_all(line.as_bytes()).is_err() {
                    break;
                }
                if writer.write_all(b"\n").is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        });

        {
            let pending = pending.clone();
            let subscribers = subscribers.clone();
            let log_label = log_label.to_string();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let parsed: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(err) => {
                            eprintln!("[app-server:{log_label}] parse error: {err} :: {line}");
                            continue;
                        }
                    };

                    let has_result =
                        parsed.get("result").is_some() || parsed.get("error").is_some();
                    if has_result {
                        let Some(id) = parsed.get("id").and_then(Value::as_i64) else {
                            continue;
                        };
                        let tx = pending.lock().ok().and_then(|mut p| p.remove(&id));
                        if let Some(tx) = tx {
                            if let Some(result) = parsed.get("result") {
                                let _ = tx.send(Ok(result.clone()));
                            } else if let Some(error) = parsed.get("error") {
                                let _ = tx.send(Err(format_rpc_error(error)));
                            } else {
                                let _ = tx.send(Err(
                                    "app-server response missing result/error".to_string()
                                ));
                            }
                        }
                    } else if let Some(method) = parsed.get("method").and_then(Value::as_str) {
                        if let Some(params) = parsed.get("params") {
                            let event = AppServerNotification {
                                request_id: parsed.get("id").and_then(Value::as_i64),
                                method: method.to_string(),
                                params: params.clone(),
                            };
                            if let Ok(mut subs) = subscribers.lock() {
                                subs.retain(|tx| tx.send(event.clone()).is_ok());
                            }
                        }

                        if method.starts_with("turn/")
                            || method.starts_with("item/")
                            || method.starts_with("thread/")
                        {
                            eprintln!("[app-server:{log_label}] event: {method}");
                        }
                    }
                }
            });
        }

        let client = Arc::new(Self {
            writer_tx,
            pending,
            next_id: Arc::new(AtomicI64::new(1)),
            subscribers,
            child: Arc::new(Mutex::new(Some(child))),
            model_list_cache: Arc::new(Mutex::new(HashMap::new())),
            log_label: log_label.to_string(),
        });

        client.initialize()?;
        Ok(client)
    }

    pub fn subscribe_notifications(&self) -> mpsc::Receiver<AppServerNotification> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.push(tx);
        }
        rx
    }

    pub fn model_list(&self, include_hidden: bool, limit: usize) -> Result<Vec<ModelInfo>, String> {
        let cache_key = (include_hidden, limit);
        if let Ok(cache) = self.model_list_cache.lock() {
            if let Some(cached) = cache.get(&cache_key).cloned() {
                return cached;
            }
        }

        let fetched = (|| {
            let result = self.request(
                "model/list",
                json!({
                    "includeHidden": include_hidden,
                    "limit": limit
                }),
            )?;
            let data = result
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| "model/list response missing `data` array".to_string())?;

            let mut out = Vec::new();
            for entry in data {
                let id = entry
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| entry.get("model").and_then(Value::as_str))
                    .unwrap_or("unknown-model")
                    .to_string();
                let display_name = entry
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string();
                let is_default = entry
                    .get("isDefault")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                out.push(ModelInfo {
                    id,
                    display_name,
                    is_default,
                    variants: Vec::new(),
                    default_reasoning_effort: entry
                        .get("defaultReasoningEffort")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    reasoning_efforts: entry
                        .get("supportedReasoningEfforts")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| {
                                    item.get("reasoningEffort")
                                        .and_then(Value::as_str)
                                        .map(ToOwned::to_owned)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                });
            }
            Ok(out)
        })();
        if let Ok(mut cache) = self.model_list_cache.lock() {
            cache.insert(cache_key, fetched.clone());
        }
        fetched
    }

    pub fn account_read(&self, refresh_token: bool) -> Result<Option<AccountInfo>, String> {
        let result = self.request("account/read", json!({ "refreshToken": refresh_token }))?;
        let Some(account) = result.get("account").and_then(Value::as_object) else {
            return Ok(None);
        };

        let account_type = account
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let email = account
            .get("email")
            .and_then(Value::as_str)
            .map(|value| value.to_string());

        Ok(Some(AccountInfo {
            account_type,
            email,
        }))
    }

    pub fn account_login_start_chatgpt(&self) -> Result<(String, String), String> {
        let result = self.request("account/login/start", json!({ "type": "chatgpt" }))?;
        let login_id = result
            .get("loginId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let auth_url = result
            .get("authUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if login_id.is_empty() || auth_url.is_empty() {
            return Err("account/login/start missing loginId/authUrl".to_string());
        }
        Ok((login_id, auth_url))
    }

    #[allow(dead_code)]
    pub fn account_login_start_api_key(&self, api_key: &str) -> Result<(), String> {
        let _ = self.request(
            "account/login/start",
            json!({ "type": "apiKey", "apiKey": api_key }),
        )?;
        Ok(())
    }

    pub fn account_logout(&self) -> Result<(), String> {
        let _ = self.request("account/logout", json!({}))?;
        Ok(())
    }

    pub fn skills_list(
        &self,
        cwds: &[String],
        force_reload: bool,
    ) -> Result<Vec<SkillInfo>, String> {
        let result = self.request(
            "skills/list",
            json!({
                "cwds": cwds,
                "forceReload": force_reload
            }),
        )?;

        let mut by_name = HashMap::<String, SkillInfo>::new();
        if let Some(entries) = result.get("data").and_then(Value::as_array) {
            for entry in entries {
                let skills = entry
                    .get("skills")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for raw in skills {
                    let Some(name) = raw.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    let key = name.trim().to_ascii_lowercase();
                    if key.is_empty() {
                        continue;
                    }
                    by_name.entry(key).or_insert(SkillInfo {
                        name: name.to_string(),
                    });
                }
            }
        }
        let mut out = by_name.into_values().collect::<Vec<_>>();
        out.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        });
        Ok(out)
    }

    pub fn mcp_server_status_list(&self, limit: usize) -> Result<Vec<McpServerInfo>, String> {
        let mut cursor: Option<String> = None;
        let mut all = Vec::<McpServerInfo>::new();

        loop {
            let mut params = serde_json::Map::new();
            params.insert("limit".to_string(), json!(limit.max(1).min(200)));
            if let Some(cursor_value) = cursor.clone() {
                params.insert("cursor".to_string(), Value::String(cursor_value));
            }
            let result = self.request("mcpServerStatus/list", Value::Object(params))?;
            if let Some(data) = result.get("data").and_then(Value::as_array) {
                for row in data {
                    let name = row
                        .get("name")
                        .or_else(|| row.get("server"))
                        .or_else(|| row.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if name.trim().is_empty() {
                        continue;
                    }
                    let auth_value = row
                        .get("authStatus")
                        .or_else(|| row.get("auth"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let (authenticated, auth_label) = parse_mcp_auth(&auth_value);

                    all.push(McpServerInfo {
                        name,
                        authenticated,
                        auth_label,
                    });
                }
            }

            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty());
            if cursor.is_none() {
                break;
            }
        }

        all.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        });
        all.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
        Ok(all)
    }

    pub fn mcp_server_oauth_login(&self, server_name: &str) -> Result<String, String> {
        let result = self.request(
            "mcpServer/oauth/login",
            json!({
                "name": server_name
            }),
        )?;
        result
            .get("authorization_url")
            .or_else(|| result.get("authorizationUrl"))
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "mcpServer/oauth/login response missing authorization URL".to_string())
    }

    pub fn config_mcp_server_reload(&self) -> Result<(), String> {
        let _ = self.request("config/mcpServer/reload", json!({}))?;
        Ok(())
    }

    pub fn config_value_write(
        &self,
        key_path: &str,
        value: Value,
        merge_strategy: &str,
    ) -> Result<(), String> {
        let _ = self.request(
            "config/value/write",
            json!({
                "keyPath": key_path,
                "value": value,
                "mergeStrategy": merge_strategy,
            }),
        )?;
        Ok(())
    }

    pub fn config_batch_write(&self, edits: Vec<(String, Value, String)>) -> Result<(), String> {
        let normalized_edits = edits
            .into_iter()
            .map(|(key_path, value, merge_strategy)| {
                json!({
                    "keyPath": key_path,
                    "value": value,
                    "mergeStrategy": merge_strategy
                })
            })
            .collect::<Vec<_>>();
        let _ = self.request(
            "config/batchWrite",
            json!({
                "edits": normalized_edits
            }),
        )?;
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), String> {
        eprintln!("[app-server:{}] shutting down", self.log_label);
        if let Ok(mut child_guard) = self.child.lock() {
            if let Some(mut child) = child_guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(())
    }

    pub fn thread_start(&self, cwd: Option<&str>, model: Option<&str>) -> Result<String, String> {
        let mut params = serde_json::Map::new();
        if let Some(cwd) = cwd {
            params.insert("cwd".to_string(), Value::String(cwd.to_string()));
        }
        if let Some(model) = model {
            params.insert("model".to_string(), Value::String(model.to_string()));
        }
        let result = self.request("thread/start", Value::Object(params))?;
        result
            .get("thread")
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
            .map(|id| id.to_string())
            .ok_or_else(|| "thread/start response missing thread.id".to_string())
    }

    pub fn thread_resume(
        &self,
        thread_id: &str,
        cwd: Option<&str>,
        model: Option<&str>,
    ) -> Result<String, String> {
        let mut params = serde_json::Map::new();
        params.insert("threadId".to_string(), Value::String(thread_id.to_string()));
        if let Some(cwd) = cwd {
            params.insert("cwd".to_string(), Value::String(cwd.to_string()));
        }
        if let Some(model) = model {
            params.insert("model".to_string(), Value::String(model.to_string()));
        }
        let result = self.request("thread/resume", Value::Object(params))?;
        result
            .get("thread")
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
            .map(|id| id.to_string())
            .ok_or_else(|| "thread/resume response missing thread.id".to_string())
    }

    pub fn thread_read(&self, thread_id: &str, include_turns: bool) -> Result<Value, String> {
        let result = self.request(
            "thread/read",
            json!({
                "threadId": thread_id,
                "includeTurns": include_turns
            }),
        )?;
        result
            .get("thread")
            .cloned()
            .ok_or_else(|| "thread/read response missing thread".to_string())
    }

    pub fn thread_fork(&self, thread_id: &str) -> Result<String, String> {
        let result = self.request("thread/fork", json!({ "threadId": thread_id }))?;
        result
            .get("thread")
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
            .map(|id| id.to_string())
            .ok_or_else(|| "thread/fork response missing thread.id".to_string())
    }

    pub fn thread_rollback(&self, thread_id: &str, count: usize) -> Result<Value, String> {
        let result = self.request(
            "thread/rollback",
            json!({
                "threadId": thread_id,
                "numTurns": count
            }),
        )?;
        result
            .get("thread")
            .cloned()
            .ok_or_else(|| "thread/rollback response missing thread".to_string())
    }

    pub fn thread_archive(&self, thread_id: &str) -> Result<(), String> {
        let _ = self.request("thread/archive", json!({ "threadId": thread_id }))?;
        Ok(())
    }

    pub fn turn_start(
        &self,
        thread_id: &str,
        text: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
        model: Option<&str>,
        effort: Option<&str>,
        sandbox_policy: Option<Value>,
        approval_policy: Option<&str>,
        collaboration_mode: Option<Value>,
        cwd: Option<&str>,
    ) -> Result<String, String> {
        let make_params = |include_collaboration_mode: bool| {
            let mut params = serde_json::Map::new();
            let input_items = Self::build_turn_input_items(text, local_image_paths, mentions);
            params.insert("threadId".to_string(), Value::String(thread_id.to_string()));
            params.insert("input".to_string(), Value::Array(input_items));
            if let Some(approval_policy) = approval_policy {
                params.insert(
                    "approvalPolicy".to_string(),
                    Value::String(approval_policy.to_string()),
                );
            }
            if let Some(model) = model {
                params.insert("model".to_string(), Value::String(model.to_string()));
            }
            if let Some(effort) = effort {
                params.insert("effort".to_string(), Value::String(effort.to_string()));
            }
            if let Some(policy) = sandbox_policy.clone() {
                params.insert("sandboxPolicy".to_string(), policy);
            }
            if let Some(cwd) = cwd {
                params.insert("cwd".to_string(), Value::String(cwd.to_string()));
            }
            if include_collaboration_mode {
                if let Some(mode) = collaboration_mode.clone() {
                    params.insert("collaborationMode".to_string(), mode);
                }
            }
            Value::Object(params)
        };

        let parse_turn_id = |result: Value| {
            result
                .get("turn")
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
                .map(|id| id.to_string())
                .ok_or_else(|| "turn/start response missing turn.id".to_string())
        };

        let request_turn_start = |include_collaboration_mode: bool| -> Result<String, String> {
            match self.request("turn/start", make_params(include_collaboration_mode)) {
                Ok(result) => parse_turn_id(result),
                Err(err) if err.contains("thread not found") => {
                    self.thread_resume(thread_id, cwd, model)?;
                    let result =
                        self.request("turn/start", make_params(include_collaboration_mode))?;
                    parse_turn_id(result)
                }
                Err(err) => Err(err),
            }
        };

        let uses_collaboration_mode = collaboration_mode.is_some();
        match request_turn_start(uses_collaboration_mode) {
            Ok(turn_id) => Ok(turn_id),
            Err(err)
                if uses_collaboration_mode
                    && (err.contains("collaborationMode")
                        || err.contains("CollaborationMode")
                        || err.contains("experimentalApi capability")) =>
            {
                request_turn_start(false)
            }
            Err(err) => Err(err),
        }
    }

    pub fn turn_interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), String> {
        let _ = self.request(
            "turn/interrupt",
            json!({
                "threadId": thread_id,
                "turnId": turn_id
            }),
        )?;
        Ok(())
    }

    pub fn turn_steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        text: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
    ) -> Result<String, String> {
        let input_items = Self::build_turn_input_items(text, local_image_paths, mentions);
        let result = self.request(
            "turn/steer",
            json!({
                "threadId": thread_id,
                "input": input_items,
                "expectedTurnId": expected_turn_id
            }),
        )?;

        result
            .get("turnId")
            .and_then(Value::as_str)
            .map(|id| id.to_string())
            .ok_or_else(|| "turn/steer response missing turnId".to_string())
    }

    fn initialize(&self) -> Result<(), String> {
        let _ = self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "enzimcoder_gtk",
                    "title": "Enzim Coder",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": true
                }
            }),
        )?;
        self.notify("initialized", json!({}))
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<Result<Value, String>>();
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| "failed to lock pending response map".to_string())?;
            pending.insert(id, tx);
        }

        let payload = json!({
            "id": id,
            "method": method,
            "params": params
        });
        self.writer_tx
            .send(payload.to_string())
            .map_err(|err| format!("failed to send app-server request `{method}`: {err}"))?;

        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => result,
            Err(err) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                Err(format!("timed out waiting for `{method}` response: {err}"))
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let payload = json!({
            "method": method,
            "params": params
        });
        self.writer_tx
            .send(payload.to_string())
            .map_err(|err| format!("failed to send app-server notification `{method}`: {err}"))
    }

    pub fn respond_to_server_request(&self, request_id: i64, result: Value) -> Result<(), String> {
        let payload = json!({
            "id": request_id,
            "result": result
        });
        self.writer_tx.send(payload.to_string()).map_err(|err| {
            format!("failed to send app-server request response `{request_id}`: {err}")
        })
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn parse_mcp_auth(value: &Value) -> (bool, String) {
    let is_no_auth_status = |raw: &str| {
        matches!(
            raw,
            "unsupported" | "not_supported" | "none" | "n/a" | "na" | "not-applicable"
        )
    };

    if let Some(flag) = value.as_bool() {
        return (
            flag,
            if flag {
                "Authenticated".to_string()
            } else {
                "Auth required".to_string()
            },
        );
    }

    if let Some(status) = value.get("status").and_then(Value::as_str) {
        let lower = status.to_ascii_lowercase();
        if is_no_auth_status(&lower) {
            return (true, "No auth".to_string());
        }
        let authenticated = matches!(lower.as_str(), "ok" | "authenticated" | "connected");
        return (authenticated, status.to_string());
    }

    if let Some(label) = value.as_str() {
        let lower = label.to_ascii_lowercase();
        if is_no_auth_status(&lower) {
            return (true, "No auth".to_string());
        }
        let authenticated = matches!(lower.as_str(), "ok" | "authenticated" | "connected");
        return (authenticated, label.to_string());
    }

    if value.is_null() {
        return (true, "Unknown".to_string());
    }

    let fallback = value.to_string();
    let lower = fallback.to_ascii_lowercase();
    if lower.contains("unsupported") {
        return (true, "No auth".to_string());
    }
    let authenticated = lower.contains("auth")
        && !lower.contains("required")
        && !lower.contains("missing")
        && !lower.contains("failed");
    (authenticated, fallback)
}
