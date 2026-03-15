use crate::backend::RuntimeClient;
use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use std::rc::Rc;
use std::sync::Arc;

fn connect_runtime_for_thread(
    db: &AppDb,
    remote_thread_id: &str,
) -> Result<Arc<RuntimeClient>, String> {
    let profile = db
        .get_thread_profile_id_by_remote_thread_id(remote_thread_id)
        .ok()
        .flatten()
        .and_then(|profile_id| db.get_codex_profile(profile_id).ok().flatten());

    RuntimeClient::connect_for_profile(profile.as_ref(), "checkpoint-restore")
}

fn resolve_runtime_for_thread(
    db: &AppDb,
    manager: Option<&Rc<CodexProfileManager>>,
    remote_thread_id: &str,
) -> Result<Arc<RuntimeClient>, String> {
    if let Some(manager) = manager {
        if let Some(client) = manager.resolve_running_client_for_thread_id(remote_thread_id) {
            return Ok(client);
        }
        if let Some(client) = manager.resolve_client_for_thread_id(remote_thread_id) {
            return Ok(client);
        }
    }
    connect_runtime_for_thread(db, remote_thread_id)
}

pub(super) fn open_checkpoint_restore_popup(
    parent: Option<gtk::Window>,
    db: Rc<AppDb>,
    manager: Option<Rc<CodexProfileManager>>,
    codex_thread_id: String,
    checkpoint_id: i64,
    _turn_id: String,
    _user_prompt: Option<String>,
) {
    let runtime = resolve_runtime_for_thread(&db, manager.as_ref(), &codex_thread_id).ok();
    let active_thread_id = Rc::new(std::cell::RefCell::new(Some(codex_thread_id.clone())));
    let workspace_path = db
        .workspace_path_for_remote_thread(&codex_thread_id)
        .ok()
        .flatten()
        .or_else(|| db.get_setting("last_active_workspace_path").ok().flatten())
        .unwrap_or_default();

    crate::ui::components::restore_preview::open_restore_preview_dialog(
        parent,
        db,
        runtime,
        codex_thread_id,
        active_thread_id,
        workspace_path,
        Some(checkpoint_id),
    );
}
