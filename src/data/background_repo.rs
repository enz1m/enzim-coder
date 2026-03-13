use crate::data::AppDb;

pub struct BackgroundRepo;

impl BackgroundRepo {
    pub fn workspace_path_for_codex_thread(codex_thread_id: &str) -> Option<String> {
        let db = AppDb::open_default();
        db.workspace_path_for_codex_thread(codex_thread_id)
            .ok()
            .flatten()
    }

    pub fn ensure_thread_baseline_checkpoint(codex_thread_id: &str) -> Option<i64> {
        let db = AppDb::open_default();
        crate::restore::ensure_thread_baseline_checkpoint(db.as_ref(), codex_thread_id)
    }

    pub fn capture_workspace_delta_checkpoint(codex_thread_id: &str, turn_id: &str) -> Option<i64> {
        let db = AppDb::open_default();
        crate::restore::capture_workspace_delta_checkpoint(db.as_ref(), codex_thread_id, turn_id)
    }
}
