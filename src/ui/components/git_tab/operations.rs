use adw::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::data::AppDb;

use super::model::{
    BranchAction, GitFileEntry, GitSnapshot, InitRepoOptions, LocalCommitEntry, PushCredentials,
    PushOutcome, UpstreamOptions,
};

pub(super) fn resolve_workspace_root(
    db: &AppDb,
    active_workspace_path: &Rc<RefCell<Option<String>>>,
) -> PathBuf {
    if let Some(path) = active_workspace_path.borrow().clone() {
        let path = PathBuf::from(path);
        if path.exists() {
            return canonicalize_or_self(path);
        }
    }

    if let Ok(Some(saved_path)) = db.get_setting("last_workspace_path") {
        let path = PathBuf::from(saved_path);
        if path.exists() {
            return canonicalize_or_self(path);
        }
    }

    canonicalize_or_self(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub(super) fn install_workspace_observer(
    db: Rc<AppDb>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    observed_workspace_root: Rc<RefCell<PathBuf>>,
    trigger_refresh: Rc<dyn Fn()>,
) {
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        let desired_root = resolve_workspace_root(&db, &active_workspace_path);
        if *observed_workspace_root.borrow() != desired_root {
            observed_workspace_root.replace(desired_root);
            trigger_refresh();
        }
        gtk::glib::ControlFlow::Continue
    });
}

pub(super) fn install_refresh_on_map(content_box: &gtk::Box, trigger_refresh: Rc<dyn Fn()>) {
    content_box.connect_map(move |_| {
        trigger_refresh();
    });
}

pub(super) fn load_git_snapshot(workspace_root: &str) -> Result<GitSnapshot, String> {
    let repo_name = PathBuf::from(workspace_root)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| workspace_root.to_string());

    run_git_command(workspace_root, &["rev-parse", "--is-inside-work-tree"], &[])
        .map_err(|_| "No Git repository found for the active workspace.".to_string())?;

    let branch_output = run_git_command(workspace_root, &["symbolic-ref", "--short", "HEAD"], &[]);
    let branch_label = match branch_output {
        Ok(stdout) => stdout.trim().to_string(),
        Err(_) => {
            let detached = run_git_command(workspace_root, &["rev-parse", "--short", "HEAD"], &[])
                .unwrap_or_else(|_| "unknown".to_string());
            format!("detached@{}", detached.trim())
        }
    };

    let current_branch = if branch_label.starts_with("detached@") {
        None
    } else {
        Some(branch_label.clone())
    };
    let has_upstream = current_branch
        .as_ref()
        .map(|branch| {
            let remote_key = format!("branch.{}.remote", branch);
            let merge_key = format!("branch.{}.merge", branch);
            let remote = run_git_command(workspace_root, &["config", "--get", &remote_key], &[])
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let merge = run_git_command(workspace_root, &["config", "--get", &merge_key], &[])
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            remote.is_some() && merge.is_some()
        })
        .unwrap_or(false);
    let remotes = run_git_command(workspace_root, &["remote"], &[])
        .ok()
        .map(|output| {
            output
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let remote_urls = remotes
        .iter()
        .map(|remote_name| {
            let url = run_git_command(
                workspace_root,
                &["remote", "get-url", "--push", remote_name],
                &[],
            )
            .ok()
            .or_else(|| {
                run_git_command(workspace_root, &["remote", "get-url", remote_name], &[]).ok()
            })
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
            (remote_name.clone(), url)
        })
        .collect::<Vec<(String, String)>>();
    let upstream_remote_name = if has_upstream {
        run_git_command(
            workspace_root,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
            &[],
        )
        .ok()
        .and_then(|upstream_ref| remote_name_from_upstream_ref(upstream_ref.trim(), &remotes))
    } else {
        None
    };
    let repository_url = upstream_remote_name
        .as_ref()
        .or_else(|| remotes.first())
        .and_then(|remote_name| {
            remote_urls
                .iter()
                .find(|(name, _)| name == remote_name)
                .map(|(_, url)| url.clone())
        })
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| "—".to_string());

    let mut ahead_count = 0usize;
    let mut unpushed_commits = Vec::<LocalCommitEntry>::new();
    let push_hint = if has_upstream {
        if let Ok(count) =
            run_git_command(workspace_root, &["rev-list", "--count", "@{u}..HEAD"], &[])
        {
            ahead_count = count.trim().parse::<usize>().unwrap_or(0);
        } else if let Ok(count) =
            run_git_command(workspace_root, &["rev-list", "--count", "HEAD"], &[])
        {
            ahead_count = count.trim().parse::<usize>().unwrap_or(0);
            if ahead_count > 0 {
                if let Ok(log) = run_git_command(
                    workspace_root,
                    &["log", "--max-count", "8", "--pretty=format:%h%x09%s"],
                    &[],
                ) {
                    unpushed_commits = parse_compact_commit_lines(&log);
                }
                return Ok(GitSnapshot {
                    workspace_root: workspace_root.to_string(),
                    repo_name,
                    repository_url,
                    branch_label,
                    push_hint:
                        "Upstream configured but not published yet. Push to publish this branch."
                            .to_string(),
                    has_upstream,
                    ahead_count,
                    unpushed_commits,
                    remotes,
                    files: parse_porcelain_entries(&run_git_command(
                        workspace_root,
                        &["status", "--porcelain=v1", "--untracked-files=all"],
                        &[],
                    )?),
                });
            }
        }

        if ahead_count > 0 {
            if let Ok(log) = run_git_command(
                workspace_root,
                &[
                    "log",
                    "--max-count",
                    "8",
                    "--pretty=format:%h%x09%s",
                    "@{u}..HEAD",
                ],
                &[],
            ) {
                unpushed_commits = parse_compact_commit_lines(&log);
            }
            format!("{} commit(s) ahead of upstream.", ahead_count)
        } else {
            "All commits are pushed.".to_string()
        }
    } else {
        if let Ok(count) = run_git_command(workspace_root, &["rev-list", "--count", "HEAD"], &[]) {
            ahead_count = count.trim().parse::<usize>().unwrap_or(0);
        }
        if ahead_count > 0 {
            if let Ok(log) = run_git_command(
                workspace_root,
                &["log", "--max-count", "8", "--pretty=format:%h%x09%s"],
                &[],
            ) {
                unpushed_commits = parse_compact_commit_lines(&log);
            }
            "No upstream configured; showing recent local commits.".to_string()
        } else {
            "No commits yet; create one to start publishing.".to_string()
        }
    };

    let porcelain = run_git_command(
        workspace_root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        &[],
    )?;
    let files = parse_porcelain_entries(&porcelain);

    Ok(GitSnapshot {
        workspace_root: workspace_root.to_string(),
        repo_name,
        repository_url,
        branch_label,
        push_hint,
        has_upstream,
        ahead_count,
        unpushed_commits,
        remotes,
        files,
    })
}

pub(super) fn remote_name_from_upstream_ref(
    upstream_ref: &str,
    remotes: &[String],
) -> Option<String> {
    if remotes.is_empty() {
        return None;
    }

    remotes
        .iter()
        .filter(|remote| upstream_ref.starts_with(&format!("{}/", remote)))
        .max_by_key(|remote| remote.len())
        .cloned()
}

pub(super) fn run_configure_upstream(
    repo_root: &str,
    options: &UpstreamOptions,
    credentials: Option<PushCredentials>,
) -> Result<String, String> {
    let remote_name = options.remote_name.trim();
    let remote_url = options.remote_url.trim();
    let branch_name = options.branch_name.trim();

    if remote_name.is_empty() || branch_name.is_empty() {
        return Err("Upstream setup failed: remote and branch are required.".to_string());
    }

    let existing_remotes = run_git_command(repo_root, &["remote"], &[])
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<String>>();
    let remote_exists = existing_remotes.iter().any(|name| name == remote_name);

    if !remote_url.is_empty() {
        if remote_exists {
            run_git_command(
                repo_root,
                &["remote", "set-url", remote_name, remote_url],
                &[],
            )?;
        } else {
            run_git_command(repo_root, &["remote", "add", remote_name, remote_url], &[])?;
        }
    } else if !remote_exists {
        return Err("Upstream setup failed: remote URL is required for a new remote.".to_string());
    }

    match run_push_command(
        repo_root,
        &["--porcelain", "--set-upstream", remote_name, branch_name],
        credentials,
    ) {
        PushOutcome::Success(_) => Ok(format!(
            "Upstream configured: {} -> {}/{}",
            branch_name, remote_name, branch_name
        )),
        PushOutcome::AuthRequired(err) => Err(err),
        PushOutcome::Failure(err) => Err(format!("Upstream setup failed: {}", err)),
    }
}

pub(super) fn parse_compact_commit_lines(input: &str) -> Vec<LocalCommitEntry> {
    let mut out = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((hash, summary)) = trimmed.split_once('\t') {
            out.push(LocalCommitEntry {
                short_hash: hash.trim().to_string(),
                summary: summary.trim().to_string(),
            });
        } else {
            out.push(LocalCommitEntry {
                short_hash: "commit".to_string(),
                summary: trimmed.to_string(),
            });
        }
    }
    out
}

pub(super) fn canonicalize_or_self(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

pub(super) fn run_initialize_repository(
    workspace_root: &str,
    options: &InitRepoOptions,
) -> Result<String, String> {
    let workspace_path = PathBuf::from(workspace_root);
    if !workspace_path.exists() || !workspace_path.is_dir() {
        return Err("Initialize failed: workspace path does not exist.".to_string());
    }

    if workspace_path.join(".git").exists() {
        return Err("A Git repository already exists in this workspace.".to_string());
    }

    let init_with_branch = run_git_command(
        workspace_root,
        &["init", "--initial-branch", &options.branch],
        &[],
    );
    if init_with_branch.is_err() {
        run_git_command(workspace_root, &["init"], &[])?;
        let head_ref = format!("refs/heads/{}", options.branch);
        let _ = run_git_command(workspace_root, &["symbolic-ref", "HEAD", &head_ref], &[]);
    }

    if options.create_gitignore {
        let gitignore_path = workspace_path.join(".gitignore");
        if !gitignore_path.exists() {
            let gitignore = "# Build output\n/target\n\n# Local environment\n.env\n.env.*\n\n# OS files\n.DS_Store\nThumbs.db\n";
            fs::write(&gitignore_path, gitignore)
                .map_err(|err| format!("Initialize failed: unable to write .gitignore ({err})"))?;
        }
    }

    if options.create_initial_commit {
        run_git_command(workspace_root, &["add", "-A"], &[])?;
        match run_git_command(
            workspace_root,
            &["commit", "-m", &options.commit_message],
            &[],
        ) {
            Ok(_) => {}
            Err(err) => {
                let lower = err.to_ascii_lowercase();
                if lower.contains("user.name") || lower.contains("user.email") {
                    return Err(
                        "Repository initialized, but initial commit failed. Configure Git identity with `git config user.name` and `git config user.email`, then commit again."
                            .to_string(),
                    );
                }
                if lower.contains("nothing to commit") {
                    return Ok(
                        "Repository initialized. No files to commit yet; add files and create your first commit.".to_string(),
                    );
                }
                return Err(format!(
                    "Repository initialized, but initial commit failed: {}",
                    err
                ));
            }
        }
    }

    Ok(
        "Repository initialized successfully. Next: add remote (`git remote add origin <url>`) and push.".to_string(),
    )
}

pub(super) fn parse_porcelain_entries(input: &str) -> Vec<GitFileEntry> {
    let mut out = Vec::new();
    for line in input.lines() {
        if line.len() < 3 {
            continue;
        }
        let status_raw = &line[0..2];
        let path_raw = line[3..].trim();
        if path_raw.is_empty() {
            continue;
        }

        let path = if let Some((_, new_path)) = path_raw.split_once(" -> ") {
            new_path.trim().to_string()
        } else {
            path_raw.to_string()
        };

        let status = if status_raw == "??" {
            "??".to_string()
        } else {
            let staged = status_raw.chars().next().unwrap_or(' ');
            let unstaged = status_raw.chars().nth(1).unwrap_or(' ');
            let code = if staged != ' ' { staged } else { unstaged };
            code.to_string()
        };

        out.push(GitFileEntry {
            status,
            path,
            selected: true,
        });
    }
    out
}

pub(super) fn run_commit_selected(
    repo_root: &str,
    selected_paths: &[String],
    message: &str,
) -> Result<String, String> {
    if selected_paths.is_empty() {
        return Err("Select at least one file to commit.".to_string());
    }

    let mut status_args = vec![
        "status".to_string(),
        "--porcelain=v1".to_string(),
        "--untracked-files=all".to_string(),
        "--".to_string(),
    ];
    status_args.extend(selected_paths.iter().cloned());
    let status_refs: Vec<&str> = status_args.iter().map(|value| value.as_str()).collect();
    let status_output = run_git_command(repo_root, &status_refs, &[])?;
    let active_selected_set: HashSet<String> = parse_porcelain_entries(&status_output)
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    let active_selected_paths: Vec<String> = selected_paths
        .iter()
        .filter(|path| active_selected_set.contains((*path).as_str()))
        .cloned()
        .collect();
    if active_selected_paths.is_empty() {
        return Err(
            "Selected files no longer have changes. Refresh Git status and try again.".to_string(),
        );
    }

    let mut staged_paths = Vec::<String>::new();
    for path in &active_selected_paths {
        match stage_path_for_commit(repo_root, path) {
            Ok(true) => staged_paths.push(path.clone()),
            Ok(false) => continue,
            Err(err) => return Err(format!("Commit failed while staging `{}`: {}", path, err)),
        }
    }

    if staged_paths.is_empty() {
        return Err(
            "Selected files no longer match repository paths. Refresh Git status and try again."
                .to_string(),
        );
    }

    let mut commit_args = vec![
        "commit".to_string(),
        "-m".to_string(),
        message.to_string(),
        "--".to_string(),
    ];
    commit_args.extend(staged_paths.iter().cloned());
    let commit_refs: Vec<&str> = commit_args.iter().map(|value| value.as_str()).collect();

    match run_git_command(repo_root, &commit_refs, &[]) {
        Ok(stdout) => {
            let first_line = stdout.lines().next().unwrap_or("Commit created.");
            let ignored_count = selected_paths.len().saturating_sub(staged_paths.len());
            if ignored_count > 0 {
                Ok(format!(
                    "Commit succeeded: {} (ignored {} stale selection(s)).",
                    first_line.trim(),
                    ignored_count
                ))
            } else {
                Ok(format!("Commit succeeded: {}", first_line.trim()))
            }
        }
        Err(err) => {
            let lower = err.to_ascii_lowercase();
            if lower.contains("nothing to commit") {
                Err("Nothing to commit for selected files.".to_string())
            } else if lower.contains("author identity unknown")
                || lower.contains("please tell me who you are")
                || lower.contains("unable to auto-detect email address")
            {
                Err(
                    "Commit failed: Git identity is not configured. Set `git config user.name \"Your Name\"` and `git config user.email \"you@example.com\"`, then retry.".to_string(),
                )
            } else if lower.contains("index.lock") {
                Err("Commit failed: index lock present. Retry after existing Git operation completes.".to_string())
            } else if lower.contains("conflict") {
                Err("Commit failed due to merge conflicts in selected files.".to_string())
            } else {
                Err(format!("Commit failed: {}", err))
            }
        }
    }
}

fn stage_path_for_commit(repo_root: &str, path: &str) -> Result<bool, String> {
    match run_git_command(repo_root, &["add", "--", path], &[]) {
        Ok(_) => {}
        Err(err) => {
            if !err.to_ascii_lowercase().contains("did not match any files") {
                return Err(err);
            }
            let _ = run_git_command(
                repo_root,
                &["rm", "--cached", "--ignore-unmatch", "--", path],
                &[],
            );
        }
    }

    let status_output = run_git_command(
        repo_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--",
            path,
        ],
        &[],
    )?;
    if status_output.trim().is_empty() {
        return Ok(false);
    }

    let mut staged = status_output
        .lines()
        .next()
        .and_then(|line| line.chars().next())
        .map(|c| c != ' ')
        .unwrap_or(false);

    if !staged && status_output.lines().any(|line| line.starts_with(" D")) {
        let _ = run_git_command(
            repo_root,
            &["rm", "--cached", "--ignore-unmatch", "--", path],
            &[],
        );
        let status_after_rm = run_git_command(
            repo_root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--",
                path,
            ],
            &[],
        )?;
        staged = status_after_rm
            .lines()
            .next()
            .and_then(|line| line.chars().next())
            .map(|c| c != ' ')
            .unwrap_or(false);
    }

    Ok(staged)
}

pub(super) fn selected_line_delta(
    repo_root: &str,
    entries: &[GitFileEntry],
) -> Result<(usize, usize), String> {
    let selected_paths: Vec<String> = entries
        .iter()
        .filter(|entry| entry.selected)
        .map(|entry| entry.path.clone())
        .collect();
    if selected_paths.is_empty() {
        return Ok((0, 0));
    }

    let mut diff_args = vec![
        "diff".to_string(),
        "--numstat".to_string(),
        "HEAD".to_string(),
        "--".to_string(),
    ];
    diff_args.extend(selected_paths.iter().cloned());
    let diff_refs: Vec<&str> = diff_args.iter().map(|value| value.as_str()).collect();

    let mut totals = match run_git_command(repo_root, &diff_refs, &[]) {
        Ok(output) => parse_numstat_totals(&output),
        Err(err) => {
            let lower = err.to_ascii_lowercase();
            if lower.contains("bad revision 'head'")
                || lower.contains("ambiguous argument 'head'")
                || lower.contains("unknown revision or path")
            {
                (0, 0)
            } else {
                return Err(err);
            }
        }
    };

    for path in entries
        .iter()
        .filter(|entry| entry.selected && entry.status == "??")
        .map(|entry| entry.path.as_str())
    {
        totals.0 += count_file_lines_for_untracked(repo_root, path);
    }

    Ok(totals)
}

fn parse_numstat_totals(input: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;

    for line in input.lines() {
        let mut parts = line.split('\t');
        let add = parts.next().unwrap_or("0").trim();
        let del = parts.next().unwrap_or("0").trim();
        added += add.parse::<usize>().unwrap_or(0);
        removed += del.parse::<usize>().unwrap_or(0);
    }

    (added, removed)
}

fn count_file_lines_for_untracked(repo_root: &str, relative_path: &str) -> usize {
    let full_path = Path::new(repo_root).join(relative_path);
    let bytes = match fs::read(full_path) {
        Ok(value) => value,
        Err(_) => return 0,
    };

    if bytes.contains(&0) {
        return 0;
    }
    if bytes.is_empty() {
        return 0;
    }

    let newline_count = bytes.iter().filter(|&&byte| byte == b'\n').count();
    if bytes.last().copied() == Some(b'\n') {
        newline_count
    } else {
        newline_count + 1
    }
}

pub(super) fn run_fetch(repo_root: &str) -> Result<String, String> {
    match run_git_command(repo_root, &["fetch", "--prune"], &[]) {
        Ok(output) => {
            let first_line = output.lines().next().unwrap_or("Fetch completed.").trim();
            if first_line.is_empty() {
                Ok("Fetch completed.".to_string())
            } else {
                Ok(format!("Fetch completed: {}", first_line))
            }
        }
        Err(err) => Err(format!("Fetch failed: {}", err)),
    }
}

pub(super) fn run_pull_ff_only(repo_root: &str) -> Result<String, String> {
    let is_dirty = run_git_command(repo_root, &["status", "--porcelain"], &[])
        .map(|output| !output.trim().is_empty())
        .unwrap_or(false);
    if is_dirty {
        return Err(
            "Pull blocked: working tree has local changes. Commit/stash them first, then retry."
                .to_string(),
        );
    }

    match run_git_command(repo_root, &["pull", "--ff-only"], &[]) {
        Ok(output) => {
            let first_line = output.lines().next().unwrap_or("Pull completed.").trim();
            if first_line.is_empty() {
                Ok("Pull completed.".to_string())
            } else {
                Ok(format!("Pull completed: {}", first_line))
            }
        }
        Err(err) => {
            let lower = err.to_ascii_lowercase();
            if lower.contains("not possible to fast-forward")
                || lower.contains("non-fast-forward")
                || lower.contains("divergent branches")
            {
                return Err(
                    "Pull requires merge/rebase (non fast-forward). Use terminal to resolve or rebase, then refresh."
                        .to_string(),
                );
            }
            Err(format!("Pull failed: {}", err))
        }
    }
}

pub(super) fn list_local_branches(repo_root: &str) -> Vec<String> {
    run_git_command(repo_root, &["branch", "--format=%(refname:short)"], &[])
        .ok()
        .map(|output| {
            output
                .lines()
                .map(|line| line.trim().trim_start_matches('*').trim().to_string())
                .filter(|line| !line.is_empty())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
}

pub(super) fn run_branch_action(repo_root: &str, action: BranchAction) -> Result<String, String> {
    match action {
        BranchAction::Switch(branch) => {
            if branch.trim().is_empty() {
                return Err("Branch switch failed: branch name is required.".to_string());
            }
            run_git_command(repo_root, &["switch", &branch], &[])
                .or_else(|_| run_git_command(repo_root, &["checkout", &branch], &[]))
                .map(|_| format!("Switched to branch: {}", branch))
                .map_err(|err| format!("Branch switch failed: {}", err))
        }
        BranchAction::Create(branch) => {
            if branch.trim().is_empty() {
                return Err("Branch creation failed: branch name is required.".to_string());
            }
            run_git_command(repo_root, &["switch", "-c", &branch], &[])
                .or_else(|_| run_git_command(repo_root, &["checkout", "-b", &branch], &[]))
                .map_err(|err| format!("Branch creation failed: {}", err))?;

            let remotes = run_git_command(repo_root, &["remote"], &[])
                .ok()
                .map(|output| {
                    output
                        .lines()
                        .map(|line| line.trim().to_string())
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();

            let remote_name = if remotes.iter().any(|remote| remote == "origin") {
                Some("origin".to_string())
            } else {
                remotes.first().cloned()
            };

            if let Some(remote_name) = remote_name {
                match set_local_branch_upstream(repo_root, &remote_name, &branch) {
                    Ok(_) => Ok(format!(
                        "Created and switched to branch: {} (upstream configured automatically)",
                        branch
                    )),
                    Err(err) => Ok(format!(
                        "Created and switched to branch: {}. Upstream auto-config failed: {}",
                        branch, err
                    )),
                }
            } else {
                Ok(format!(
                    "Created and switched to branch: {}. No remote found, so upstream was not set.",
                    branch
                ))
            }
        }
    }
}

pub(super) fn set_local_branch_upstream(
    repo_root: &str,
    remote_name: &str,
    branch_name: &str,
) -> Result<(), String> {
    let remote_key = format!("branch.{}.remote", branch_name);
    let merge_key = format!("branch.{}.merge", branch_name);
    let merge_ref = format!("refs/heads/{}", branch_name);

    run_git_command(repo_root, &["config", &remote_key, remote_name], &[])?;
    run_git_command(repo_root, &["config", &merge_key, &merge_ref], &[])?;
    Ok(())
}

pub(super) fn run_push_with_optional_credentials(
    repo_root: &str,
    credentials: Option<PushCredentials>,
) -> PushOutcome {
    run_push_command(repo_root, &["--porcelain"], credentials)
}

pub(super) fn run_push_command(
    repo_root: &str,
    push_args: &[&str],
    credentials: Option<PushCredentials>,
) -> PushOutcome {
    let had_credentials = credentials.is_some();
    let mut envs = vec![
        ("GIT_TERMINAL_PROMPT", OsString::from("0")),
        ("GIT_ASKPASS", OsString::from("/bin/false")),
        ("SSH_ASKPASS", OsString::from("/bin/false")),
        ("GCM_INTERACTIVE", OsString::from("never")),
    ];
    let mut cleanup_script: Option<PathBuf> = None;

    if let Some(credentials) = credentials {
        match create_askpass_script() {
            Ok(script_path) => {
                envs.push(("GIT_ASKPASS", script_path.as_os_str().to_os_string()));
                envs.push(("GIT_USERNAME", OsString::from(credentials.username)));
                envs.push(("GIT_PASSWORD", OsString::from(credentials.password)));
                cleanup_script = Some(script_path);
            }
            Err(err) => {
                return PushOutcome::Failure(format!("Push failed: {}", err));
            }
        }
    }

    let mut args = vec!["push"];
    args.extend_from_slice(push_args);
    let result = run_git_command(repo_root, &args, &envs);

    if let Some(script_path) = cleanup_script {
        let _ = fs::remove_file(script_path);
    }

    match result {
        Ok(stdout) => {
            let summary = stdout
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("Push completed.")
                .trim()
                .to_string();
            PushOutcome::Success(format!("Push succeeded: {}", summary))
        }
        Err(err) => {
            let lower = err.to_ascii_lowercase();
            if credentials_required(&lower) {
                if !had_credentials {
                    return PushOutcome::AuthRequired(err);
                }
                return PushOutcome::Failure(
                    "Push failed: authentication was rejected.".to_string(),
                );
            }
            if lower.contains("no configured push destination")
                || lower.contains("has no upstream branch")
            {
                return PushOutcome::Failure(
                    "Push failed: no upstream remote configured. Set upstream and retry."
                        .to_string(),
                );
            }
            if lower.contains("non-fast-forward") || lower.contains("rejected") {
                return PushOutcome::Failure(
                    "Push rejected (non-fast-forward). Pull/rebase and retry push.".to_string(),
                );
            }
            PushOutcome::Failure(format!("Push failed: {}", err))
        }
    }
}

pub(super) fn run_git_command(
    repo_root: &str,
    args: &[&str],
    envs: &[(&str, OsString)],
) -> Result<String, String> {
    crate::git_exec::run_git_text_with_env(Path::new(repo_root), args, envs)
}

pub(super) fn credentials_required(error_text: &str) -> bool {
    error_text.contains("authentication failed")
        || error_text.contains("could not read username")
        || error_text.contains("terminal prompts disabled")
        || error_text.contains("http basic")
        || error_text.contains("access denied")
}

pub(super) fn create_askpass_script() -> Result<PathBuf, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let file_name = format!("enzimcoder-git-askpass-{}-{}.sh", std::process::id(), now);
    let script_path = std::env::temp_dir().join(file_name);

    let contents = "#!/bin/sh\ncase \"$1\" in\n  *Username*) printf '%s\\n' \"$GIT_USERNAME\" ;;\n  *) printf '%s\\n' \"$GIT_PASSWORD\" ;;\nesac\n";
    fs::write(&script_path, contents)
        .map_err(|err| format!("unable to write askpass helper: {}", err))?;

    let mut perms = fs::metadata(&script_path)
        .map_err(|err| format!("unable to stat askpass helper: {}", err))?
        .permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&script_path, perms)
        .map_err(|err| format!("unable to chmod askpass helper: {}", err))?;

    Ok(script_path)
}
