pub mod background_repo;
pub use enzim_core::csv;
pub use enzim_core::data_model::{
    CodexProfileRecord, LocalChatTurnInput, LocalChatTurnRecord, RemotePendingPromptRecord,
    RemoteTelegramAccountRecord, ThreadRecord, VoiceToTextConfig, WorkspaceRecord,
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

mod app_db_impl;
fn default_db_path() -> PathBuf {
    default_app_data_dir().join("enzimcoder.db")
}
