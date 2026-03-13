use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

fn format_git_failure(
    args: &[&str],
    status: std::process::ExitStatus,
    stderr: &[u8],
    stdout: &[u8],
) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    if detail.is_empty() {
        format!("git {:?} failed (status={})", args, status)
    } else {
        format!("git {:?} failed (status={}): {}", args, status, detail)
    }
}

fn running_in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

fn strip_flatpak_spawn_debug_lines(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with("** (flatpak-spawn:")
                && trimmed.contains("DEBUG:")
                && (trimmed.contains("child pid:") || trimmed.contains("child exit code")))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_git_command(cwd: &Path, args: &[&str], envs: &[(&str, OsString)]) -> Command {
    if running_in_flatpak() {
        let mut command = Command::new("flatpak-spawn");
        command.env_remove("G_MESSAGES_DEBUG");
        command.env_remove("G_MESSAGES_PREFIXED");
        command.env_remove("G_DEBUG");
        command.arg("--host");
        command.arg(format!("--directory={}", cwd.to_string_lossy()));
        for (key, value) in envs {
            command.arg(format!("--env={}={}", key, value.to_string_lossy()));
        }
        command.arg("git");
        command.args(args);
        command
    } else {
        let mut command = Command::new("git");
        command.args(args).current_dir(cwd);
        for (key, value) in envs {
            command.env(key, value);
        }
        command
    }
}

fn run_git_output_with_env(
    cwd: &Path,
    args: &[&str],
    envs: &[(&str, OsString)],
) -> Result<std::process::Output, String> {
    let mut command = build_git_command(cwd, args, envs);
    command.output().map_err(|err| {
        if running_in_flatpak() {
            format!("failed to run host git {:?} via flatpak-spawn: {err}", args)
        } else {
            format!("failed to run git {:?}: {err}", args)
        }
    })
}

pub fn run_git_text(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = run_git_output_with_env(cwd, args, &[])?;
    if !output.status.success() {
        return Err(format_git_failure(
            args,
            output.status,
            &output.stderr,
            &output.stdout,
        ));
    }
    Ok(
        strip_flatpak_spawn_debug_lines(&String::from_utf8_lossy(&output.stdout))
            .trim()
            .to_string(),
    )
}

pub fn run_git_text_with_env(
    cwd: &Path,
    args: &[&str],
    envs: &[(&str, OsString)],
) -> Result<String, String> {
    let output = run_git_output_with_env(cwd, args, envs)?;
    if !output.status.success() {
        return Err(format_git_failure(
            args,
            output.status,
            &output.stderr,
            &output.stdout,
        ));
    }
    Ok(strip_flatpak_spawn_debug_lines(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

pub fn run_git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = run_git_output_with_env(cwd, args, &[])?;
    if !output.status.success() {
        return Err(format_git_failure(
            args,
            output.status,
            &output.stderr,
            &output.stdout,
        ));
    }
    Ok(output.stdout)
}

pub fn run_git_with_input(cwd: &Path, args: &[&str], input: &[u8]) -> Result<(), String> {
    let mut child = build_git_command(cwd, args, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            if running_in_flatpak() {
                format!("failed to run host git {:?} via flatpak-spawn: {err}", args)
            } else {
                format!("failed to run git {:?}: {err}", args)
            }
        })?;

    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin
            .write_all(input)
            .map_err(|err| format!("failed to write stdin for git {:?}: {err}", args))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("failed to await git {:?}: {err}", args))?;
    if !output.status.success() {
        return Err(format_git_failure(
            args,
            output.status,
            &output.stderr,
            &output.stdout,
        ));
    }
    Ok(())
}

fn scoped_git_args(git_dir: &Path, work_tree: &Path, args: &[&str]) -> Vec<String> {
    let mut scoped = vec![
        "--git-dir".to_string(),
        git_dir.to_string_lossy().to_string(),
        "--work-tree".to_string(),
        work_tree.to_string_lossy().to_string(),
    ];
    scoped.extend(args.iter().map(|arg| (*arg).to_string()));
    scoped
}

pub fn run_git_scoped_text(
    git_dir: &Path,
    work_tree: &Path,
    args: &[&str],
) -> Result<String, String> {
    let scoped = scoped_git_args(git_dir, work_tree, args);
    let refs = scoped.iter().map(String::as_str).collect::<Vec<_>>();
    run_git_text(work_tree, &refs)
}

pub fn run_git_scoped_bytes(
    git_dir: &Path,
    work_tree: &Path,
    args: &[&str],
) -> Result<Vec<u8>, String> {
    let scoped = scoped_git_args(git_dir, work_tree, args);
    let refs = scoped.iter().map(String::as_str).collect::<Vec<_>>();
    run_git_bytes(work_tree, &refs)
}
