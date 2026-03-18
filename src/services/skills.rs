use crate::data::{AppDb, CodexProfileRecord};
use serde_json::Value;

pub use crate::skill_mcp::{
    disabled_skill_markers, McpCatalogEntry, PolicyKind, ProfileAssignments, SkillCatalogEntry,
    SkillMcpCatalog,
};

pub fn supports_skill_assignment_for_backend(backend_kind: &str) -> bool {
    crate::skill_mcp::supports_skill_assignment_for_backend(backend_kind)
}

pub fn write_skill_assignment_for_profile(
    profile: &CodexProfileRecord,
    slug: &str,
    content: &str,
    enabled: bool,
) -> Result<(), String> {
    crate::skill_mcp::write_skill_assignment_for_profile(profile, slug, content, enabled)
}

pub fn load_catalog(db: &AppDb) -> SkillMcpCatalog {
    crate::skill_mcp::load_catalog(db)
}

pub fn load_profile_assignments(db: &AppDb, profile_id: i64) -> ProfileAssignments {
    crate::skill_mcp::load_profile_assignments(db, profile_id)
}

pub fn set_profile_assigned(
    db: &AppDb,
    profile_id: i64,
    kind: PolicyKind,
    key_or_name: &str,
    enabled: bool,
) -> Result<(), String> {
    crate::skill_mcp::set_profile_assigned(db, profile_id, kind, key_or_name, enabled)
}

pub fn upsert_catalog_skill(
    db: &AppDb,
    name: &str,
    description: &str,
    content: &str,
) -> Result<SkillCatalogEntry, String> {
    crate::skill_mcp::upsert_catalog_skill(db, name, description, content)
}

pub fn remove_catalog_skill(db: &AppDb, key_or_name: &str) -> Result<(), String> {
    crate::skill_mcp::remove_catalog_skill(db, key_or_name)
}

pub fn upsert_catalog_mcp(
    db: &AppDb,
    name: &str,
    description: &str,
    config: Value,
) -> Result<McpCatalogEntry, String> {
    crate::skill_mcp::upsert_catalog_mcp(db, name, description, config)
}

pub fn remove_catalog_mcp(db: &AppDb, key_or_name: &str) -> Result<(), String> {
    crate::skill_mcp::remove_catalog_mcp(db, key_or_name)
}
