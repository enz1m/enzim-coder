use crate::data::AppDb;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HOST_PID_MARKER: &str = "__ENZIM_ACTION_HOST_PID__:";
const HOST_ACTION_WRAPPER: &str = r#"if command -v setsid >/dev/null 2>&1; then
  setsid /usr/bin/env bash -lc "$1" &
else
  /usr/bin/env bash -lc "$1" &
fi
child=$!
printf '__ENZIM_ACTION_HOST_PID__:%s\n' "$child"
wait "$child"
"#;

fn running_in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

fn is_flatpak_spawn_debug_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("** (flatpak-spawn:")
        && trimmed.contains("DEBUG:")
        && (trimmed.contains("child pid:") || trimmed.contains("child exit code"))
}

#[derive(Clone, Debug)]
pub struct SavedWorkspaceAction {
    pub id: String,
    pub title: Option<String>,
    pub command: String,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct ActionRunSnapshot {
    pub id: u64,
    #[allow(dead_code)]
    pub title: String,
    pub command: String,
    pub output: String,
    pub is_running: bool,
    pub status_text: String,
}

#[derive(Clone, Debug)]
struct ActionRunEntry {
    id: u64,
    workspace_path: String,
    title: String,
    command: String,
    output: VecDeque<String>,
    is_running: bool,
    exit_code: Option<i32>,
    error: Option<String>,
    pid: i32,
    local_pgid: i32,
    host_pid: Option<i32>,
    finished_at: Option<i64>,
}

struct ActionRunnerState {
    runs: HashMap<u64, ActionRunEntry>,
}

pub struct ActionRunner {
    state: Mutex<ActionRunnerState>,
    next_id: AtomicU64,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
}

pub fn canonical_workspace_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Some(canonical.to_string_lossy().to_string())
}

fn workspace_actions_key(workspace_path: &str) -> String {
    format!("workspace_actions::{workspace_path}")
}

fn parse_saved_actions(raw: &str) -> Vec<SavedWorkspaceAction> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("a{}", now_micros()));
        let command = item
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string());
        let Some(command) = command else {
            continue;
        };
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string());
        let created_at = item.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        out.push(SavedWorkspaceAction {
            id,
            title,
            command,
            created_at,
        });
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

fn serialize_saved_actions(items: &[SavedWorkspaceAction]) -> String {
    let value = Value::Array(
        items
            .iter()
            .map(|item| {
                json!({
                    "id": item.id,
                    "title": item.title,
                    "command": item.command,
                    "createdAt": item.created_at,
                })
            })
            .collect(),
    );
    value.to_string()
}

pub fn load_workspace_actions(db: &AppDb, workspace_path: &str) -> Vec<SavedWorkspaceAction> {
    let Some(canonical) = canonical_workspace_path(workspace_path) else {
        return Vec::new();
    };
    let key = workspace_actions_key(&canonical);
    db.get_setting(&key)
        .ok()
        .flatten()
        .map(|raw| parse_saved_actions(&raw))
        .unwrap_or_default()
}

pub fn save_workspace_action(
    db: &AppDb,
    workspace_path: &str,
    title: Option<&str>,
    command: &str,
) -> Result<(), String> {
    let Some(canonical) = canonical_workspace_path(workspace_path) else {
        return Err("No active workspace selected.".to_string());
    };
    let command = command.trim();
    if command.is_empty() {
        return Err("Command cannot be empty.".to_string());
    }
    let mut actions = load_workspace_actions(db, &canonical);
    let next = SavedWorkspaceAction {
        id: format!("a{}", now_micros()),
        title: title
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string()),
        command: command.to_string(),
        created_at: now_unix(),
    };
    actions.push(next);
    let key = workspace_actions_key(&canonical);
    db.set_setting(&key, &serialize_saved_actions(&actions))
        .map_err(|err| format!("Failed to save action: {err}"))
}

pub fn remove_workspace_action(
    db: &AppDb,
    workspace_path: &str,
    action_id: &str,
) -> Result<(), String> {
    let Some(canonical) = canonical_workspace_path(workspace_path) else {
        return Err("No active workspace selected.".to_string());
    };
    let action_id = action_id.trim();
    if action_id.is_empty() {
        return Err("Invalid action id.".to_string());
    }

    let mut actions = load_workspace_actions(db, &canonical);
    let before = actions.len();
    actions.retain(|item| item.id != action_id);
    if actions.len() == before {
        return Err("Action not found.".to_string());
    }

    let key = workspace_actions_key(&canonical);
    db.set_setting(&key, &serialize_saved_actions(&actions))
        .map_err(|err| format!("Failed to remove action: {err}"))
}

impl ActionRunner {
    fn new() -> Self {
        Self {
            state: Mutex::new(ActionRunnerState {
                runs: HashMap::new(),
            }),
            next_id: AtomicU64::new(1),
        }
    }

    fn append_output(&self, run_id: u64, line: String) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        if let Some(entry) = state.runs.get_mut(&run_id) {
            if is_flatpak_spawn_debug_line(&line) {
                return;
            }
            if let Some(raw_pid) = line.trim().strip_prefix(HOST_PID_MARKER) {
                if let Ok(host_pid) = raw_pid.parse::<i32>() {
                    entry.host_pid = Some(host_pid);
                    return;
                }
            }
            if entry.output.len() >= 400 {
                let _ = entry.output.pop_front();
            }
            entry.output.push_back(line);
        }
    }

    fn finish_run(&self, run_id: u64, status: Result<std::process::ExitStatus, String>) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        if let Some(entry) = state.runs.get_mut(&run_id) {
            entry.is_running = false;
            entry.finished_at = Some(now_unix());
            match status {
                Ok(exit) => {
                    entry.exit_code = exit.code();
                    let line = match exit.code() {
                        Some(code) => format!("[actions] exited with code {code}"),
                        None => "[actions] process terminated by signal".to_string(),
                    };
                    if entry.output.len() >= 400 {
                        let _ = entry.output.pop_front();
                    }
                    entry.output.push_back(line);
                }
                Err(err) => {
                    entry.error = Some(err.clone());
                    if entry.output.len() >= 400 {
                        let _ = entry.output.pop_front();
                    }
                    entry
                        .output
                        .push_back(format!("[actions] wait failed: {err}"));
                }
            }
        }
    }

    fn prune_finished_locked(state: &mut ActionRunnerState) {
        let now = now_unix();
        state.runs.retain(|_, entry| {
            entry.is_running
                || entry
                    .finished_at
                    .map(|finished| now.saturating_sub(finished) < 600)
                    .unwrap_or(true)
        });
    }

    pub fn start(
        self: &Arc<Self>,
        workspace_path: &str,
        title: Option<&str>,
        command: &str,
    ) -> Result<u64, String> {
        let Some(workspace_path) = canonical_workspace_path(workspace_path) else {
            return Err("No active workspace selected.".to_string());
        };
        let command = command.trim();
        if command.is_empty() {
            return Err("Command cannot be empty.".to_string());
        }
        let via_flatpak_host = running_in_flatpak();

        let run_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let title = title
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
            .unwrap_or_else(|| command.to_string());

        let mut child_cmd = if via_flatpak_host {
            let mut command_runner = Command::new("flatpak-spawn");
            command_runner.env_remove("G_MESSAGES_DEBUG");
            command_runner.env_remove("G_MESSAGES_PREFIXED");
            command_runner.env_remove("G_DEBUG");
            command_runner
                .arg("--host")
                .arg("--watch-bus")
                .arg(format!("--directory={workspace_path}"))
                .arg("/usr/bin/env")
                .arg("bash")
                .arg("-lc")
                .arg(HOST_ACTION_WRAPPER)
                .arg("_")
                .arg(command);
            command_runner
        } else {
            let mut command_runner = Command::new("/usr/bin/env");
            command_runner
                .arg("bash")
                .arg("-lc")
                .arg(command)
                .current_dir(&workspace_path);
            command_runner
        };
        child_cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "linux")]
        unsafe {
            child_cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = child_cmd.spawn().map_err(|err| {
            if via_flatpak_host {
                format!("Failed to start host command via flatpak-spawn: {err}")
            } else {
                format!("Failed to start command: {err}")
            }
        })?;
        let pid = child.id() as i32;

        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Failed to lock actions runner".to_string())?;
            Self::prune_finished_locked(&mut state);
            state.runs.insert(
                run_id,
                ActionRunEntry {
                    id: run_id,
                    workspace_path: workspace_path.clone(),
                    title,
                    command: command.to_string(),
                    output: VecDeque::from(vec![format!(
                        "[actions] started{} in {}",
                        if via_flatpak_host { " on host" } else { "" },
                        workspace_path
                    )]),
                    is_running: true,
                    exit_code: None,
                    error: None,
                    pid,
                    local_pgid: pid,
                    host_pid: None,
                    finished_at: None,
                },
            );
        }

        if let Some(stdout) = child.stdout.take() {
            let runner = Arc::clone(self);
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(line) => runner.append_output(run_id, line),
                        Err(err) => {
                            runner.append_output(run_id, format!("[stdout read error] {err}"));
                            break;
                        }
                    }
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let runner = Arc::clone(self);
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(line) => runner.append_output(run_id, line),
                        Err(err) => {
                            runner.append_output(run_id, format!("[stderr read error] {err}"));
                            break;
                        }
                    }
                }
            });
        }

        let runner = Arc::clone(self);
        thread::spawn(move || {
            let status = child.wait().map_err(|err| err.to_string());
            runner.finish_run(run_id, status);
        });

        Ok(run_id)
    }

    fn terminate_pgid(pgid: i32, sig: i32) {
        if pgid <= 0 {
            return;
        }
        #[cfg(target_family = "unix")]
        unsafe {
            let _ = libc::kill(-pgid, sig);
        }
    }

    fn terminate_host_pid(host_pid: i32, signal_name: &str) {
        if host_pid <= 0 || !running_in_flatpak() {
            return;
        }
        let _ = Command::new("flatpak-spawn")
            .arg("--host")
            .arg("/usr/bin/env")
            .arg("bash")
            .arg("-lc")
            .arg(r#"kill -s "$1" -- -"$2" 2>/dev/null || kill -s "$1" "$2" 2>/dev/null || true"#)
            .arg("_")
            .arg(signal_name)
            .arg(host_pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    pub fn kill(self: &Arc<Self>, run_id: u64) -> Result<(), String> {
        let (local_pgid, host_pid) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Failed to lock actions runner".to_string())?;
            let Some(entry) = state.runs.get_mut(&run_id) else {
                return Err("Action process not found.".to_string());
            };
            if !entry.is_running {
                return Ok(());
            }
            if entry.output.len() >= 400 {
                let _ = entry.output.pop_front();
            }
            entry.output.push_back("[actions] stopping...".to_string());
            (entry.local_pgid, entry.host_pid)
        };

        if let Some(host_pid) = host_pid {
            Self::terminate_host_pid(host_pid, "TERM");
        }
        Self::terminate_pgid(local_pgid, libc::SIGTERM);
        let runner = Arc::clone(self);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1200));
            let should_force = runner
                .state
                .lock()
                .ok()
                .and_then(|state| {
                    state
                        .runs
                        .get(&run_id)
                        .map(|entry| (entry.is_running, entry.host_pid))
                })
                .unwrap_or((false, None));
            if should_force.0 {
                if let Some(host_pid) = should_force.1 {
                    ActionRunner::terminate_host_pid(host_pid, "KILL");
                }
                ActionRunner::terminate_pgid(local_pgid, libc::SIGKILL);
            }
        });
        Ok(())
    }

    pub fn list_for_workspace(&self, workspace_path: &str) -> Vec<ActionRunSnapshot> {
        let Some(workspace_path) = canonical_workspace_path(workspace_path) else {
            return Vec::new();
        };
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };
        Self::prune_finished_locked(&mut state);
        let mut out: Vec<ActionRunSnapshot> = state
            .runs
            .values()
            .filter(|entry| entry.workspace_path == workspace_path)
            .map(|entry| {
                let status_text = if entry.is_running {
                    format!("Running · pid {}", entry.host_pid.unwrap_or(entry.pid))
                } else if let Some(err) = entry.error.as_deref() {
                    format!("Failed · {err}")
                } else if let Some(code) = entry.exit_code {
                    format!("Exited · code {code}")
                } else {
                    "Exited".to_string()
                };
                ActionRunSnapshot {
                    id: entry.id,
                    title: entry.title.clone(),
                    command: entry.command.clone(),
                    output: entry.output.iter().cloned().collect::<Vec<_>>().join("\n"),
                    is_running: entry.is_running,
                    status_text,
                }
            })
            .collect();
        out.sort_by(|a, b| b.id.cmp(&a.id));
        out
    }

    pub fn shutdown_all(&self) {
        let mut pgids = Vec::new();
        if let Ok(state) = self.state.lock() {
            for entry in state.runs.values() {
                if entry.is_running {
                    if let Some(host_pid) = entry.host_pid {
                        Self::terminate_host_pid(host_pid, "TERM");
                    }
                    pgids.push(entry.local_pgid);
                }
            }
        }
        for pgid in pgids {
            Self::terminate_pgid(pgid, libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(200));
        let mut pgids_force = Vec::new();
        if let Ok(state) = self.state.lock() {
            for entry in state.runs.values() {
                if entry.is_running {
                    if let Some(host_pid) = entry.host_pid {
                        Self::terminate_host_pid(host_pid, "KILL");
                    }
                    pgids_force.push(entry.local_pgid);
                }
            }
        }
        for pgid in pgids_force {
            Self::terminate_pgid(pgid, libc::SIGKILL);
        }
    }
}

fn global_runner_inner() -> &'static Arc<ActionRunner> {
    static RUNNER: OnceLock<Arc<ActionRunner>> = OnceLock::new();
    RUNNER.get_or_init(|| Arc::new(ActionRunner::new()))
}

pub fn action_runner() -> Arc<ActionRunner> {
    Arc::clone(global_runner_inner())
}

pub fn shutdown_all_running_actions() {
    global_runner_inner().shutdown_all();
}
