pub use crate::backend::{AccountProviderInfo, BackendCapabilities, OAuthFlowInfo, RuntimeClient};
pub use crate::codex_appserver::{
    AccountInfo, AppServerNotification, McpServerInfo, ModelInfo, SkillInfo,
};

pub fn any_runtime_cli_available() -> bool {
    crate::backend::any_runtime_cli_available()
}

pub fn runtime_cli_available_for_backend(backend_kind: &str) -> bool {
    crate::backend::runtime_cli_available_for_backend(backend_kind)
}

pub fn backend_display_name(backend_kind: &str) -> &'static str {
    crate::backend::backend_display_name(backend_kind)
}

pub fn capabilities_for_backend_kind(backend_kind: &str) -> BackendCapabilities {
    crate::backend::capabilities_for_backend_kind(backend_kind)
}
