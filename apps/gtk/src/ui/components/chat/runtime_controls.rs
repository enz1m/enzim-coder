use crate::services::app::runtime::RuntimeClient;
use crate::services::app::runtime::ModelInfo;
use crate::services::app::chat::AppDb;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

fn model_cache() -> &'static Mutex<HashMap<String, Vec<ModelInfo>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<ModelInfo>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn model_cache_version_counter() -> &'static AtomicU64 {
    static VERSION: OnceLock<AtomicU64> = OnceLock::new();
    VERSION.get_or_init(|| AtomicU64::new(1))
}

fn hidden_model_cache() -> &'static Mutex<HashMap<i64, HashSet<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<i64, HashSet<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn model_cache_refresh_inflight() -> &'static Mutex<HashSet<String>> {
    static CACHE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn try_begin_model_cache_refresh(key: &str) -> bool {
    model_cache_refresh_inflight()
        .lock()
        .ok()
        .map(|mut inflight| inflight.insert(key.to_string()))
        .unwrap_or(false)
}

fn finish_model_cache_refresh(key: &str) {
    if let Ok(mut inflight) = model_cache_refresh_inflight().lock() {
        inflight.remove(key);
    }
}

fn fetch_model_options(client: &Arc<RuntimeClient>) -> Vec<ModelInfo> {
    let is_opencode = client.backend_kind().eq_ignore_ascii_case("opencode");
    let limit = if is_opencode { 500 } else { 50 };
    let Some(mut models) = client.model_list(false, limit).ok() else {
        return Vec::new();
    };
    if is_opencode {
        let allowed_providers = client
            .account_provider_list()
            .ok()
            .unwrap_or_default()
            .into_iter()
            .filter(|provider| provider.connected || provider.has_saved_auth)
            .map(|provider| provider.provider_id)
            .collect::<HashSet<_>>();
        if !allowed_providers.is_empty() {
            models.retain(|model| {
                model
                    .id
                    .split_once(':')
                    .map(|(provider_id, _)| allowed_providers.contains(provider_id))
                    .unwrap_or(false)
            });
        }
    }
    models
}

fn visible_models_for_client(
    client: &Arc<RuntimeClient>,
    models: Vec<ModelInfo>,
) -> Vec<ModelInfo> {
    let Some(profile_id) = client.profile_id() else {
        return models;
    };
    let hidden = if let Ok(cache) = hidden_model_cache().lock() {
        cache.get(&profile_id).cloned().unwrap_or_default()
    } else {
        HashSet::new()
    };
    if hidden.is_empty() {
        return models;
    }
    models
        .into_iter()
        .filter(|model| !hidden.contains(&model.id))
        .collect()
}

fn bump_model_options_cache_version() {
    model_cache_version_counter().fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn invalidate_model_options_cache_for_backend(backend_kind: &str) {
    let prefix = if backend_kind.eq_ignore_ascii_case("opencode") {
        "opencode:"
    } else {
        "codex:"
    };
    if let Ok(mut cache) = model_cache().lock() {
        cache.retain(|key, _| !key.starts_with(prefix));
    }
    bump_model_options_cache_version();
}

pub(crate) fn model_options_cache_version() -> u64 {
    model_cache_version_counter().load(Ordering::Relaxed)
}

pub(crate) fn preload_opencode_hidden_model_cache(db: &AppDb) {
    let mut next = HashMap::new();
    for profile in db.list_codex_profiles().unwrap_or_default() {
        if !profile.backend_kind.eq_ignore_ascii_case("opencode") {
            continue;
        }
        let hidden = db.opencode_hidden_models(profile.id).unwrap_or_default();
        next.insert(profile.id, hidden);
    }
    if let Ok(mut cache) = hidden_model_cache().lock() {
        *cache = next;
    }
}

pub(crate) fn hidden_opencode_model_ids(profile_id: i64) -> HashSet<String> {
    if let Ok(cache) = hidden_model_cache().lock() {
        cache.get(&profile_id).cloned().unwrap_or_default()
    } else {
        HashSet::new()
    }
}

pub(crate) fn set_opencode_model_hidden(
    db: &AppDb,
    profile_id: i64,
    model_id: &str,
    hidden: bool,
) -> Result<(), String> {
    let hidden_models = db
        .set_opencode_model_hidden(profile_id, model_id, hidden)
        .map_err(|err| err.to_string())?;
    if let Ok(mut cache) = hidden_model_cache().lock() {
        cache.insert(profile_id, hidden_models);
    }
    bump_model_options_cache_version();
    Ok(())
}

pub(crate) fn refresh_model_options_cache(
    runtime_client: Option<&Arc<RuntimeClient>>,
) -> Vec<ModelInfo> {
    let Some(client) = runtime_client else {
        return Vec::new();
    };
    let key = client.model_cache_key();
    let models = fetch_model_options(client);
    if let Ok(mut cache) = model_cache().lock() {
        cache.insert(key, models.clone());
    }
    bump_model_options_cache_version();
    models
}

pub(crate) fn refresh_model_options_cache_async(runtime_client: Option<Arc<RuntimeClient>>) {
    let Some(client) = runtime_client else {
        return;
    };
    let key = client.model_cache_key();
    if !try_begin_model_cache_refresh(&key) {
        return;
    }
    thread::spawn(move || {
        let models = fetch_model_options(&client);
        if let Ok(mut cache) = model_cache().lock() {
            cache.insert(key.clone(), models);
        }
        finish_model_cache_refresh(&key);
        bump_model_options_cache_version();
    });
}

pub(super) fn model_options(runtime_client: Option<&Arc<RuntimeClient>>) -> Vec<ModelInfo> {
    let Some(client) = runtime_client else {
        return Vec::new();
    };
    let key = client.model_cache_key();
    if let Ok(cache) = model_cache().lock() {
        if let Some(models) = cache.get(&key) {
            return visible_models_for_client(client, models.clone());
        }
    }
    refresh_model_options_cache_async(Some(client.clone()));
    Vec::new()
}

pub(crate) fn model_options_unfiltered(
    runtime_client: Option<&Arc<RuntimeClient>>,
) -> Vec<ModelInfo> {
    let Some(client) = runtime_client else {
        return Vec::new();
    };
    let key = client.model_cache_key();
    if let Ok(cache) = model_cache().lock() {
        if let Some(models) = cache.get(&key) {
            return models.clone();
        }
    }
    refresh_model_options_cache_async(Some(client.clone()));
    Vec::new()
}

fn variant_rank(value: &str) -> usize {
    match value {
        "" => 0,
        "none" => 1,
        "minimal" => 2,
        "low" => 3,
        "medium" => 4,
        "high" => 5,
        "max" => 6,
        "xhigh" => 7,
        _ => 100,
    }
}

fn format_variant_label(value: &str) -> String {
    match value {
        "" => "Default".to_string(),
        "xhigh" => "Extra High".to_string(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn compact_opencode_model_label(display_name: &str) -> String {
    let model_name = display_name
        .split_once(" / ")
        .map(|(_, model)| model)
        .unwrap_or(display_name)
        .trim();
    truncate_label(model_name, 15)
}

pub(super) fn reasoning_effort_options_from_models(
    models: &[ModelInfo],
    model_id: &str,
) -> (Vec<(String, String)>, Option<String>) {
    let Some(model) = models.iter().find(|model| model.id == model_id) else {
        return (Vec::new(), None);
    };
    if model.reasoning_efforts.is_empty() {
        return (Vec::new(), model.default_reasoning_effort.clone());
    }
    let mut efforts = model.reasoning_efforts.clone();
    efforts.sort_by(|left, right| {
        variant_rank(left)
            .cmp(&variant_rank(right))
            .then_with(|| left.cmp(right))
    });
    let options = efforts
        .into_iter()
        .map(|effort| (format_variant_label(&effort), effort))
        .collect::<Vec<_>>();
    (options, model.default_reasoning_effort.clone())
}

pub(super) fn opencode_variant_options_from_models(
    models: &[ModelInfo],
    model_id: &str,
) -> Vec<(String, String)> {
    let Some(model) = models.iter().find(|model| model.id == model_id).cloned() else {
        return Vec::new();
    };
    if model.variants.is_empty() {
        return Vec::new();
    }
    let mut variants = model.variants;
    variants.sort_by(|left, right| {
        variant_rank(left)
            .cmp(&variant_rank(right))
            .then_with(|| left.cmp(right))
    });
    let mut options = Vec::with_capacity(variants.len() + 1);
    options.push(("Default".to_string(), String::new()));
    options.extend(
        variants
            .into_iter()
            .map(|variant| (format_variant_label(&variant), variant)),
    );
    options
}

pub(super) fn build_model_selector_with_state(
    runtime_client: Option<&Arc<RuntimeClient>>,
    selected: Rc<RefCell<String>>,
    initial_model_id: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<dyn Fn(&str)>) {
    let models = model_options(runtime_client);
    let is_opencode =
        runtime_client.is_some_and(|client| client.backend_kind().eq_ignore_ascii_case("opencode"));
    let selected_label = runtime_client
        .filter(|client| client.backend_kind().eq_ignore_ascii_case("opencode"))
        .map(|_| {
            Rc::new(|display_name: &str, _value: &str| compact_opencode_model_label(display_name))
                as Rc<dyn Fn(&str, &str) -> String>
        });

    if models.is_empty() {
        let (fallback_label, fallback_model) = if is_opencode {
            ("No models".to_string(), String::new())
        } else {
            ("GPT-5.3 Codex".to_string(), "gpt-5.3-codex".to_string())
        };
        let selected_model = if is_opencode {
            String::new()
        } else {
            initial_model_id.unwrap_or_else(|| fallback_model.clone())
        };
        selected.replace(selected_model.clone());
        let options = vec![(fallback_label.clone(), fallback_model.clone())];
        let (selector, set_selected) = super::create_selector_menu(
            &fallback_label,
            &options,
            selected.clone(),
            selected_label,
            on_change,
            gtk::PositionType::Top,
        );
        set_selected(&selected_model);
        return (selector, set_selected);
    }

    let default_idx = models.iter().position(|m| m.is_default).unwrap_or(0);
    let default_model_id = models[default_idx].id.clone();
    let default_model_name = models[default_idx].display_name.clone();
    let selected_model_id = initial_model_id
        .and_then(|candidate| {
            models
                .iter()
                .find(|m| m.id == candidate)
                .map(|m| m.id.clone())
        })
        .unwrap_or(default_model_id);
    selected.replace(selected_model_id.clone());
    let options: Vec<(String, String)> =
        models.into_iter().map(|m| (m.display_name, m.id)).collect();
    let (selector, set_selected) = if is_opencode {
        super::create_grouped_selector_menu(
            &default_model_name,
            &options,
            selected.clone(),
            selected_label,
            on_change,
            gtk::PositionType::Top,
        )
    } else {
        super::create_selector_menu(
            &default_model_name,
            &options,
            selected.clone(),
            selected_label,
            on_change,
            gtk::PositionType::Top,
        )
    };
    set_selected(&selected_model_id);
    (selector, set_selected)
}

pub(super) fn build_model_selector(
    runtime_client: Option<&Arc<RuntimeClient>>,
    initial_model_id: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let selected = Rc::new(RefCell::new(String::new()));
    let (selector, set_selected) = build_model_selector_with_state(
        runtime_client,
        selected.clone(),
        initial_model_id,
        on_change,
    );
    (selector, selected, set_selected)
}

pub(super) fn build_mode_selector(
    initial_mode: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let selected_mode = initial_mode.unwrap_or_else(|| "default".to_string());
    let selected = Rc::new(RefCell::new(selected_mode.clone()));
    let options = vec![
        ("Agent".to_string(), "default".to_string()),
        ("Plan".to_string(), "plan".to_string()),
    ];
    let (selector, set_selected) = super::create_selector_menu(
        "Agent",
        &options,
        selected.clone(),
        None,
        on_change,
        gtk::PositionType::Bottom,
    );
    set_selected(&selected_mode);
    (selector, selected, set_selected)
}

pub(super) fn build_access_selector(
    initial_access_mode: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let selected_access_mode =
        initial_access_mode.unwrap_or_else(|| "dangerFullAccess".to_string());
    let selected = Rc::new(RefCell::new(selected_access_mode.clone()));
    let options = vec![
        ("Full access".to_string(), "dangerFullAccess".to_string()),
        ("Workspace write".to_string(), "workspaceWrite".to_string()),
        ("Read only".to_string(), "readOnly".to_string()),
    ];
    let (selector, set_selected) = super::create_selector_menu(
        "Full access",
        &options,
        selected.clone(),
        None,
        on_change,
        gtk::PositionType::Bottom,
    );
    set_selected(&selected_access_mode);
    (selector, selected, set_selected)
}

pub(super) fn build_opencode_command_selector(
    initial_command_mode: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let selected_command_mode = initial_command_mode.unwrap_or_else(|| "allowAll".to_string());
    let selected = Rc::new(RefCell::new(selected_command_mode.clone()));
    let options = vec![
        ("Allow all".to_string(), "allowAll".to_string()),
        ("Ask".to_string(), "ask".to_string()),
    ];
    let (selector, set_selected) = super::create_selector_menu(
        "Allow all",
        &options,
        selected.clone(),
        None,
        on_change,
        gtk::PositionType::Bottom,
    );
    set_selected(&selected_command_mode);
    (selector, selected, set_selected)
}

pub(super) fn build_effort_selector(
    options: &[(String, String)],
    initial_effort: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let default_value = options
        .first()
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| "medium".to_string());
    let default_label = options
        .first()
        .map(|(label, _)| label.as_str())
        .unwrap_or("Medium");
    let selected_effort = initial_effort.unwrap_or_else(|| default_value.clone());
    let selected = Rc::new(RefCell::new(selected_effort.clone()));
    let (selector, set_selected) = super::create_selector_menu(
        default_label,
        options,
        selected.clone(),
        None,
        on_change,
        gtk::PositionType::Bottom,
    );
    set_selected(&selected_effort);
    (selector, selected, set_selected)
}

pub(super) fn build_variant_selector(
    options: &[(String, String)],
    initial_variant: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let default_value = options
        .first()
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let default_label = options
        .first()
        .map(|(label, _)| label.as_str())
        .unwrap_or("Default");
    let selected_variant = initial_variant.unwrap_or_else(|| default_value.clone());
    let selected = Rc::new(RefCell::new(selected_variant.clone()));
    let (selector, set_selected) = super::create_selector_menu(
        default_label,
        options,
        selected.clone(),
        None,
        on_change,
        gtk::PositionType::Bottom,
    );
    set_selected(&selected_variant);
    (selector, selected, set_selected)
}

pub(crate) fn sandbox_policy_for(access_mode: &str) -> Option<Value> {
    match access_mode {
        "dangerFullAccess" => Some(json!({ "type": "dangerFullAccess" })),
        "workspaceWrite" => Some(json!({ "type": "workspaceWrite", "networkAccess": true })),
        "readOnly" => Some(json!({ "type": "readOnly" })),
        _ => None,
    }
}

pub(crate) fn opencode_session_policy_for(access_mode: &str, command_mode: &str) -> Value {
    json!({
        "type": access_mode,
        "opencode": {
            "access_mode": access_mode,
            "command_mode": command_mode,
        }
    })
}
