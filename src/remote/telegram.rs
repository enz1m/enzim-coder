use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct TelegramAuthMatch {
    pub chat_id: String,
    pub user_id: String,
    pub username: Option<String>,
}

pub fn start_telegram_auth_poll(
    bot_token: String,
    expected_code: String,
    timeout: Duration,
) -> (
    mpsc::Receiver<Result<TelegramAuthMatch, String>>,
    Arc<AtomicBool>,
) {
    let (tx, rx) = mpsc::channel::<Result<TelegramAuthMatch, String>>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel.clone();

    thread::spawn(move || {
        let client = match TelegramClient::new(bot_token.clone()) {
            Ok(client) => client,
            Err(err) => {
                let _ = tx.send(Err(err));
                return;
            }
        };

        if let Err(err) = client.verify_token() {
            let _ = tx.send(Err(err));
            return;
        }

        let mut offset: Option<i64> = None;
        let deadline = Instant::now() + timeout;
        let expected_code = expected_code.trim().to_string();
        let mut consecutive_errors = 0usize;

        while Instant::now() < deadline {
            if cancel_for_thread.load(Ordering::Relaxed) {
                let _ = tx.send(Err("Telegram authentication was cancelled.".to_string()));
                return;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let poll_timeout = remaining.as_secs().clamp(1, 20) as i64;

            match client.get_updates(offset, poll_timeout) {
                Ok((next_offset, updates)) => {
                    consecutive_errors = 0;
                    offset = next_offset.or(offset);
                    for update in updates {
                        if let Some(found) = extract_auth_match(&update, &expected_code) {
                            let _ = tx.send(Ok(found));
                            return;
                        }
                    }
                }
                Err(err) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    if consecutive_errors >= 3 {
                        let _ = tx.send(Err(err));
                        return;
                    }
                    thread::sleep(Duration::from_millis(350));
                }
            }
        }

        let _ = tx.send(Err(
            "Timed out waiting for the 6-digit code in Telegram (3 minutes).".to_string(),
        ));
    });

    (rx, cancel)
}

pub struct TelegramClient {
    bot_token: String,
    http: Client,
}

impl TelegramClient {
    pub fn new(bot_token: String) -> Result<Self, String> {
        let bot_token = bot_token.trim().to_string();
        if bot_token.is_empty() {
            return Err("Telegram bot token is required.".to_string());
        }
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(35))
            .build()
            .map_err(|err| format!("Failed to create Telegram HTTP client: {err}"))?;
        Ok(Self { bot_token, http })
    }

    pub fn verify_token(&self) -> Result<(), String> {
        let value = self.post_method("getMe", json!({}))?;
        let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            return Ok(());
        }
        Err(self.error_from_response(&value, "Telegram token verification failed"))
    }

    #[allow(dead_code)]
    pub fn send_html_message(
        &self,
        chat_id: &str,
        html_text: &str,
        reply_to_message_id: Option<i64>,
    ) -> Result<i64, String> {
        self.send_html_message_with_markup(chat_id, html_text, reply_to_message_id, None)
    }

    pub fn send_html_message_with_markup(
        &self,
        chat_id: &str,
        html_text: &str,
        reply_to_message_id: Option<i64>,
        reply_markup: Option<Value>,
    ) -> Result<i64, String> {
        let mut payload = serde_json::Map::new();
        payload.insert("chat_id".to_string(), Value::String(chat_id.to_string()));
        payload.insert("text".to_string(), Value::String(html_text.to_string()));
        payload.insert("parse_mode".to_string(), Value::String("HTML".to_string()));
        if let Some(message_id) = reply_to_message_id {
            payload.insert(
                "reply_to_message_id".to_string(),
                Value::Number(message_id.into()),
            );
        }
        if let Some(reply_markup) = reply_markup {
            payload.insert("reply_markup".to_string(), reply_markup);
        }
        let value = self.post_method("sendMessage", Value::Object(payload))?;
        let message_id = value
            .get("result")
            .and_then(|result| result.get("message_id"))
            .and_then(Value::as_i64);
        message_id.ok_or_else(|| "Telegram sendMessage response missing message_id.".to_string())
    }

    pub fn edit_html_message_with_markup(
        &self,
        chat_id: &str,
        message_id: i64,
        html_text: &str,
        reply_markup: Option<Value>,
    ) -> Result<(), String> {
        let mut payload = serde_json::Map::new();
        payload.insert("chat_id".to_string(), Value::String(chat_id.to_string()));
        payload.insert("message_id".to_string(), Value::Number(message_id.into()));
        payload.insert("text".to_string(), Value::String(html_text.to_string()));
        payload.insert("parse_mode".to_string(), Value::String("HTML".to_string()));
        if let Some(reply_markup) = reply_markup {
            payload.insert("reply_markup".to_string(), reply_markup);
        }
        let value = self.post_method("editMessageText", Value::Object(payload))?;
        let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            return Ok(());
        }
        Err(self.error_from_response(&value, "Telegram editMessageText failed"))
    }

    pub fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<(), String> {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "callback_query_id".to_string(),
            Value::String(callback_query_id.to_string()),
        );
        if let Some(text) = text {
            payload.insert("text".to_string(), Value::String(text.to_string()));
            payload.insert("show_alert".to_string(), Value::Bool(false));
        }
        let value = self.post_method("answerCallbackQuery", Value::Object(payload))?;
        let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            return Ok(());
        }
        Err(self.error_from_response(&value, "Telegram answerCallbackQuery failed"))
    }

    pub fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_secs: i64,
    ) -> Result<(Option<i64>, Vec<Value>), String> {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "timeout".to_string(),
            Value::Number(timeout_secs.max(1).into()),
        );
        payload.insert(
            "allowed_updates".to_string(),
            json!(["message", "callback_query"]),
        );
        if let Some(offset) = offset {
            payload.insert("offset".to_string(), Value::Number(offset.into()));
        }

        let value = self.post_method("getUpdates", Value::Object(payload))?;
        let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if !ok {
            return Err(self.error_from_response(&value, "Telegram getUpdates failed"));
        }

        let updates = value
            .get("result")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let next_offset = updates
            .iter()
            .filter_map(|update| update.get("update_id").and_then(Value::as_i64))
            .max()
            .map(|max_id| max_id + 1);

        Ok((next_offset, updates))
    }

    fn post_method(&self, method: &str, payload: Value) -> Result<Value, String> {
        let url = format!("https://api.telegram.org/bot{}/{}", self.bot_token, method);
        let response = self
            .http
            .post(&url)
            .json(&payload)
            .send()
            .map_err(|err| format!("Telegram request `{method}` failed: {err}"))?;
        let status = response.status();
        let parsed: Value = response.json().map_err(|err| {
            format!("Telegram request `{method}` returned invalid JSON: {err} (status: {status})")
        })?;
        if !status.is_success() {
            return Err(self.error_from_response(
                &parsed,
                &format!("Telegram request `{method}` failed with HTTP {status}"),
            ));
        }
        Ok(parsed)
    }

    fn error_from_response(&self, value: &Value, default_message: &str) -> String {
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(default_message);
        let code = value
            .get("error_code")
            .and_then(Value::as_i64)
            .map(|code| format!(" ({code})"))
            .unwrap_or_default();
        format!("{description}{code}")
    }
}

fn extract_auth_match(update: &Value, expected_code: &str) -> Option<TelegramAuthMatch> {
    let message = update.get("message")?;
    let text = message.get("text").and_then(Value::as_str)?.trim();
    if text != expected_code {
        return None;
    }

    let chat_id = value_to_id_string(message.get("chat")?.get("id")?)?;
    let from = message.get("from")?;
    let user_id = value_to_id_string(from.get("id")?)?;
    let username = from
        .get("username")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    Some(TelegramAuthMatch {
        chat_id,
        user_id,
        username,
    })
}

fn value_to_id_string(value: &Value) -> Option<String> {
    if let Some(raw) = value.as_i64() {
        return Some(raw.to_string());
    }
    value
        .as_str()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
