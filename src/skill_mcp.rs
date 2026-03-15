use crate::backend::capabilities_for_backend_kind;
use crate::data::AppDb;
use crate::data::CodexProfileRecord;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::path::PathBuf;

const CATALOG_KEY: &str = "skill_mcp_catalog_v2";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyKind {
    Skill,
    Mcp,
}

#[derive(Clone, Debug, Default)]
pub struct SkillCatalogEntry {
    pub key: String,
    pub name: String,
    pub description: String,
    pub slug: String,
    pub content: String,
}

#[derive(Clone, Debug, Default)]
pub struct McpCatalogEntry {
    pub key: String,
    pub name: String,
    pub description: String,
    pub config: Value,
}

#[derive(Clone, Debug, Default)]
pub struct SkillMcpCatalog {
    pub skills: Vec<SkillCatalogEntry>,
    pub mcps: Vec<McpCatalogEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileAssignments {
    pub skills: HashSet<String>,
    pub mcps: HashSet<String>,
}

fn profile_assignments_key(profile_id: i64) -> String {
    format!("skill_mcp_profile_assignments_v2::{profile_id}")
}

fn normalize_name(raw: &str) -> String {
    let mut out = String::new();
    let mut previous_was_sep = false;
    for ch in raw.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            previous_was_sep = false;
            continue;
        }
        let is_separator = matches!(lower, '-' | '_' | '.' | '/' | ' ' | '\t' | '\n' | '\r');
        if is_separator && !out.is_empty() && !previous_was_sep {
            out.push('-');
            previous_was_sep = true;
        }
    }
    out.trim_matches('-').to_string()
}

pub fn normalize_skill_key(raw: &str) -> String {
    normalize_name(raw)
}

pub fn normalize_mcp_key(raw: &str) -> String {
    normalize_name(raw)
}

pub fn skill_slug_from_name(raw: &str) -> String {
    let slug = normalize_skill_key(raw);
    if slug.is_empty() {
        "custom-skill".to_string()
    } else {
        slug
    }
}

fn os_user_home_dir() -> Option<PathBuf> {
    unsafe {
        let uid = libc::geteuid();
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let buf_len = libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX);
        let buf_len = if buf_len <= 0 {
            16_384
        } else {
            buf_len as usize
        };
        let mut buf = vec![0u8; buf_len];
        let status = libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        );
        if status != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let home = CStr::from_ptr(pwd.pw_dir)
            .to_string_lossy()
            .trim()
            .to_string();
        if home.is_empty() {
            None
        } else {
            Some(PathBuf::from(home))
        }
    }
}

pub fn profile_skill_file_path(profile_home: &str, backend_kind: &str, slug: &str) -> PathBuf {
    let base_dir = if backend_kind.eq_ignore_ascii_case("opencode") {
        os_user_home_dir().unwrap_or_else(|| PathBuf::from(profile_home))
    } else {
        PathBuf::from(profile_home)
    };
    let runtime_dir = if backend_kind.eq_ignore_ascii_case("opencode") {
        ".opencode"
    } else {
        ".codex"
    };
    base_dir
        .join(runtime_dir)
        .join("skills")
        .join(skill_slug_from_name(slug))
        .join("SKILL.md")
}

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

fn parse_catalog(raw: &str) -> SkillMcpCatalog {
    let parsed: Value = serde_json::from_str(raw).unwrap_or_else(|_| Value::Null);
    let Some(obj) = parsed.as_object() else {
        return SkillMcpCatalog::default();
    };

    let mut skills = Vec::<SkillCatalogEntry>::new();
    let mut seen_skills = HashSet::<String>::new();
    for item in obj
        .get("skills")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(entry) = item.as_object() else {
            continue;
        };
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let key = entry
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(normalize_skill_key)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| normalize_skill_key(&name));
        if key.is_empty() || seen_skills.contains(&key) {
            continue;
        }
        let content = entry
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if content.trim().is_empty() {
            continue;
        }
        let description = entry
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        let slug = entry
            .get("slug")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| skill_slug_from_name(&name));

        seen_skills.insert(key.clone());
        skills.push(SkillCatalogEntry {
            key,
            name,
            description,
            slug,
            content,
        });
    }

    let mut mcps = Vec::<McpCatalogEntry>::new();
    let mut seen_mcps = HashSet::<String>::new();
    for item in obj
        .get("mcps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(entry) = item.as_object() else {
            continue;
        };
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let key = entry
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(normalize_mcp_key)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| normalize_mcp_key(&name));
        if key.is_empty() || seen_mcps.contains(&key) {
            continue;
        }

        let config = entry
            .get("config")
            .cloned()
            .filter(|value| value.is_object())
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !config.is_object() {
            continue;
        }

        let description = entry
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();

        seen_mcps.insert(key.clone());
        mcps.push(McpCatalogEntry {
            key,
            name,
            description,
            config,
        });
    }

    skills.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    mcps.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });

    SkillMcpCatalog { skills, mcps }
}

fn catalog_to_value(catalog: &SkillMcpCatalog) -> Value {
    let mut skills = catalog.skills.clone();
    skills.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    let mut mcps = catalog.mcps.clone();
    mcps.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });

    json!({
        "skills": skills.into_iter().map(|entry| {
            json!({
                "key": entry.key,
                "name": entry.name,
                "description": entry.description,
                "slug": entry.slug,
                "content": entry.content,
            })
        }).collect::<Vec<_>>(),
        "mcps": mcps.into_iter().map(|entry| {
            json!({
                "key": entry.key,
                "name": entry.name,
                "description": entry.description,
                "config": entry.config,
            })
        }).collect::<Vec<_>>(),
    })
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

fn parse_assignments(raw: &str) -> ProfileAssignments {
    let parsed: Value = serde_json::from_str(raw).unwrap_or_else(|_| Value::Null);
    let Some(obj) = parsed.as_object() else {
        return ProfileAssignments::default();
    };

    let to_set = |value: Option<&Value>, normalize: fn(&str) -> String| {
        value
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .map(|item| normalize(&item))
            .filter(|item| !item.is_empty())
            .collect::<HashSet<_>>()
    };

    ProfileAssignments {
        skills: to_set(obj.get("skills"), normalize_skill_key),
        mcps: to_set(obj.get("mcps"), normalize_mcp_key),
    }
}

fn assignments_to_value(assignments: &ProfileAssignments) -> Value {
    let mut skills = assignments.skills.iter().cloned().collect::<Vec<_>>();
    skills.sort();
    let mut mcps = assignments.mcps.iter().cloned().collect::<Vec<_>>();
    mcps.sort();
    json!({
        "skills": skills,
        "mcps": mcps,
    })
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

pub fn disabled_skill_markers(
    text: &str,
    catalog: &SkillMcpCatalog,
    assignments: &ProfileAssignments,
) -> Vec<String> {
    let mut skill_names = HashMap::<String, String>::new();
    for skill in &catalog.skills {
        skill_names.insert(skill.key.clone(), skill.name.clone());
    }

    let mut blocked = Vec::new();
    for token in text.split_whitespace() {
        let Some(stripped) = token.strip_prefix('$') else {
            continue;
        };
        let marker = stripped
            .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
        if marker.is_empty() {
            continue;
        }
        let normalized = normalize_skill_key(marker);
        if normalized.is_empty() {
            continue;
        }
        if !skill_names.contains_key(&normalized) {
            continue;
        }
        if !assignments.skills.contains(&normalized) {
            blocked.push(marker.to_string());
        }
    }
    blocked.sort();
    blocked.dedup();
    blocked
}
