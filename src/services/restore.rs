pub use crate::restore::{RestoreAction, RestoreApplyResult, RestoreCheckpoint, RestorePreview};

pub fn init(db: &crate::data::AppDb) {
    crate::restore::init(db);
}

pub fn capture_turn_checkpoint(
    db: &crate::data::AppDb,
    remote_thread_id: &str,
    turn_id: &str,
    file_change_items: &[serde_json::Value],
) -> Option<i64> {
    crate::restore::capture_turn_checkpoint_by_remote_id(
        db,
        remote_thread_id,
        turn_id,
        file_change_items,
    )
}

pub fn capture_preimages_for_item(
    db: &crate::data::AppDb,
    remote_thread_id: &str,
    item: &serde_json::Value,
) -> Option<serde_json::Value> {
    crate::restore::capture_preimages_for_item_by_remote_id(db, remote_thread_id, item)
}

pub fn list_checkpoints_for_remote_thread(
    db: &crate::data::AppDb,
    remote_thread_id: &str,
) -> Vec<RestoreCheckpoint> {
    crate::restore::list_checkpoints_for_remote_thread(db, remote_thread_id)
}

pub fn preview_restore_to_checkpoint_by_remote_id(
    db: &crate::data::AppDb,
    remote_thread_id: &str,
    target_checkpoint_id: i64,
) -> Option<RestorePreview> {
    crate::restore::preview_restore_to_checkpoint_by_remote_id(
        db,
        remote_thread_id,
        target_checkpoint_id,
    )
}

pub fn last_backup_checkpoint_for_remote_thread(
    db: &crate::data::AppDb,
    remote_thread_id: &str,
) -> Option<i64> {
    crate::restore::last_backup_checkpoint_for_remote_thread(db, remote_thread_id)
}

pub fn apply_restore_to_checkpoint_by_remote_id(
    db: &crate::data::AppDb,
    remote_thread_id: &str,
    target_checkpoint_id: i64,
    selected_paths: &[String],
    forced_paths: &[String],
) -> Result<Option<RestoreApplyResult>, String> {
    crate::restore::apply_restore_to_checkpoint_by_remote_id(
        db,
        remote_thread_id,
        target_checkpoint_id,
        selected_paths,
        forced_paths,
    )
}

pub fn clear_thread_restore_data(
    db: &crate::data::AppDb,
    local_thread_id: i64,
) -> Result<(), String> {
    crate::restore::clear_thread_restore_data(db, local_thread_id)
}
