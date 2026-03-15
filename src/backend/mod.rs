mod opencode;

use crate::codex_appserver::{
    AccountInfo, AppServerNotification, CodexAppServer, McpServerInfo, ModelInfo, SkillInfo,
};
use crate::data::CodexProfileRecord;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, mpsc};

pub use opencode::OpenCodeAppServer;

pub fn any_runtime_cli_available() -> bool {
    crate::codex_appserver::cli_available() || opencode::opencode_cli_available()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub supports_oauth_login: bool,
    pub supports_api_key_login: bool,
    pub supports_logout: bool,
    pub supports_skill_assignment: bool,
    pub supports_mcp_management: bool,
    pub supports_fork: bool,
    pub supports_rollback: bool,
    pub supports_streaming_events: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountProviderInfo {
    pub provider_id: String,
    pub provider_name: String,
    pub connected: bool,
    pub has_saved_auth: bool,
    pub supports_oauth: bool,
    pub supports_api_key: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthFlowInfo {
    pub provider_id: String,
    pub url: String,
    pub method: String,
    pub instructions: Option<String>,
    pub method_index: u32,
}

pub const CODEX_CAPABILITIES: BackendCapabilities = BackendCapabilities {
    supports_oauth_login: true,
    supports_api_key_login: true,
    supports_logout: true,
    supports_skill_assignment: true,
    supports_mcp_management: true,
    supports_fork: true,
    supports_rollback: true,
    supports_streaming_events: true,
};

pub const OPENCODE_CAPABILITIES: BackendCapabilities = BackendCapabilities {
    supports_oauth_login: true,
    supports_api_key_login: true,
    supports_logout: true,
    supports_skill_assignment: true,
    supports_mcp_management: true,
    supports_fork: true,
    supports_rollback: true,
    supports_streaming_events: true,
};

pub fn capabilities_for_backend_kind(backend_kind: &str) -> BackendCapabilities {
    if backend_kind.eq_ignore_ascii_case("opencode") {
        OPENCODE_CAPABILITIES
    } else {
        CODEX_CAPABILITIES
    }
}

pub fn backend_display_name(backend_kind: &str) -> &'static str {
    if backend_kind.eq_ignore_ascii_case("opencode") {
        "OpenCode"
    } else {
        "Codex"
    }
}

#[derive(Clone)]
pub enum RuntimeClient {
    Codex(Arc<CodexAppServer>),
    OpenCode(Arc<OpenCodeAppServer>),
}

impl RuntimeClient {
    pub fn connect_for_profile(
        profile: Option<&CodexProfileRecord>,
        log_label: &str,
    ) -> Result<Arc<Self>, String> {
        match profile.map(|profile| profile.backend_kind.as_str()) {
            Some("opencode") => {
                let profile = profile.ok_or_else(|| "profile required".to_string())?;
                Ok(Arc::new(Self::OpenCode(
                    OpenCodeAppServer::connect_profile(profile, log_label)?,
                )))
            }
            _ => {
                let home_dir = profile.map(|profile| Path::new(profile.home_dir.as_str()));
                Ok(Arc::new(Self::Codex(
                    CodexAppServer::connect_with_home_and_label(home_dir, log_label)?,
                )))
            }
        }
    }

    pub fn subscribe_notifications(&self) -> mpsc::Receiver<AppServerNotification> {
        match self {
            Self::Codex(client) => client.subscribe_notifications(),
            Self::OpenCode(client) => client.subscribe_notifications(),
        }
    }

    pub fn backend_kind(&self) -> &'static str {
        match self {
            Self::Codex(_) => "codex",
            Self::OpenCode(_) => "opencode",
        }
    }

    pub fn model_cache_key(&self) -> String {
        match self {
            Self::Codex(client) => format!("codex:{:p}", Arc::as_ptr(client)),
            Self::OpenCode(client) => format!("opencode:{}", client.profile_id()),
        }
    }

    pub fn profile_id(&self) -> Option<i64> {
        match self {
            Self::Codex(_) => None,
            Self::OpenCode(client) => Some(client.profile_id()),
        }
    }

    pub fn capabilities(&self) -> BackendCapabilities {
        capabilities_for_backend_kind(self.backend_kind())
    }

    pub fn model_list(&self, include_hidden: bool, limit: usize) -> Result<Vec<ModelInfo>, String> {
        match self {
            Self::Codex(client) => client.model_list(include_hidden, limit),
            Self::OpenCode(client) => client.model_list(include_hidden, limit),
        }
    }

    pub fn account_read(&self, refresh_token: bool) -> Result<Option<AccountInfo>, String> {
        match self {
            Self::Codex(client) => client.account_read(refresh_token),
            Self::OpenCode(client) => client.account_read(refresh_token),
        }
    }

    pub fn account_login_start_chatgpt(&self) -> Result<(String, String), String> {
        match self {
            Self::Codex(client) => client.account_login_start_chatgpt(),
            Self::OpenCode(client) => client.account_login_start_chatgpt(),
        }
    }

    pub fn account_provider_list(&self) -> Result<Vec<AccountProviderInfo>, String> {
        match self {
            Self::Codex(_) => Ok(vec![AccountProviderInfo {
                provider_id: "openai".to_string(),
                provider_name: "OpenAI".to_string(),
                connected: false,
                has_saved_auth: false,
                supports_oauth: true,
                supports_api_key: true,
            }]),
            Self::OpenCode(client) => client.account_provider_list(),
        }
    }

    pub fn account_login_start_oauth_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<(String, String), String> {
        match self {
            Self::Codex(client) => {
                let provider_id = provider_id.trim();
                if !provider_id.is_empty() && !provider_id.eq_ignore_ascii_case("openai") {
                    return Err(format!(
                        "Codex OAuth login does not support provider `{provider_id}`."
                    ));
                }
                client.account_login_start_chatgpt()
            }
            Self::OpenCode(client) => client.account_login_start_oauth_for_provider(provider_id),
        }
    }

    pub fn account_login_start_oauth_for_provider_info(
        &self,
        provider_id: &str,
    ) -> Result<OAuthFlowInfo, String> {
        match self {
            Self::Codex(client) => {
                let provider_id = provider_id.trim();
                if !provider_id.is_empty() && !provider_id.eq_ignore_ascii_case("openai") {
                    return Err(format!(
                        "Codex OAuth login does not support provider `{provider_id}`."
                    ));
                }
                let (provider_id, url) = client.account_login_start_chatgpt()?;
                Ok(OAuthFlowInfo {
                    provider_id,
                    url,
                    method: "external".to_string(),
                    instructions: None,
                    method_index: 0,
                })
            }
            Self::OpenCode(client) => client.account_login_start_oauth_for_provider_info(provider_id),
        }
    }

    pub fn account_complete_oauth_for_provider(
        &self,
        provider_id: &str,
        method_index: u32,
        code: Option<&str>,
    ) -> Result<Option<AccountInfo>, String> {
        match self {
            Self::Codex(_) => Err(
                "Codex OAuth completion is handled outside the OpenCode provider flow."
                    .to_string(),
            ),
            Self::OpenCode(client) => {
                client.account_complete_oauth_for_provider(provider_id, method_index, code)
            }
        }
    }

    #[allow(dead_code)]
    pub fn account_login_start_api_key(&self, api_key: &str) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.account_login_start_api_key(api_key),
            Self::OpenCode(client) => client.account_login_start_api_key(api_key),
        }
    }

    pub fn account_api_key_provider_options(&self) -> Result<Vec<(String, String)>, String> {
        match self {
            Self::Codex(_) => Ok(vec![("openai".to_string(), "OpenAI".to_string())]),
            Self::OpenCode(client) => client.account_api_key_provider_options(),
        }
    }

    pub fn account_login_start_api_key_for_provider(
        &self,
        provider_id: &str,
        api_key: &str,
    ) -> Result<(), String> {
        match self {
            Self::Codex(client) => {
                let provider_id = provider_id.trim();
                if !provider_id.is_empty() && !provider_id.eq_ignore_ascii_case("openai") {
                    return Err(format!(
                        "Codex API-key login does not support provider `{provider_id}`."
                    ));
                }
                client.account_login_start_api_key(api_key)
            }
            Self::OpenCode(client) => {
                client.account_login_start_api_key_for_provider(provider_id, api_key)
            }
        }
    }

    pub fn account_logout(&self) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.account_logout(),
            Self::OpenCode(client) => client.account_logout(),
        }
    }

    #[allow(dead_code)]
    pub fn account_logout_provider(&self, provider_id: &str) -> Result<(), String> {
        match self {
            Self::Codex(client) => {
                let provider_id = provider_id.trim();
                if !provider_id.is_empty() && !provider_id.eq_ignore_ascii_case("openai") {
                    return Err(format!(
                        "Codex logout does not support provider `{provider_id}`."
                    ));
                }
                client.account_logout()
            }
            Self::OpenCode(client) => client.account_logout_provider(provider_id),
        }
    }

    pub fn skills_list(
        &self,
        cwds: &[String],
        force_reload: bool,
    ) -> Result<Vec<SkillInfo>, String> {
        match self {
            Self::Codex(client) => client.skills_list(cwds, force_reload),
            Self::OpenCode(client) => client.skills_list(cwds, force_reload),
        }
    }

    pub fn mcp_server_status_list(&self, limit: usize) -> Result<Vec<McpServerInfo>, String> {
        match self {
            Self::Codex(client) => client.mcp_server_status_list(limit),
            Self::OpenCode(client) => client.mcp_server_status_list(limit),
        }
    }

    pub fn mcp_server_oauth_login(&self, server_name: &str) -> Result<String, String> {
        match self {
            Self::Codex(client) => client.mcp_server_oauth_login(server_name),
            Self::OpenCode(client) => client.mcp_server_oauth_login(server_name),
        }
    }

    pub fn config_mcp_server_reload(&self) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.config_mcp_server_reload(),
            Self::OpenCode(client) => client.config_mcp_server_reload(),
        }
    }

    pub fn config_value_write(
        &self,
        key_path: &str,
        value: Value,
        merge_strategy: &str,
    ) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.config_value_write(key_path, value, merge_strategy),
            Self::OpenCode(client) => client.config_value_write(key_path, value, merge_strategy),
        }
    }

    pub fn config_batch_write(&self, edits: Vec<(String, Value, String)>) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.config_batch_write(edits),
            Self::OpenCode(client) => client.config_batch_write(edits),
        }
    }

    pub fn shutdown(&self) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.shutdown(),
            Self::OpenCode(client) => client.shutdown(),
        }
    }

    pub fn thread_start(
        &self,
        cwd: Option<&str>,
        model: Option<&str>,
        sandbox_policy: Option<Value>,
    ) -> Result<String, String> {
        match self {
            Self::Codex(client) => client.thread_start(cwd, model),
            Self::OpenCode(client) => client.thread_start(cwd, model, sandbox_policy),
        }
    }

    pub fn thread_resume(
        &self,
        thread_id: &str,
        cwd: Option<&str>,
        model: Option<&str>,
    ) -> Result<String, String> {
        match self {
            Self::Codex(client) => client.thread_resume(thread_id, cwd, model),
            Self::OpenCode(client) => client.thread_resume(thread_id, cwd, model),
        }
    }

    pub fn thread_set_command_mode(&self, thread_id: &str, command_mode: &str) -> Result<(), String> {
        match self {
            Self::Codex(_) => Ok(()),
            Self::OpenCode(client) => client.thread_set_command_mode(thread_id, command_mode),
        }
    }

    pub fn thread_read(&self, thread_id: &str, include_turns: bool) -> Result<Value, String> {
        match self {
            Self::Codex(client) => client.thread_read(thread_id, include_turns),
            Self::OpenCode(client) => client.thread_read(thread_id, include_turns),
        }
    }

    pub fn thread_fork(&self, thread_id: &str) -> Result<String, String> {
        match self {
            Self::Codex(client) => client.thread_fork(thread_id),
            Self::OpenCode(client) => client.thread_fork(thread_id),
        }
    }

    pub fn thread_rollback(&self, thread_id: &str, count: usize) -> Result<Value, String> {
        match self {
            Self::Codex(client) => client.thread_rollback(thread_id, count),
            Self::OpenCode(client) => client.thread_rollback(thread_id, count),
        }
    }

    pub fn thread_archive(&self, thread_id: &str) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.thread_archive(thread_id),
            Self::OpenCode(client) => client.thread_archive(thread_id),
        }
    }

    #[allow(clippy::too_many_arguments)]
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
        match self {
            Self::Codex(client) => client.turn_start(
                thread_id,
                text,
                local_image_paths,
                mentions,
                model,
                effort,
                sandbox_policy,
                approval_policy,
                collaboration_mode,
                cwd,
            ),
            Self::OpenCode(client) => client.turn_start(
                thread_id,
                text,
                local_image_paths,
                mentions,
                model,
                effort,
                sandbox_policy,
                approval_policy,
                collaboration_mode,
                cwd,
            ),
        }
    }

    pub fn turn_interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.turn_interrupt(thread_id, turn_id),
            Self::OpenCode(client) => client.turn_interrupt(thread_id, turn_id),
        }
    }

    pub fn turn_steer(
        &self,
        thread_id: &str,
        turn_id: &str,
        prompt: &str,
        local_image_paths: &[String],
        mentions: &[(String, String)],
    ) -> Result<String, String> {
        match self {
            Self::Codex(client) => {
                client.turn_steer(thread_id, turn_id, prompt, local_image_paths, mentions)
            }
            Self::OpenCode(client) => {
                client.turn_steer(thread_id, turn_id, prompt, local_image_paths, mentions)
            }
        }
    }

    pub fn respond_to_server_request(&self, request_id: i64, result: Value) -> Result<(), String> {
        match self {
            Self::Codex(client) => client.respond_to_server_request(request_id, result),
            Self::OpenCode(client) => client.respond_to_server_request(request_id, result),
        }
    }

    pub fn pending_server_requests_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Vec<Value>, String> {
        match self {
            Self::Codex(_) => Ok(Vec::new()),
            Self::OpenCode(client) => client.pending_server_requests_for_thread(thread_id),
        }
    }

    pub fn active_opencode_turn_count(&self) -> usize {
        match self {
            Self::Codex(_) => 0,
            Self::OpenCode(client) => client.active_turn_count(),
        }
    }
}
