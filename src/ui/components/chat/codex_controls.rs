use crate::codex_appserver::{CodexAppServer, ModelInfo};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

fn model_options(codex: Option<&Arc<CodexAppServer>>) -> Vec<ModelInfo> {
    codex
        .and_then(|client| client.model_list(false, 50).ok())
        .unwrap_or_default()
}

pub(super) fn build_model_selector(
    codex: Option<&Arc<CodexAppServer>>,
    initial_model_id: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let models = model_options(codex);

    if models.is_empty() {
        let selected_model = initial_model_id.unwrap_or_else(|| "gpt-5.3-codex".to_string());
        let selected = Rc::new(RefCell::new(selected_model.clone()));
        let options = vec![("GPT-5.3 Codex".to_string(), "gpt-5.3-codex".to_string())];
        let (selector, set_selected) =
            super::create_selector_menu("GPT-5.3 Codex", &options, selected.clone(), on_change);
        set_selected(&selected_model);
        return (selector, selected, set_selected);
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
    let selected = Rc::new(RefCell::new(selected_model_id.clone()));
    let options: Vec<(String, String)> =
        models.into_iter().map(|m| (m.display_name, m.id)).collect();
    let (selector, set_selected) =
        super::create_selector_menu(&default_model_name, &options, selected.clone(), on_change);
    set_selected(&selected_model_id);
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
    let (selector, set_selected) =
        super::create_selector_menu("Agent", &options, selected.clone(), on_change);
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
    let (selector, set_selected) =
        super::create_selector_menu("Full access", &options, selected.clone(), on_change);
    set_selected(&selected_access_mode);
    (selector, selected, set_selected)
}

pub(super) fn build_effort_selector(
    initial_effort: Option<String>,
    on_change: Option<Rc<dyn Fn(String)>>,
) -> (gtk::Button, Rc<RefCell<String>>, Rc<dyn Fn(&str)>) {
    let selected_effort = initial_effort.unwrap_or_else(|| "medium".to_string());
    let selected = Rc::new(RefCell::new(selected_effort.clone()));
    let options = vec![
        ("Low".to_string(), "low".to_string()),
        ("Medium".to_string(), "medium".to_string()),
        ("High".to_string(), "high".to_string()),
    ];
    let (selector, set_selected) =
        super::create_selector_menu("Medium", &options, selected.clone(), on_change);
    set_selected(&selected_effort);
    (selector, selected, set_selected)
}

pub(super) fn sandbox_policy_for(access_mode: &str) -> Option<Value> {
    match access_mode {
        "dangerFullAccess" => Some(json!({ "type": "dangerFullAccess" })),
        "workspaceWrite" => Some(json!({ "type": "workspaceWrite", "networkAccess": true })),
        "readOnly" => Some(json!({ "type": "readOnly" })),
        _ => None,
    }
}
