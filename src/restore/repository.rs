use crate::data::AppDb;
use crate::restore::types::{
    RestoreAction, RestoreApplyResult, RestoreCheckpoint, RestorePreview, RestorePreviewItem,
};
use rusqlite::params;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct ThreadContext {
    local_thread_id: i64,
    workspace_root: PathBuf,
    remote_thread_id: String,
}

#[derive(Clone, Debug)]
struct GitCheckpointState {
    before_tree: String,
    touched_paths: Vec<String>,
}

#[derive(Clone, Debug)]
struct GitDiffEntry {
    status: char,
    path: String,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn normalize_rel_path(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let trimmed = p.trim_start_matches("./").trim_matches('/').to_string();
    if trimmed.is_empty() || trimmed.starts_with(".git/") || trimmed == ".git" {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_path_for_workspace(raw_path: &str, workspace_root: &Path) -> Option<String> {
    let path = PathBuf::from(raw_path);
    if path.is_absolute() {
        if let Ok(rel) = path.strip_prefix(workspace_root) {
            return normalize_rel_path(&rel.to_string_lossy());
        }
    }

    let root = workspace_root.to_string_lossy().replace('\\', "/");
    let root_no_leading = root.trim_start_matches('/').to_string();
    let normalized = raw_path.replace('\\', "/");

    if let Some(rest) = normalized.strip_prefix(&root) {
        return normalize_rel_path(rest);
    }
    if let Some(rest) = normalized.strip_prefix(&root_no_leading) {
        return normalize_rel_path(rest);
    }

    normalize_rel_path(raw_path)
}

fn extract_local_thread_id_for_remote_thread(db: &AppDb, remote_thread_id: &str) -> Option<i64> {
    let workspaces = db.list_workspaces_with_threads().ok()?;
    for ws in workspaces {
        for thread in ws.threads {
            if thread.remote_thread_id() == Some(remote_thread_id) {
                return Some(thread.id);
            }
        }
    }
    None
}

fn workspace_of_thread(db: &AppDb, local_thread_id: i64) -> Option<PathBuf> {
    let workspaces = db.list_workspaces_with_threads().ok()?;
    for ws in workspaces {
        for thread in &ws.threads {
            if thread.id == local_thread_id {
                return Some(PathBuf::from(ws.workspace.path));
            }
        }
    }
    None
}

fn resolve_thread_context_by_remote_id(
    db: &AppDb,
    remote_thread_id: &str,
) -> Option<ThreadContext> {
    let local_thread_id = extract_local_thread_id_for_remote_thread(db, remote_thread_id)?;
    let workspace_root = workspace_of_thread(db, local_thread_id)?;
    Some(ThreadContext {
        local_thread_id,
        workspace_root,
        remote_thread_id: remote_thread_id.to_string(),
    })
}

fn restore_data_root() -> PathBuf {
    crate::data::default_app_data_dir().join("shadow_restore")
}

fn git_dir_for_thread(local_thread_id: i64) -> PathBuf {
    restore_data_root()
        .join("git_snapshots")
        .join(format!("thread-{local_thread_id}.git"))
}

fn prune_empty_ancestors(mut path: PathBuf, stop_at: &Path) {
    while path.starts_with(stop_at) && path != stop_at {
        match fs::remove_dir(&path) {
            Ok(_) => {
                if let Some(parent) = path.parent() {
                    path = parent.to_path_buf();
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn run_git_text(git_dir: &Path, workspace_root: &Path, args: &[&str]) -> Result<String, String> {
    crate::git_exec::run_git_scoped_text(git_dir, workspace_root, args)
}

fn run_git_bytes(git_dir: &Path, workspace_root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    crate::git_exec::run_git_scoped_bytes(git_dir, workspace_root, args)
}

fn ensure_git_repo(ctx: &ThreadContext) -> Result<PathBuf, String> {
    let git_dir = git_dir_for_thread(ctx.local_thread_id);
    if git_dir.join("HEAD").exists() {
        return Ok(git_dir);
    }

    if let Some(parent) = git_dir.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create git snapshot dir failed: {err}"))?;
    }

    let _ = run_git_text(&git_dir, &ctx.workspace_root, &["init", "--quiet"])?;
    let _ = run_git_text(&git_dir, &ctx.workspace_root, &["config", "gc.auto", "0"]);
    Ok(git_dir)
}

fn snapshot_workspace_tree(git_dir: &Path, workspace_root: &Path) -> Result<String, String> {
    let _ = run_git_text(git_dir, workspace_root, &["add", "-A"])?;
    let tree = run_git_text(git_dir, workspace_root, &["write-tree"])?;
    Ok(tree.trim().to_string())
}

fn parse_diff_name_status(raw: &str, workspace_root: &Path) -> Vec<GitDiffEntry> {
    let mut out = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        let status_raw = parts.next().unwrap_or_default();
        let path_raw = parts.next().unwrap_or_default();
        let status = status_raw.chars().next().unwrap_or('M');
        let Some(path) = normalize_path_for_workspace(path_raw, workspace_root) else {
            continue;
        };
        out.push(GitDiffEntry { status, path });
    }

    out
}

fn git_diff_name_status(
    git_dir: &Path,
    workspace_root: &Path,
    from_tree: &str,
    to_tree: &str,
) -> Result<Vec<GitDiffEntry>, String> {
    let raw = run_git_text(
        git_dir,
        workspace_root,
        &["diff", "--name-status", "--no-renames", from_tree, to_tree],
    )?;
    Ok(parse_diff_name_status(&raw, workspace_root))
}

fn latest_after_tree_for_thread(db: &AppDb, local_thread_id: i64) -> Option<String> {
    let conn = db.connection();
    let conn = conn.borrow();
    let mut stmt = conn
        .prepare(
            "SELECT s.after_tree
             FROM restore_git_states s
             INNER JOIN restore_checkpoints c ON c.id = s.checkpoint_id
             WHERE c.local_thread_id = ?1
             ORDER BY c.created_at DESC, c.id DESC
             LIMIT 1",
        )
        .ok()?;
    let mut rows = stmt.query(params![local_thread_id]).ok()?;
    let row = rows.next().ok()??;
    row.get::<_, String>(0).ok()
}

fn checkpoint_state(
    db: &AppDb,
    local_thread_id: i64,
    checkpoint_id: i64,
) -> rusqlite::Result<Option<GitCheckpointState>> {
    let conn = db.connection();
    let conn = conn.borrow();
    let mut stmt = conn.prepare(
        "SELECT s.before_tree, s.after_tree, s.touched_paths_json
         FROM restore_git_states s
         INNER JOIN restore_checkpoints c ON c.id = s.checkpoint_id
         WHERE s.checkpoint_id = ?1
           AND c.local_thread_id = ?2
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![checkpoint_id, local_thread_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };

    let before_tree: String = row.get(0)?;
    let _after_tree: String = row.get(1)?;
    let touched_raw: String = row.get(2)?;
    let touched_paths = serde_json::from_str::<Vec<String>>(&touched_raw).unwrap_or_default();

    Ok(Some(GitCheckpointState {
        before_tree,
        touched_paths,
    }))
}

fn insert_checkpoint(
    db: &AppDb,
    ctx: &ThreadContext,
    turn_id: &str,
    git_dir: &Path,
    before_tree: &str,
    after_tree: &str,
    touched_paths: &[String],
) -> rusqlite::Result<i64> {
    if !turn_id.starts_with("restore-") {
        let conn = db.connection();
        let conn = conn.borrow();
        let mut existing_stmt = conn.prepare(
            "SELECT id
             FROM restore_checkpoints
             WHERE local_thread_id = ?1
               AND turn_id = ?2
             ORDER BY id DESC
             LIMIT 1",
        )?;
        let mut existing_rows = existing_stmt.query(params![ctx.local_thread_id, turn_id])?;
        if let Some(row) = existing_rows.next()? {
            return row.get(0);
        }
    }

    let now = unix_now();
    let touched_json = serde_json::to_string(touched_paths).unwrap_or_else(|_| "[]".to_string());

    let conn = db.connection();
    let conn = conn.borrow_mut();
    conn.execute(
        "INSERT INTO restore_checkpoints(local_thread_id, codex_thread_id, turn_id, created_at)
         VALUES(?1, ?2, ?3, ?4)",
        params![ctx.local_thread_id, ctx.remote_thread_id, turn_id, now],
    )?;
    let checkpoint_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO restore_git_states(
            checkpoint_id, local_thread_id, codex_thread_id,
            git_dir, before_tree, after_tree, touched_paths_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            checkpoint_id,
            ctx.local_thread_id,
            ctx.remote_thread_id,
            git_dir.to_string_lossy().to_string(),
            before_tree,
            after_tree,
            touched_json,
            now
        ],
    )?;

    Ok(checkpoint_id)
}

fn extract_touched_paths_from_items(
    file_change_items: &[Value],
    workspace_root: &Path,
) -> Vec<String> {
    let mut touched = BTreeSet::new();

    for item in file_change_items {
        if let Some(changes) = item.get("changes").and_then(Value::as_array) {
            for change in changes {
                let Some(raw_path) = change.get("path").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(path) = normalize_path_for_workspace(raw_path, workspace_root) {
                    touched.insert(path);
                }
            }
        }

        if let Some(raw_path) = item.get("path").and_then(Value::as_str) {
            if let Some(path) = normalize_path_for_workspace(raw_path, workspace_root) {
                touched.insert(path);
            }
        }
    }

    touched.into_iter().collect()
}

fn action_from_status(status: char) -> RestoreAction {
    match status {
        'A' => RestoreAction::Recreate,
        'D' => RestoreAction::Delete,
        'M' | 'T' | 'C' | 'R' | 'U' | 'X' | 'B' => RestoreAction::Write,
        _ => RestoreAction::Write,
    }
}

fn reason_for_action(action: &RestoreAction) -> &'static str {
    match action {
        RestoreAction::Noop => "Already at target state",
        RestoreAction::Write => "Will restore file contents",
        RestoreAction::Delete => "Will delete file created after target",
        RestoreAction::Recreate => "Will recreate file deleted after target",
    }
}

fn read_tree_file_bytes(
    git_dir: &Path,
    workspace_root: &Path,
    tree_hash: &str,
    rel_path: &str,
) -> Result<Vec<u8>, String> {
    let spec = format!("{tree_hash}:{rel_path}");
    run_git_bytes(git_dir, workspace_root, &["cat-file", "-p", &spec])
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create parent failed: {err}"))?;
    }
    let temp = path.with_extension("restore.tmp");
    fs::write(&temp, bytes).map_err(|err| format!("write temp failed: {err}"))?;
    fs::rename(&temp, path).map_err(|err| format!("rename temp failed: {err}"))?;
    Ok(())
}

fn prune_empty_parents(workspace_root: &Path, path: &Path) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == workspace_root {
            break;
        }
        match fs::read_dir(dir) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    break;
                }
                let _ = fs::remove_dir(dir);
            }
            Err(_) => break,
        }
        current = dir.parent();
    }
}

fn create_hidden_snapshot_checkpoint(
    db: &AppDb,
    ctx: &ThreadContext,
    git_dir: &Path,
    label_prefix: &str,
    before_tree: &str,
    after_tree: &str,
    touched_paths: &[String],
) -> Result<i64, String> {
    let turn_id = format!("{label_prefix}:{}", unix_now());
    insert_checkpoint(
        db,
        ctx,
        &turn_id,
        git_dir,
        before_tree,
        after_tree,
        touched_paths,
    )
    .map_err(|err| format!("create {label_prefix} checkpoint failed: {err}"))
}

pub fn init_schema(db: &AppDb) -> rusqlite::Result<()> {
    db.connection().borrow_mut().execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS restore_checkpoints (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            local_thread_id INTEGER NOT NULL,
            codex_thread_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS restore_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            checkpoint_id INTEGER NOT NULL,
            rel_path TEXT NOT NULL,
            before_exists INTEGER NOT NULL,
            before_hash TEXT,
            before_blob_hash TEXT,
            after_exists INTEGER NOT NULL,
            after_hash TEXT,
            after_blob_hash TEXT,
            FOREIGN KEY(checkpoint_id) REFERENCES restore_checkpoints(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS restore_git_states (
            checkpoint_id INTEGER PRIMARY KEY,
            local_thread_id INTEGER NOT NULL,
            codex_thread_id TEXT NOT NULL,
            git_dir TEXT NOT NULL,
            before_tree TEXT NOT NULL,
            after_tree TEXT NOT NULL,
            touched_paths_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(checkpoint_id) REFERENCES restore_checkpoints(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_restore_checkpoints_thread_created
            ON restore_checkpoints(local_thread_id, created_at DESC, id DESC);

        CREATE INDEX IF NOT EXISTS idx_restore_entries_checkpoint
            ON restore_entries(checkpoint_id);

        CREATE INDEX IF NOT EXISTS idx_restore_git_states_thread_created
            ON restore_git_states(local_thread_id, created_at DESC, checkpoint_id DESC);
        "#,
    )?;
    Ok(())
}

pub fn clear_thread_restore_data(db: &AppDb, local_thread_id: i64) -> Result<(), String> {
    {
        let conn = db.connection();
        let conn = conn.borrow_mut();
        conn.execute(
            "DELETE FROM restore_checkpoints WHERE local_thread_id = ?1",
            params![local_thread_id],
        )
        .map_err(|err| format!("failed to clear restore checkpoints: {err}"))?;
        conn.execute(
            "DELETE FROM restore_git_states WHERE local_thread_id = ?1",
            params![local_thread_id],
        )
        .map_err(|err| format!("failed to clear restore git states: {err}"))?;
    }

    let git_dir = git_dir_for_thread(local_thread_id);
    if git_dir.exists() {
        fs::remove_dir_all(&git_dir)
            .map_err(|err| format!("failed to remove restore git snapshot dir: {err}"))?;
    }

    if let Some(parent) = git_dir.parent() {
        let root = restore_data_root();
        let snapshots_root = root.join("git_snapshots");
        prune_empty_ancestors(parent.to_path_buf(), &snapshots_root);
    }

    Ok(())
}

pub fn capture_turn_checkpoint(
    db: &AppDb,
    remote_thread_id: &str,
    turn_id: &str,
    file_change_items: &[Value],
) -> rusqlite::Result<Option<i64>> {
    let Some(ctx) = resolve_thread_context_by_remote_id(db, remote_thread_id) else {
        return Ok(None);
    };

    let touched_paths = extract_touched_paths_from_items(file_change_items, &ctx.workspace_root);

    let git_dir = match ensure_git_repo(&ctx) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("[restore] git repo init failed for checkpoint capture: {err}");
            return Ok(None);
        }
    };

    let after_tree = match snapshot_workspace_tree(&git_dir, &ctx.workspace_root) {
        Ok(tree) => tree,
        Err(err) => {
            eprintln!("[restore] snapshot capture failed for checkpoint: {err}");
            return Ok(None);
        }
    };

    let before_tree =
        latest_after_tree_for_thread(db, ctx.local_thread_id).unwrap_or_else(|| after_tree.clone());

    let checkpoint_id = insert_checkpoint(
        db,
        &ctx,
        turn_id,
        &git_dir,
        &before_tree,
        &after_tree,
        &touched_paths,
    )?;

    Ok(Some(checkpoint_id))
}

pub fn capture_workspace_delta_checkpoint(
    db: &AppDb,
    remote_thread_id: &str,
    turn_id: &str,
) -> rusqlite::Result<Option<i64>> {
    let Some(ctx) = resolve_thread_context_by_remote_id(db, remote_thread_id) else {
        return Ok(None);
    };

    let Some(before_tree) = latest_after_tree_for_thread(db, ctx.local_thread_id) else {
        return Ok(None);
    };

    let git_dir = match ensure_git_repo(&ctx) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("[restore] git repo init failed for workspace delta capture: {err}");
            return Ok(None);
        }
    };

    let after_tree = match snapshot_workspace_tree(&git_dir, &ctx.workspace_root) {
        Ok(tree) => tree,
        Err(err) => {
            eprintln!("[restore] snapshot capture failed for workspace delta: {err}");
            return Ok(None);
        }
    };

    if before_tree == after_tree {
        return Ok(None);
    }

    let diff_entries =
        match git_diff_name_status(&git_dir, &ctx.workspace_root, &before_tree, &after_tree) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("[restore] diff capture failed for workspace delta: {err}");
                return Ok(None);
            }
        };

    let mut touched = BTreeSet::new();
    for entry in diff_entries {
        touched.insert(entry.path);
    }
    let touched_paths: Vec<String> = touched.into_iter().collect();

    if touched_paths.is_empty() {
        return Ok(None);
    }

    if touched_paths.len() > 4096 {
        eprintln!(
            "[restore] skipped workspace delta checkpoint: too many changed files ({}) thread_id={}",
            touched_paths.len(),
            remote_thread_id
        );
        return Ok(None);
    }

    let checkpoint_id = insert_checkpoint(
        db,
        &ctx,
        turn_id,
        &git_dir,
        &before_tree,
        &after_tree,
        &touched_paths,
    )?;

    Ok(Some(checkpoint_id))
}

pub fn ensure_thread_baseline_checkpoint(
    db: &AppDb,
    remote_thread_id: &str,
) -> rusqlite::Result<Option<i64>> {
    let Some(ctx) = resolve_thread_context_by_remote_id(db, remote_thread_id) else {
        return Ok(None);
    };

    let conn = db.connection();
    let conn = conn.borrow();
    let existing_count: i64 = conn.query_row(
        "SELECT COUNT(1) FROM restore_checkpoints WHERE local_thread_id = ?1",
        params![ctx.local_thread_id],
        |row| row.get(0),
    )?;
    drop(conn);

    if existing_count > 0 {
        return Ok(None);
    }

    let git_dir = match ensure_git_repo(&ctx) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("[restore] git repo init failed for baseline checkpoint: {err}");
            return Ok(None);
        }
    };

    let tree = match snapshot_workspace_tree(&git_dir, &ctx.workspace_root) {
        Ok(tree) => tree,
        Err(err) => {
            eprintln!("[restore] baseline snapshot failed: {err}");
            return Ok(None);
        }
    };

    let checkpoint_id = insert_checkpoint(
        db,
        &ctx,
        &format!("restore-baseline:{}", unix_now()),
        &git_dir,
        &tree,
        &tree,
        &[],
    )?;
    eprintln!(
        "[restore] created baseline checkpoint thread_id={} checkpoint_id={} tree={}",
        remote_thread_id, checkpoint_id, tree
    );

    Ok(Some(checkpoint_id))
}

pub fn capture_preimages_for_item(
    _db: &AppDb,
    _remote_thread_id: &str,
    _item: &Value,
) -> Option<Value> {
    None
}

pub fn list_checkpoints_for_thread(
    db: &AppDb,
    remote_thread_id: &str,
) -> rusqlite::Result<Vec<RestoreCheckpoint>> {
    let Some(local_thread_id) = extract_local_thread_id_for_remote_thread(db, remote_thread_id)
    else {
        return Ok(Vec::new());
    };

    let conn = db.connection();
    let conn = conn.borrow();
    let mut stmt = conn.prepare(
        "SELECT c.id, c.local_thread_id, c.codex_thread_id, c.turn_id, c.created_at,
                s.before_tree, s.after_tree
         FROM restore_checkpoints c
         INNER JOIN restore_git_states s ON s.checkpoint_id = c.id
         WHERE c.local_thread_id = ?1
           AND c.turn_id NOT LIKE 'restore-%'
         ORDER BY c.created_at DESC, c.id DESC",
    )?;

    let rows = stmt.query_map(params![local_thread_id], |row| {
        Ok((
            RestoreCheckpoint {
                id: row.get(0)?,
                local_thread_id: row.get(1)?,
                codex_thread_id: row.get(2)?,
                turn_id: row.get(3)?,
                created_at: row.get(4)?,
            },
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (checkpoint, _before_tree, _after_tree) = row?;
        out.push(checkpoint);
    }
    Ok(out)
}

pub fn last_backup_checkpoint_for_thread(
    db: &AppDb,
    remote_thread_id: &str,
) -> rusqlite::Result<Option<i64>> {
    let Some(local_thread_id) = extract_local_thread_id_for_remote_thread(db, remote_thread_id)
    else {
        return Ok(None);
    };

    let conn = db.connection();
    let conn = conn.borrow();
    let mut stmt = conn.prepare(
        "SELECT c.id
         FROM restore_checkpoints c
         INNER JOIN restore_git_states s ON s.checkpoint_id = c.id
         WHERE c.local_thread_id = ?1
           AND c.turn_id LIKE 'restore-backup:%'
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![local_thread_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub fn preview_restore_to_checkpoint(
    db: &AppDb,
    remote_thread_id: &str,
    target_checkpoint_id: i64,
) -> rusqlite::Result<Option<RestorePreview>> {
    let Some(ctx) = resolve_thread_context_by_remote_id(db, remote_thread_id) else {
        return Ok(None);
    };
    let Some(state) = checkpoint_state(db, ctx.local_thread_id, target_checkpoint_id)? else {
        return Ok(None);
    };

    let git_dir = match ensure_git_repo(&ctx) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("[restore] git repo init failed for preview: {err}");
            return Ok(None);
        }
    };

    let current_tree = match snapshot_workspace_tree(&git_dir, &ctx.workspace_root) {
        Ok(tree) => tree,
        Err(err) => {
            eprintln!("[restore] snapshot capture failed for preview: {err}");
            return Ok(None);
        }
    };

    let diff_entries = match git_diff_name_status(
        &git_dir,
        &ctx.workspace_root,
        &current_tree,
        &state.before_tree,
    ) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("[restore] diff failed for preview: {err}");
            return Ok(None);
        }
    };

    let mut items = Vec::new();
    for entry in diff_entries {
        let action = action_from_status(entry.status);
        items.push(RestorePreviewItem {
            path: entry.path,
            action: action.clone(),
            conflict: false,
            reason: reason_for_action(&action).to_string(),
        });
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Some(RestorePreview {
        target_checkpoint_id,
        items,
    }))
}

pub fn apply_restore_to_checkpoint(
    db: &AppDb,
    remote_thread_id: &str,
    target_checkpoint_id: i64,
    selected_paths: &[String],
    forced_paths: &[String],
) -> Result<Option<RestoreApplyResult>, String> {
    let _ = forced_paths;

    let Some(ctx) = resolve_thread_context_by_remote_id(db, remote_thread_id) else {
        return Ok(None);
    };
    let Some(state) = checkpoint_state(db, ctx.local_thread_id, target_checkpoint_id)
        .map_err(|err| format!("restore state lookup failed: {err}"))?
    else {
        return Ok(None);
    };

    let git_dir = ensure_git_repo(&ctx)?;

    let current_tree = snapshot_workspace_tree(&git_dir, &ctx.workspace_root)?;
    let diff_entries = git_diff_name_status(
        &git_dir,
        &ctx.workspace_root,
        &current_tree,
        &state.before_tree,
    )?;

    let selected_path_set: Option<HashSet<&str>> = if selected_paths.is_empty() {
        None
    } else {
        Some(selected_paths.iter().map(String::as_str).collect())
    };

    let filtered_diff_entries: Vec<&GitDiffEntry> = diff_entries
        .iter()
        .filter(|entry| {
            selected_path_set
                .as_ref()
                .map(|selected| selected.contains(entry.path.as_str()))
                .unwrap_or(true)
        })
        .collect();

    let touched_paths: Vec<String> = if diff_entries.is_empty() {
        state
            .touched_paths
            .iter()
            .filter(|path| {
                selected_path_set
                    .as_ref()
                    .map(|selected| selected.contains(path.as_str()))
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    } else {
        filtered_diff_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect()
    };

    let backup_checkpoint_id = create_hidden_snapshot_checkpoint(
        db,
        &ctx,
        &git_dir,
        "restore-backup",
        &current_tree,
        &current_tree,
        &touched_paths,
    )?;

    if filtered_diff_entries.is_empty() {
        return Ok(Some(RestoreApplyResult {
            target_checkpoint_id,
            backup_checkpoint_id,
            restored_count: 0,
            deleted_count: 0,
            recreated_count: 0,
            skipped_conflicts: 0,
        }));
    }

    let mut restored_count = 0usize;
    let mut deleted_count = 0usize;
    let mut recreated_count = 0usize;

    for entry in filtered_diff_entries {
        let abs_path = ctx.workspace_root.join(&entry.path);
        match action_from_status(entry.status) {
            RestoreAction::Delete => {
                if abs_path.is_file() {
                    fs::remove_file(&abs_path)
                        .map_err(|err| format!("delete file failed for {}: {err}", entry.path))?;
                } else if abs_path.is_dir() {
                    fs::remove_dir_all(&abs_path)
                        .map_err(|err| format!("delete folder failed for {}: {err}", entry.path))?;
                }
                prune_empty_parents(&ctx.workspace_root, &abs_path);
                deleted_count += 1;
            }
            RestoreAction::Write | RestoreAction::Recreate => {
                let bytes = read_tree_file_bytes(
                    &git_dir,
                    &ctx.workspace_root,
                    &state.before_tree,
                    &entry.path,
                )?;
                atomic_write(&abs_path, &bytes)?;
                if matches!(action_from_status(entry.status), RestoreAction::Recreate) {
                    recreated_count += 1;
                } else {
                    restored_count += 1;
                }
            }
            RestoreAction::Noop => {}
        }
    }

    let after_tree = snapshot_workspace_tree(&git_dir, &ctx.workspace_root)?;
    let _ = create_hidden_snapshot_checkpoint(
        db,
        &ctx,
        &git_dir,
        "restore-postapply",
        &after_tree,
        &after_tree,
        &touched_paths,
    )?;

    Ok(Some(RestoreApplyResult {
        target_checkpoint_id,
        backup_checkpoint_id,
        restored_count,
        deleted_count,
        recreated_count,
        skipped_conflicts: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::{normalize_path_for_workspace, parse_diff_name_status};
    use std::path::Path;

    #[test]
    fn normalize_path_handles_absolute_and_relative() {
        let root = Path::new("/tmp/workspace");
        assert_eq!(
            normalize_path_for_workspace("/tmp/workspace/src/main.rs", root),
            Some("src/main.rs".to_string())
        );
        assert_eq!(
            normalize_path_for_workspace("src/lib.rs", root),
            Some("src/lib.rs".to_string())
        );
        assert_eq!(normalize_path_for_workspace(".git/config", root), None);
    }

    #[test]
    fn parse_diff_name_status_filters_and_parses_entries() {
        let root = Path::new("/tmp/workspace");
        let raw = "M\tsrc/main.rs\nD\t.git/index\nA\tREADME.md\n";
        let entries = parse_diff_name_status(raw, root);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, 'M');
        assert_eq!(entries[0].path, "src/main.rs");
        assert_eq!(entries[1].status, 'A');
        assert_eq!(entries[1].path, "README.md");
    }
}
