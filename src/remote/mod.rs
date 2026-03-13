pub mod formatting;
pub mod runtime;
pub mod telegram;

use std::time::{SystemTime, UNIX_EPOCH};

pub const SETTING_REMOTE_MODE_ENABLED: &str = "remote_mode_enabled";
pub const SETTING_REMOTE_TELEGRAM_ACTIVE_ACCOUNT_ID: &str = "remote_telegram_active_account_id";
pub const SETTING_REMOTE_TELEGRAM_POLLING_ENABLED: &str = "remote_telegram_polling_enabled";
pub const SETTING_REMOTE_TELEGRAM_ACTIVATE_LOCAL_THREAD_ID: &str =
    "remote_telegram_activate_local_thread_id";
pub const SETTING_REMOTE_TELEGRAM_AUTH_EXPECTED_CODE: &str = "remote_telegram_auth_expected_code";
pub const SETTING_REMOTE_TELEGRAM_AUTH_EXPIRES_AT: &str = "remote_telegram_auth_expires_at";

pub use telegram::start_telegram_auth_poll;

pub fn bool_from_setting(raw: Option<String>, default: bool) -> bool {
    raw.map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

pub fn generate_auth_code() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mixed = nanos ^ ((std::process::id() as u64) << 16);
    let code = 100_000 + (mixed % 900_000);
    format!("{code:06}")
}

pub fn mask_bot_token(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.len() <= 8 {
        return "********".to_string();
    }
    let prefix = &trimmed[..4];
    let suffix = &trimmed[trimmed.len().saturating_sub(4)..];
    format!("{prefix}…{suffix}")
}
