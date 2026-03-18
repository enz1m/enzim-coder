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
    pub device_code: Option<String>,
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
