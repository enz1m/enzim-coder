use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::HashSet, io};

#[derive(Clone, Debug)]
pub struct CreatedWorktree {
    pub path: String,
    pub branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeMergeAction {
    Write,
    Delete,
    Rename,
}

#[derive(Clone, Debug)]
pub struct WorktreeMergePreviewItem {
    pub path: String,
    pub action: WorktreeMergeAction,
    pub from_path: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorktreeMergePreview {
    pub items: Vec<WorktreeMergePreviewItem>,
}

#[derive(Clone, Debug)]
pub struct WorktreeMergeResult {
    pub merged_count: usize,
    pub deleted_count: usize,
    pub renamed_count: usize,
}

fn run_git(args: &[&str], cwd: &Path) -> Result<String, String> {
    crate::git_exec::run_git_text(cwd, args)
}

fn run_git_raw(args: &[&str], cwd: &Path) -> Result<Vec<u8>, String> {
    crate::git_exec::run_git_bytes(cwd, args)
}

fn run_git_with_input(args: &[&str], cwd: &Path, input: &[u8]) -> Result<(), String> {
    crate::git_exec::run_git_with_input(cwd, args, input)
}

fn prune_empty_worktree_parents(path: &Path) {
    let stop = crate::data::default_app_data_dir().join("worktrees");
    let mut current = path.parent().map(Path::to_path_buf);
    while let Some(dir) = current {
        if !dir.starts_with(&stop) || dir == stop {
            break;
        }
        match fs::remove_dir(&dir) {
            Ok(_) => {
                current = dir.parent().map(Path::to_path_buf);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                current = dir.parent().map(Path::to_path_buf);
            }
            Err(err) if err.kind() == io::ErrorKind::DirectoryNotEmpty => {
                break;
            }
            Err(_) => break,
        }
    }
}

fn seed_worktree_from_source(
    source_root: &Path,
    worktree_root: &Path,
    branch: &str,
) -> Result<(), String> {
    let tracked_patch = run_git_raw(&["diff", "--binary", "HEAD"], source_root)?;
    let mut seeded_any = false;
    if !tracked_patch.is_empty() {
        run_git_with_input(
            &["apply", "--whitespace=nowarn", "-"],
            worktree_root,
            &tracked_patch,
        )?;
        seeded_any = true;
    }

    let untracked_raw = run_git_raw(
        &["ls-files", "--others", "--exclude-standard", "-z"],
        source_root,
    )?;
    for rel in split_nul_fields(&untracked_raw) {
        if !is_safe_relative_path(&rel) {
            continue;
        }
        let src = source_root.join(&rel);
        let dst = worktree_root.join(&rel);
        if src.is_file() {
            copy_file(&src, &dst)?;
            seeded_any = true;
        }
    }

    if !seeded_any {
        return Ok(());
    }

    let _ = run_git(&["add", "-A"], worktree_root)?;
    run_git(
        &[
            "-c",
            "user.name=EnzimCoder",
            "-c",
            "user.email=enzimcoder@local",
            "commit",
            "-m",
            "enzimcoder: worktree baseline snapshot",
        ],
        worktree_root,
    )
    .map_err(|err| format!("failed to commit worktree baseline on branch {branch}: {err}"))?;
    Ok(())
}

fn sanitize_component(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn create_thread_worktree(
    source_workspace_path: &str,
    source_local_thread_id: i64,
    variant_index: usize,
) -> Result<CreatedWorktree, String> {
    let source_path = PathBuf::from(source_workspace_path);
    if !source_path.exists() {
        return Err(format!(
            "workspace path does not exist: {}",
            source_workspace_path
        ));
    }
    let git_root_raw = run_git(&["rev-parse", "--show-toplevel"], &source_path)?;
    let git_root = PathBuf::from(git_root_raw.trim());
    let base_commit = run_git(&["rev-parse", "HEAD"], &git_root)?;
    let repo_name = git_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_component)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_string());

    let base_dir = crate::data::default_app_data_dir()
        .join("worktrees")
        .join(repo_name)
        .join(format!("thread-{}", source_local_thread_id));
    fs::create_dir_all(&base_dir).map_err(|err| {
        format!(
            "failed to create worktree base directory {}: {}",
            base_dir.display(),
            err
        )
    })?;

    for attempt in 0..8usize {
        let stamp = now_unix();
        let suffix = if attempt == 0 {
            format!("v{}-{}", variant_index, stamp)
        } else {
            format!("v{}-{}-{}", variant_index, stamp, attempt)
        };
        let branch = format!("enzimcoder/wt-{}-{}", source_local_thread_id, suffix);
        let path = base_dir.join(&suffix);
        let path_str = path.to_string_lossy().to_string();
        let result = run_git(
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &path_str,
                base_commit.as_str(),
            ],
            &git_root,
        );
        if result.is_ok() {
            let seed_result = seed_worktree_from_source(&source_path, &path, &branch);
            if let Err(err) = seed_result {
                let _ = run_git(&["worktree", "remove", "--force", &path_str], &git_root);
                return Err(err);
            }
            return Ok(CreatedWorktree {
                path: path_str,
                branch,
            });
        }
    }

    Err("failed to create git worktree after multiple attempts".to_string())
}

fn split_nul_fields(raw: &[u8]) -> Vec<String> {
    raw.split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect()
}

fn is_safe_relative_path(path: &str) -> bool {
    let p = Path::new(path);
    if p.is_absolute() {
        return false;
    }
    !p.components().any(|comp| {
        matches!(
            comp,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create directory {}: {err}", parent.display()))?;
    }
    fs::copy(src, dst).map_err(|err| {
        format!(
            "failed to copy {} -> {}: {err}",
            src.display(),
            dst.display()
        )
    })?;
    Ok(())
}

pub fn preview_worktree_merge(worktree_path: &str) -> Result<WorktreeMergePreview, String> {
    let worktree_root = PathBuf::from(worktree_path);
    if !worktree_root.exists() {
        return Err(format!("worktree path does not exist: {worktree_path}"));
    }

    let mut items = Vec::<WorktreeMergePreviewItem>::new();
    let mut seen = HashSet::<String>::new();

    let tracked_raw = run_git_raw(&["diff", "--name-status", "-z", "HEAD"], &worktree_root)?;
    let tracked_fields = split_nul_fields(&tracked_raw);
    let mut idx = 0usize;
    while idx < tracked_fields.len() {
        let status = tracked_fields[idx].trim().to_string();
        idx += 1;
        if status.is_empty() {
            continue;
        }
        let code = status.chars().next().unwrap_or('M');
        if matches!(code, 'R' | 'C') {
            if idx + 1 >= tracked_fields.len() {
                break;
            }
            let from_path = tracked_fields[idx].clone();
            let to_path = tracked_fields[idx + 1].clone();
            idx += 2;
            if !is_safe_relative_path(&to_path) {
                continue;
            }
            if seen.insert(to_path.clone()) {
                items.push(WorktreeMergePreviewItem {
                    path: to_path,
                    action: if code == 'R' {
                        WorktreeMergeAction::Rename
                    } else {
                        WorktreeMergeAction::Write
                    },
                    from_path: if code == 'R' { Some(from_path) } else { None },
                });
            }
            continue;
        }

        if idx >= tracked_fields.len() {
            break;
        }
        let path = tracked_fields[idx].clone();
        idx += 1;
        if !is_safe_relative_path(&path) {
            continue;
        }
        let action = match code {
            'D' => WorktreeMergeAction::Delete,
            _ => WorktreeMergeAction::Write,
        };
        if seen.insert(path.clone()) {
            items.push(WorktreeMergePreviewItem {
                path,
                action,
                from_path: None,
            });
        }
    }

    let untracked_raw = run_git_raw(
        &["ls-files", "--others", "--exclude-standard", "-z"],
        &worktree_root,
    )?;
    for path in split_nul_fields(&untracked_raw) {
        if !is_safe_relative_path(&path) {
            continue;
        }
        if seen.insert(path.clone()) {
            items.push(WorktreeMergePreviewItem {
                path,
                action: WorktreeMergeAction::Write,
                from_path: None,
            });
        }
    }

    items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(WorktreeMergePreview { items })
}

pub fn apply_worktree_merge(
    worktree_path: &str,
    live_workspace_path: &str,
) -> Result<WorktreeMergeResult, String> {
    let preview = preview_worktree_merge(worktree_path)?;
    let worktree_root = PathBuf::from(worktree_path);
    let live_root = PathBuf::from(live_workspace_path);
    if !live_root.exists() {
        return Err(format!(
            "live workspace path does not exist: {live_workspace_path}"
        ));
    }

    let tracked_patch = run_git_raw(&["diff", "--binary", "--no-color", "HEAD"], &worktree_root)?;
    if !tracked_patch.is_empty() {
        run_git_with_input(
            &["apply", "--whitespace=nowarn", "-"],
            &live_root,
            &tracked_patch,
        )?;
    }

    let untracked_raw = run_git_raw(
        &["ls-files", "--others", "--exclude-standard", "-z"],
        &worktree_root,
    )?;
    let untracked_paths: HashSet<String> = split_nul_fields(&untracked_raw).into_iter().collect();

    for item in &preview.items {
        if item.action != WorktreeMergeAction::Write {
            continue;
        }
        if !untracked_paths.contains(&item.path) {
            continue;
        }
        let rel_path = Path::new(&item.path);
        let src_path = worktree_root.join(rel_path);
        let dst_path = live_root.join(rel_path);
        if !src_path.exists() {
            return Err(format!(
                "worktree source path does not exist: {}",
                src_path.display()
            ));
        }
        if dst_path.exists() {
            let src_data = fs::read(&src_path)
                .map_err(|err| format!("failed to read {}: {err}", src_path.display()))?;
            let dst_data = fs::read(&dst_path)
                .map_err(|err| format!("failed to read {}: {err}", dst_path.display()))?;
            if src_data != dst_data {
                return Err(format!(
                    "cannot merge untracked file `{}` because live workspace already has different content",
                    item.path
                ));
            }
            continue;
        }
        if src_path.is_file() {
            copy_file(&src_path, &dst_path)?;
        } else if src_path.is_dir() {
            fs::create_dir_all(&dst_path).map_err(|err| {
                format!("failed to create directory {}: {err}", dst_path.display())
            })?;
        } else {
            return Err(format!(
                "unsupported path type for merge: {}",
                src_path.display()
            ));
        }
    }

    let merged_count = preview
        .items
        .iter()
        .filter(|item| item.action == WorktreeMergeAction::Write)
        .count();
    let deleted_count = preview
        .items
        .iter()
        .filter(|item| item.action == WorktreeMergeAction::Delete)
        .count();
    let renamed_count = preview
        .items
        .iter()
        .filter(|item| item.action == WorktreeMergeAction::Rename)
        .count();

    Ok(WorktreeMergeResult {
        merged_count,
        deleted_count,
        renamed_count,
    })
}

pub fn stop_worktree_checkout(worktree_path: &str) -> Result<(), String> {
    let worktree_root = PathBuf::from(worktree_path);
    if !worktree_root.exists() {
        return Ok(());
    }
    let git_root_raw = run_git(&["rev-parse", "--show-toplevel"], &worktree_root)?;
    let git_root = PathBuf::from(git_root_raw.trim());

    if let Err(detail) = run_git(&["worktree", "remove", "--force", worktree_path], &git_root) {
        let lower = detail.to_ascii_lowercase();
        let ignorable = lower.contains("is not a working tree")
            || lower.contains("not a git repository")
            || lower.contains("is not registered as a worktree")
            || lower.contains("no such file or directory");
        if !ignorable {
            return Err(format!("failed to stop worktree checkout: {detail}"));
        }
    }

    match fs::remove_dir_all(&worktree_root) {
        Ok(_) => {
            prune_empty_worktree_parents(&worktree_root);
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            prune_empty_worktree_parents(&worktree_root);
            Ok(())
        }
        Err(err) => Err(format!(
            "worktree removed, but cleanup failed for {}: {err}",
            worktree_root.display()
        )),
    }
}
