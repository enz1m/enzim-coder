use crate::data::AppDb;

pub struct BackgroundRepo;

impl BackgroundRepo {
    pub fn workspace_path_for_remote_thread(remote_thread_id: &str) -> Option<String> {
        let db = AppDb::open_default();
        db.workspace_path_for_remote_thread(remote_thread_id)
            .ok()
            .flatten()
    }

    #[allow(dead_code)]
    pub fn workspace_path_for_codex_thread(codex_thread_id: &str) -> Option<String> {
        Self::workspace_path_for_remote_thread(codex_thread_id)
    }

    pub fn ensure_remote_thread_baseline_checkpoint(remote_thread_id: &str) -> Option<i64> {
        let db = AppDb::open_default();
        crate::restore::ensure_thread_baseline_checkpoint_by_remote_id(
            db.as_ref(),
            remote_thread_id,
        )
    }

    #[allow(dead_code)]
    pub fn ensure_thread_baseline_checkpoint(codex_thread_id: &str) -> Option<i64> {
        Self::ensure_remote_thread_baseline_checkpoint(codex_thread_id)
    }

    pub fn capture_workspace_delta_checkpoint_for_remote_thread(
        remote_thread_id: &str,
        turn_id: &str,
    ) -> Option<i64> {
        let db = AppDb::open_default();
        crate::restore::capture_workspace_delta_checkpoint_by_remote_id(
            db.as_ref(),
            remote_thread_id,
            turn_id,
        )
    }

    #[allow(dead_code)]
    pub fn capture_workspace_delta_checkpoint(codex_thread_id: &str, turn_id: &str) -> Option<i64> {
        Self::capture_workspace_delta_checkpoint_for_remote_thread(codex_thread_id, turn_id)
    }
}
