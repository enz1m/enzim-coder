pub use crate::worktree::{
    CreatedWorktree, WorktreeMergeAction, WorktreeMergePreview, WorktreeMergePreviewItem,
    WorktreeMergeResult,
};

pub fn create_thread_worktree(
    source_workspace_path: &str,
    source_local_thread_id: i64,
    variant_index: usize,
) -> Result<CreatedWorktree, String> {
    crate::worktree::create_thread_worktree(
        source_workspace_path,
        source_local_thread_id,
        variant_index,
    )
}

pub fn preview_worktree_merge(worktree_path: &str) -> Result<WorktreeMergePreview, String> {
    crate::worktree::preview_worktree_merge(worktree_path)
}

pub fn apply_worktree_merge(
    worktree_path: &str,
    live_workspace_path: &str,
) -> Result<WorktreeMergeResult, String> {
    crate::worktree::apply_worktree_merge(worktree_path, live_workspace_path)
}

pub fn stop_worktree_checkout(worktree_path: &str) -> Result<(), String> {
    crate::worktree::stop_worktree_checkout(worktree_path)
}
