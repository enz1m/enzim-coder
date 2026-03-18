use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

pub fn format_rpc_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown app-server error");
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);

    let mut out = format!("app-server error {code}: {message}");

    let retry_after_seconds = error
        .get("data")
        .and_then(|v| v.get("retryAfterSeconds"))
        .and_then(Value::as_i64)
        .or_else(|| error.get("retryAfterSeconds").and_then(Value::as_i64));
    if let Some(seconds) = retry_after_seconds {
        out.push_str(&format!(" (retryAfterSeconds={seconds})"));
    }

    let resets_at = error
        .get("data")
        .and_then(|v| v.get("resetsAt"))
        .and_then(Value::as_i64)
        .or_else(|| error.get("resetsAt").and_then(Value::as_i64));
    if let Some(ts) = resets_at {
        out.push_str(&format!(" (resetsAt={ts})"));
    }

    out
}

pub fn running_in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

static HOST_CODEX_EXECUTABLE: OnceLock<Option<String>> = OnceLock::new();
static LOCAL_CODEX_EXECUTABLE: OnceLock<Option<String>> = OnceLock::new();

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn push_candidate(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if is_executable_file(&path) {
        candidates.push(path);
    }
}

fn local_codex_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        push_candidate(&mut candidates, home.join(".local/bin/codex"));
        push_candidate(&mut candidates, home.join(".npm-global/bin/codex"));
        push_candidate(&mut candidates, home.join(".local/share/npm/bin/codex"));
        push_candidate(&mut candidates, home.join(".volta/bin/codex"));
        push_candidate(&mut candidates, home.join(".yarn/bin/codex"));
        push_candidate(
            &mut candidates,
            home.join(".config/yarn/global/node_modules/.bin/codex"),
        );

        let nvm_versions_dir = home.join(".nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(&nvm_versions_dir) {
            for entry in entries.flatten() {
                push_candidate(&mut candidates, entry.path().join("bin/codex"));
            }
        }
    }

    push_candidate(&mut candidates, PathBuf::from("/usr/local/bin/codex"));
    push_candidate(&mut candidates, PathBuf::from("/usr/bin/codex"));

    if let Some(path_env) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_env) {
            push_candidate(&mut candidates, dir.join("codex"));
        }
    }

    candidates
}

fn resolve_local_codex_executable() -> Option<String> {
    LOCAL_CODEX_EXECUTABLE
        .get_or_init(|| {
            local_codex_candidates()
                .into_iter()
                .map(|path| path.to_string_lossy().to_string())
                .next()
        })
        .clone()
}

fn resolve_host_codex_executable() -> Option<String> {
    HOST_CODEX_EXECUTABLE
        .get_or_init(|| {
            let lookup_script = r#"
for candidate in \
  "$HOME/.local/bin/codex" \
  "$HOME/.npm-global/bin/codex" \
  "$HOME/.local/share/npm/bin/codex" \
  "$HOME/.nvm/versions/node"/*/bin/codex \
  "$HOME/.volta/bin/codex" \
  "$HOME/.yarn/bin/codex" \
  "$HOME/.config/yarn/global/node_modules/.bin/codex" \
  "/usr/local/bin/codex" \
  "/usr/bin/codex"
do
  if [ -x "$candidate" ]; then
    printf '%s\n' "$candidate"
    exit 0
  fi
done
command -v codex 2>/dev/null || true
"#;

            let output = Command::new("flatpak-spawn")
                .arg("--host")
                .arg("sh")
                .arg("-lc")
                .arg(lookup_script)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }

            let resolved = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())?
                .to_string();

            Some(resolved)
        })
        .clone()
}

pub fn build_codex_command(home_dir: Option<&Path>) -> Result<Command, String> {
    if running_in_flatpak() {
        let mut host_command = Command::new("flatpak-spawn");
        host_command.arg("--host");
        configure_codex_env(&mut host_command, home_dir, true);
        if let Some(host_codex) = resolve_host_codex_executable() {
            host_command.arg(host_codex);
        } else {
            host_command
                .arg("bash")
                .arg("-lc")
                .arg("exec codex \"$@\"")
                .arg("_");
        }
        Ok(host_command)
    } else {
        let mut local_command = if let Some(local_codex) = resolve_local_codex_executable() {
            Command::new(local_codex)
        } else {
            let mut shell_command = Command::new("bash");
            shell_command.arg("-lc").arg("exec codex \"$@\"").arg("_");
            shell_command
        };
        configure_codex_env(&mut local_command, home_dir, false);
        Ok(local_command)
    }
}

pub fn cli_available() -> bool {
    let mut command = match build_codex_command(None) {
        Ok(command) => command,
        Err(_) => return false,
    };
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn configure_codex_env(command: &mut Command, home_dir: Option<&Path>, via_flatpak_host: bool) {
    let Some(home_dir) = home_dir else {
        return;
    };

    let home_str = home_dir.to_string_lossy().to_string();
    let xdg_data_home = home_dir.join("data");
    let xdg_config_home = home_dir.join("config");
    let xdg_cache_home = home_dir.join("cache");

    let _ = std::fs::create_dir_all(home_dir);
    let _ = std::fs::create_dir_all(&xdg_data_home);
    let _ = std::fs::create_dir_all(&xdg_config_home);
    let _ = std::fs::create_dir_all(&xdg_cache_home);

    if via_flatpak_host {
        command.arg(format!("--env=HOME={home_str}"));
        command.arg(format!(
            "--env=XDG_DATA_HOME={}",
            xdg_data_home.to_string_lossy()
        ));
        command.arg(format!(
            "--env=XDG_CONFIG_HOME={}",
            xdg_config_home.to_string_lossy()
        ));
        command.arg(format!(
            "--env=XDG_CACHE_HOME={}",
            xdg_cache_home.to_string_lossy()
        ));
    } else {
        command.env("HOME", &home_str);
        command.env("XDG_DATA_HOME", xdg_data_home.to_string_lossy().to_string());
        command.env(
            "XDG_CONFIG_HOME",
            xdg_config_home.to_string_lossy().to_string(),
        );
        command.env(
            "XDG_CACHE_HOME",
            xdg_cache_home.to_string_lossy().to_string(),
        );
    }
}
