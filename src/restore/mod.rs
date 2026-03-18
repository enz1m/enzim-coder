mod repository;
pub use enzim_core::restore_types as types;

use crate::data::AppDb;
use serde_json::Value;

pub use types::RestoreApplyResult;
pub use types::{RestoreAction, RestoreCheckpoint, RestorePreview};

pub fn init(db: &AppDb) {
    if let Err(err) = repository::init_schema(db) {
        eprintln!("failed to initialize restore schema: {err}");
    }
}

pub fn capture_turn_checkpoint_by_remote_id(
    db: &AppDb,
    remote_thread_id: &str,
    turn_id: &str,
    file_change_items: &[Value],
) -> Option<i64> {
    match repository::capture_turn_checkpoint(db, remote_thread_id, turn_id, file_change_items) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("failed to capture restore checkpoint: {err}");
            None
        }
    }
}

#[allow(dead_code)]
pub fn capture_turn_checkpoint(
    db: &AppDb,
    codex_thread_id: &str,
    turn_id: &str,
    file_change_items: &[Value],
) -> Option<i64> {
    capture_turn_checkpoint_by_remote_id(db, codex_thread_id, turn_id, file_change_items)
}

pub fn capture_workspace_delta_checkpoint_by_remote_id(
    db: &AppDb,
    remote_thread_id: &str,
    turn_id: &str,
) -> Option<i64> {
    match repository::capture_workspace_delta_checkpoint(db, remote_thread_id, turn_id) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("failed to capture workspace delta checkpoint: {err}");
            None
        }
    }
}

#[allow(dead_code)]
pub fn capture_workspace_delta_checkpoint(
    db: &AppDb,
    codex_thread_id: &str,
    turn_id: &str,
) -> Option<i64> {
    capture_workspace_delta_checkpoint_by_remote_id(db, codex_thread_id, turn_id)
}

pub fn ensure_thread_baseline_checkpoint_by_remote_id(
    db: &AppDb,
    remote_thread_id: &str,
) -> Option<i64> {
    match repository::ensure_thread_baseline_checkpoint(db, remote_thread_id) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("failed to ensure restore baseline checkpoint: {err}");
            None
        }
    }
}

#[allow(dead_code)]
pub fn ensure_thread_baseline_checkpoint(db: &AppDb, codex_thread_id: &str) -> Option<i64> {
    ensure_thread_baseline_checkpoint_by_remote_id(db, codex_thread_id)
}

pub fn capture_preimages_for_item_by_remote_id(
    db: &AppDb,
    remote_thread_id: &str,
    item: &Value,
) -> Option<Value> {
    repository::capture_preimages_for_item(db, remote_thread_id, item)
}

#[allow(dead_code)]
pub fn capture_preimages_for_item(
    db: &AppDb,
    codex_thread_id: &str,
    item: &Value,
) -> Option<Value> {
    capture_preimages_for_item_by_remote_id(db, codex_thread_id, item)
}

pub fn list_checkpoints_for_remote_thread(
    db: &AppDb,
    remote_thread_id: &str,
) -> Vec<RestoreCheckpoint> {
    repository::list_checkpoints_for_thread(db, remote_thread_id).unwrap_or_default()
}

#[allow(dead_code)]
pub fn list_checkpoints_for_thread(db: &AppDb, codex_thread_id: &str) -> Vec<RestoreCheckpoint> {
    list_checkpoints_for_remote_thread(db, codex_thread_id)
}

pub fn preview_restore_to_checkpoint_by_remote_id(
    db: &AppDb,
    remote_thread_id: &str,
    target_checkpoint_id: i64,
) -> Option<RestorePreview> {
    repository::preview_restore_to_checkpoint(db, remote_thread_id, target_checkpoint_id)
        .ok()
        .flatten()
}

#[allow(dead_code)]
pub fn preview_restore_to_checkpoint(
    db: &AppDb,
    codex_thread_id: &str,
    target_checkpoint_id: i64,
) -> Option<RestorePreview> {
    preview_restore_to_checkpoint_by_remote_id(db, codex_thread_id, target_checkpoint_id)
}

pub fn last_backup_checkpoint_for_remote_thread(db: &AppDb, remote_thread_id: &str) -> Option<i64> {
    repository::last_backup_checkpoint_for_thread(db, remote_thread_id)
        .ok()
        .flatten()
}

#[allow(dead_code)]
pub fn last_backup_checkpoint_for_thread(db: &AppDb, codex_thread_id: &str) -> Option<i64> {
    last_backup_checkpoint_for_remote_thread(db, codex_thread_id)
}

pub fn apply_restore_to_checkpoint_by_remote_id(
    db: &AppDb,
    remote_thread_id: &str,
    target_checkpoint_id: i64,
    selected_paths: &[String],
    forced_paths: &[String],
) -> Result<Option<RestoreApplyResult>, String> {
    repository::apply_restore_to_checkpoint(
        db,
        remote_thread_id,
        target_checkpoint_id,
        selected_paths,
        forced_paths,
    )
}

#[allow(dead_code)]
pub fn apply_restore_to_checkpoint(
    db: &AppDb,
    codex_thread_id: &str,
    target_checkpoint_id: i64,
    selected_paths: &[String],
    forced_paths: &[String],
) -> Result<Option<RestoreApplyResult>, String> {
    apply_restore_to_checkpoint_by_remote_id(
        db,
        codex_thread_id,
        target_checkpoint_id,
        selected_paths,
        forced_paths,
    )
}

pub fn clear_thread_restore_data(db: &AppDb, local_thread_id: i64) -> Result<(), String> {
    repository::clear_thread_restore_data(db, local_thread_id)
}

#[allow(dead_code)]
pub fn clear_remote_thread_restore_data(db: &AppDb, local_thread_id: i64) -> Result<(), String> {
    clear_thread_restore_data(db, local_thread_id)
}
