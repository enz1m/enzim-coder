use crate::backend::capabilities_for_backend_kind;
use crate::data::AppDb;
use crate::data::CodexProfileRecord;
pub use enzim_core::skill_mcp_support::{
    CATALOG_KEY, McpCatalogEntry, PolicyKind, ProfileAssignments, SkillCatalogEntry,
    SkillMcpCatalog, assignments_to_value, catalog_to_value, disabled_skill_markers,
    normalize_mcp_key, normalize_skill_key, parse_assignments, parse_catalog,
    profile_assignments_key, profile_skill_file_path, skill_slug_from_name,
};
use serde_json::Value;

pub fn supports_skill_assignment_for_backend(backend_kind: &str) -> bool {
    capabilities_for_backend_kind(backend_kind).supports_skill_assignment
}

pub fn write_skill_assignment_for_profile(
    profile: &CodexProfileRecord,
    slug: &str,
    content: &str,
    enabled: bool,
) -> Result<(), String> {
    if !supports_skill_assignment_for_backend(&profile.backend_kind) {
        return Err(format!(
            "{} does not support skill assignment from Enzim yet.",
            crate::backend::backend_display_name(&profile.backend_kind)
        ));
    }

    let path = profile_skill_file_path(&profile.home_dir, &profile.backend_kind, slug);
    if enabled {
        let Some(parent) = path.parent() else {
            return Err("invalid skill path".to_string());
        };
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create skill directory {}: {err}",
                parent.display()
            )
        })?;
        std::fs::write(&path, content)
            .map_err(|err| format!("Failed to write skill file {}: {err}", path.display()))?;
    } else {
        let _ = std::fs::remove_file(&path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    Ok(())
}


pub fn load_catalog(db: &AppDb) -> SkillMcpCatalog {
    let raw = db
        .get_setting(CATALOG_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    parse_catalog(&raw)
}

pub fn save_catalog(db: &AppDb, catalog: &SkillMcpCatalog) -> Result<(), String> {
    let payload = catalog_to_value(catalog);
    db.set_setting(CATALOG_KEY, &payload.to_string())
        .map_err(|err| format!("failed to save skill/mcp catalog: {err}"))
}

pub fn upsert_catalog_skill(
    db: &AppDb,
    name: &str,
    description: &str,
    content: &str,
) -> Result<SkillCatalogEntry, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("skill name is required".to_string());
    }
    let key = normalize_skill_key(name);
    if key.is_empty() {
        return Err("skill name is invalid".to_string());
    }
    let mut content = content.trim().to_string();
    if content.is_empty() {
        return Err("skill content is required".to_string());
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }

    let mut catalog = load_catalog(db);
    catalog.skills.retain(|entry| entry.key != key);
    let entry = SkillCatalogEntry {
        key: key.clone(),
        name: name.to_string(),
        description: description.trim().to_string(),
        slug: skill_slug_from_name(name),
        content,
    };
    catalog.skills.push(entry.clone());
    save_catalog(db, &catalog)?;
    Ok(entry)
}

pub fn remove_catalog_skill(db: &AppDb, key_or_name: &str) -> Result<(), String> {
    let key = normalize_skill_key(key_or_name);
    if key.is_empty() {
        return Ok(());
    }
    let mut catalog = load_catalog(db);
    catalog.skills.retain(|entry| entry.key != key);
    save_catalog(db, &catalog)
}

pub fn upsert_catalog_mcp(
    db: &AppDb,
    name: &str,
    description: &str,
    config: Value,
) -> Result<McpCatalogEntry, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("mcp server name is required".to_string());
    }
    let key = normalize_mcp_key(name);
    if key.is_empty() {
        return Err("mcp server name is invalid".to_string());
    }
    if !config.is_object() {
        return Err("mcp config must be a JSON object".to_string());
    }

    let mut catalog = load_catalog(db);
    catalog.mcps.retain(|entry| entry.key != key);
    let entry = McpCatalogEntry {
        key: key.clone(),
        name: name.to_string(),
        description: description.trim().to_string(),
        config,
    };
    catalog.mcps.push(entry.clone());
    save_catalog(db, &catalog)?;
    Ok(entry)
}

pub fn remove_catalog_mcp(db: &AppDb, key_or_name: &str) -> Result<(), String> {
    let key = normalize_mcp_key(key_or_name);
    if key.is_empty() {
        return Ok(());
    }
    let mut catalog = load_catalog(db);
    catalog.mcps.retain(|entry| entry.key != key);
    save_catalog(db, &catalog)
}


pub fn load_profile_assignments(db: &AppDb, profile_id: i64) -> ProfileAssignments {
    let raw = db
        .get_setting(&profile_assignments_key(profile_id))
        .ok()
        .flatten()
        .unwrap_or_default();
    parse_assignments(&raw)
}

pub fn save_profile_assignments(
    db: &AppDb,
    profile_id: i64,
    assignments: &ProfileAssignments,
) -> Result<(), String> {
    let payload = assignments_to_value(assignments);
    db.set_setting(&profile_assignments_key(profile_id), &payload.to_string())
        .map_err(|err| format!("failed to save profile skill/mcp assignments: {err}"))
}

pub fn set_profile_assigned(
    db: &AppDb,
    profile_id: i64,
    kind: PolicyKind,
    key_or_name: &str,
    enabled: bool,
) -> Result<(), String> {
    let key = match kind {
        PolicyKind::Skill => normalize_skill_key(key_or_name),
        PolicyKind::Mcp => normalize_mcp_key(key_or_name),
    };
    if key.is_empty() {
        return Ok(());
    }
    let mut assignments = load_profile_assignments(db, profile_id);
    let target = match kind {
        PolicyKind::Skill => &mut assignments.skills,
        PolicyKind::Mcp => &mut assignments.mcps,
    };
    if enabled {
        target.insert(key);
    } else {
        target.remove(&key);
    }
    save_profile_assignments(db, profile_id, &assignments)
}
