pub mod background_repo;
pub use enzim_core::csv;
pub use enzim_core::data_model::{
    CodexProfileRecord, LocalChatTurnInput, LocalChatTurnRecord, RemotePendingPromptRecord,
    RemoteTelegramAccountRecord, ThreadAutocloseConfig, ThreadRecord, VoiceToTextConfig, WorkspaceRecord,
    WorkspaceWithThreads,
};
pub use enzim_core::data_support::{
    PROFILE_HOME_OVERRIDE_ENV, PROFILE_ICON_POOL, configured_profile_home_dir,
    default_app_data_dir, format_relative_age, profile_home_override_dir, unix_now,
};

use rusqlite::Connection;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct AppDb {
    conn: RefCell<Connection>,
}

#[derive(Clone, Debug)]
pub struct EnzimAgentConfigRecord {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model_id: Option<String>,
    pub system_prompt_override: Option<String>,
    pub cached_models_json: Option<String>,
    pub cached_models_refreshed_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub struct EnzimAgentLoopRecord {
    pub id: i64,
    pub local_thread_id: i64,
    pub status: String,
    pub prompt_text: String,
    pub instructions_text: String,
    pub backend_kind: String,
    pub remote_thread_id_snapshot: Option<String>,
    pub config_base_url_snapshot: String,
    pub config_model_id_snapshot: String,
    pub system_prompt_snapshot: String,
    pub iteration_count: i64,
    pub error_count: i64,
    pub last_seen_external_turn_id: Option<String>,
    pub final_summary_text: Option<String>,
    pub last_error_text: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct EnzimAgentLoopEventRecord {
    pub id: i64,
    pub loop_id: i64,
    pub sequence_no: i64,
    pub event_kind: String,
    pub author_kind: String,
    pub external_turn_id: Option<String>,
    pub full_text: Option<String>,
    pub compact_text: Option<String>,
    pub decision_json: Option<String>,
    pub created_at: i64,
}

mod app_db_impl;
fn default_db_path() -> PathBuf {
    default_app_data_dir().join("enzimcoder.db")
}
