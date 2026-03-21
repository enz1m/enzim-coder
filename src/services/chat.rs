pub use crate::data::background_repo::BackgroundRepo;
pub use crate::data::{
    AppDb, CodexProfileRecord, EnzimAgentConfigRecord, EnzimAgentLoopEventRecord,
    EnzimAgentLoopRecord, LocalChatTurnInput, LocalChatTurnRecord, RemotePendingPromptRecord,
    RemoteTelegramAccountRecord, ThreadAutocloseConfig, ThreadRecord, VoiceToTextConfig,
    WorkspaceRecord, WorkspaceWithThreads,
};

pub fn default_app_data_dir() -> std::path::PathBuf {
    crate::data::default_app_data_dir()
}

pub fn configured_profile_home_dir(app_data_dir: &std::path::Path) -> std::path::PathBuf {
    crate::data::configured_profile_home_dir(app_data_dir)
}
