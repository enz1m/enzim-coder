pub use enzim_core::worktree::{
    CreatedWorktree, WorktreeMergeAction, WorktreeMergePreview, WorktreeMergePreviewItem,
    WorktreeMergeResult,
};

pub fn create_thread_worktree(
    source_workspace_path: &str,
    source_local_thread_id: i64,
    variant_index: usize,
) -> Result<CreatedWorktree, String> {
    enzim_core::worktree::create_thread_worktree(
        source_workspace_path,
        source_local_thread_id,
        variant_index,
        &crate::data::default_app_data_dir(),
    )
}

pub fn preview_worktree_merge(worktree_path: &str) -> Result<WorktreeMergePreview, String> {
    enzim_core::worktree::preview_worktree_merge(worktree_path)
}

pub fn apply_worktree_merge(
    worktree_path: &str,
    live_workspace_path: &str,
) -> Result<WorktreeMergeResult, String> {
    enzim_core::worktree::apply_worktree_merge(worktree_path, live_workspace_path)
}

pub fn stop_worktree_checkout(worktree_path: &str) -> Result<(), String> {
    enzim_core::worktree::stop_worktree_checkout(worktree_path, &crate::data::default_app_data_dir())
}
