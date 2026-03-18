#[derive(Clone, Debug)]
pub struct RestoreCheckpoint {
    pub id: i64,
    #[allow(dead_code)]
    pub local_thread_id: i64,
    #[allow(dead_code)]
    pub codex_thread_id: String,
    pub turn_id: String,
    pub created_at: i64,
}

impl RestoreCheckpoint {
    #[allow(dead_code)]
    pub fn remote_thread_id(&self) -> &str {
        &self.codex_thread_id
    }

    #[allow(dead_code)]
    pub fn remote_thread_id_owned(&self) -> String {
        self.codex_thread_id.clone()
    }

    #[allow(dead_code)]
    pub fn legacy_codex_thread_id(&self) -> &str {
        &self.codex_thread_id
    }
}

#[derive(Clone, Debug)]
pub enum RestoreAction {
    #[allow(dead_code)]
    Noop,
    Write,
    Delete,
    Recreate,
}

#[derive(Clone, Debug)]
pub struct RestorePreviewItem {
    pub path: String,
    pub action: RestoreAction,
    pub conflict: bool,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct RestorePreview {
    #[allow(dead_code)]
    pub target_checkpoint_id: i64,
    pub items: Vec<RestorePreviewItem>,
}

#[derive(Clone, Debug)]
pub struct RestoreApplyResult {
    pub target_checkpoint_id: i64,
    pub backup_checkpoint_id: i64,
    pub restored_count: usize,
    pub deleted_count: usize,
    pub recreated_count: usize,
    pub skipped_conflicts: usize,
}
