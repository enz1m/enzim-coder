use crate::data_support::{format_relative_age, unix_now};

#[derive(Clone, Debug)]
pub struct WorkspaceRecord {
    pub id: i64,
    pub name: String,
    pub path: String,
    #[allow(dead_code)]
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct CodexProfileRecord {
    pub id: i64,
    pub backend_kind: String,
    pub name: String,
    pub icon_name: String,
    pub home_dir: String,
    pub last_account_type: Option<String>,
    pub last_email: Option<String>,
    pub status: String,
    #[allow(dead_code)]
    pub created_at: i64,
    #[allow(dead_code)]
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub struct ThreadRecord {
    pub id: i64,
    pub workspace_id: i64,
    pub profile_id: i64,
    pub parent_thread_id: Option<i64>,
    pub worktree_path: Option<String>,
    #[allow(dead_code)]
    pub worktree_branch: Option<String>,
    pub worktree_active: bool,
    pub title: String,
    pub codex_thread_id: Option<String>,
    pub codex_account_type: Option<String>,
    pub codex_account_email: Option<String>,
    pub created_at: i64,
    #[allow(dead_code)]
    pub updated_at: i64,
}

impl ThreadRecord {
    #[allow(dead_code)]
    pub fn relative_time(&self) -> String {
        format_relative_age(self.updated_at.max(self.created_at))
    }

    #[allow(dead_code)]
    pub fn remote_thread_id(&self) -> Option<&str> {
        self.codex_thread_id.as_deref()
    }

    #[allow(dead_code)]
    pub fn remote_thread_id_owned(&self) -> Option<String> {
        self.remote_thread_id().map(ToOwned::to_owned)
    }

    #[allow(dead_code)]
    pub fn remote_account_type(&self) -> Option<&str> {
        self.codex_account_type.as_deref()
    }

    #[allow(dead_code)]
    pub fn remote_account_type_owned(&self) -> Option<String> {
        self.remote_account_type().map(ToOwned::to_owned)
    }

    #[allow(dead_code)]
    pub fn remote_account_email(&self) -> Option<&str> {
        self.codex_account_email.as_deref()
    }

    #[allow(dead_code)]
    pub fn remote_account_email_owned(&self) -> Option<String> {
        self.remote_account_email().map(ToOwned::to_owned)
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceWithThreads {
    pub workspace: WorkspaceRecord,
    pub threads: Vec<ThreadRecord>,
}

#[derive(Clone, Debug)]
pub struct VoiceToTextConfig {
    pub provider: String,
    pub local_whisper_command: String,
    pub local_model_path: Option<String>,
    pub cloud_provider: String,
    pub cloud_url: Option<String>,
    pub cloud_api_key: Option<String>,
    pub cloud_model: Option<String>,
    pub updated_at: i64,
}

impl Default for VoiceToTextConfig {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            local_whisper_command: "whisper".to_string(),
            local_model_path: None,
            cloud_provider: "openai".to_string(),
            cloud_url: Some("https://api.openai.com/v1/audio/transcriptions".to_string()),
            cloud_api_key: None,
            cloud_model: Some("gpt-4o-mini-transcribe".to_string()),
            updated_at: unix_now(),
        }
    }
}

impl VoiceToTextConfig {
    pub fn is_valid(&self) -> bool {
        match self.provider.as_str() {
            "local" => self
                .local_model_path
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false),
            "cloud" => {
                let has_url = self
                    .cloud_url
                    .as_deref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
                let has_key = self
                    .cloud_api_key
                    .as_deref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
                let needs_model = self.cloud_provider != "azure";
                let has_model = self
                    .cloud_model
                    .as_deref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
                has_url && has_key && (!needs_model || has_model)
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct RemoteTelegramAccountRecord {
    pub id: i64,
    pub bot_token: String,
    pub telegram_user_id: String,
    pub telegram_chat_id: String,
    pub telegram_username: Option<String>,
    pub linked_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct RemotePendingPromptRecord {
    pub id: i64,
    pub local_thread_id: i64,
    pub source: String,
    pub telegram_chat_id: Option<String>,
    pub telegram_message_id: Option<String>,
    pub telegram_user_id: Option<String>,
    pub telegram_username: Option<String>,
    pub text: String,
    pub created_at: i64,
    pub consumed_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct LocalChatTurnRecord {
    pub external_turn_id: String,
    pub user_text: String,
    pub assistant_text: String,
    pub raw_items_json: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct LocalChatTurnInput {
    pub external_turn_id: String,
    pub user_text: String,
    pub assistant_text: String,
    pub raw_items_json: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}
