use serde_json::Value;

#[derive(Clone, Debug)]
pub struct AppServerNotification {
    pub request_id: Option<i64>,
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub is_default: bool,
    pub variants: Vec<String>,
    pub default_reasoning_effort: Option<String>,
    pub reasoning_efforts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AccountInfo {
    pub account_type: String,
    pub email: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SkillInfo {
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct McpServerInfo {
    pub name: String,
    pub authenticated: bool,
    pub auth_label: String,
}
