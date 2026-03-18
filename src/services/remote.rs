pub use crate::remote::{
    bool_from_setting, generate_auth_code, mask_bot_token, start_telegram_auth_poll,
    SETTING_REMOTE_MODE_ENABLED, SETTING_REMOTE_TELEGRAM_ACTIVATE_LOCAL_THREAD_ID,
    SETTING_REMOTE_TELEGRAM_ACTIVE_ACCOUNT_ID, SETTING_REMOTE_TELEGRAM_AUTH_EXPECTED_CODE,
    SETTING_REMOTE_TELEGRAM_AUTH_EXPIRES_AT, SETTING_REMOTE_TELEGRAM_POLLING_ENABLED,
};

pub fn start_background_worker() {
    crate::remote::runtime::start_background_worker();
}

pub fn stop_background_worker() {
    crate::remote::runtime::stop_background_worker();
}

pub fn forward_turn_completion_if_enabled(
    db: &crate::data::AppDb,
    codex_thread_id: &str,
    turn_id: &str,
    assistant_text: &str,
    command_count: usize,
    file_edit_count: usize,
    other_action_count: usize,
) {
    crate::remote::runtime::forward_turn_completion_if_enabled(
        db,
        codex_thread_id,
        turn_id,
        assistant_text,
        command_count,
        file_edit_count,
        other_action_count,
    );
}
