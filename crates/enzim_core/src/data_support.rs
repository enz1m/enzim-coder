use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROFILE_ICON_POOL: [&str; 15] = [
    "person-symbolic",
    "briefcase-symbolic",
    "laptop-symbolic",
    "computer-symbolic",
    "star-symbolic",
    "go-home-symbolic",
    "rocket-symbolic",
    "brain-symbolic",
    "chat-bubble-symbolic",
    "bookmark-symbolic",
    "folder-symbolic",
    "target-symbolic",
    "shield-symbolic",
    "globe-symbolic",
    "car-side-symbolic",
];

pub const PROFILE_HOME_OVERRIDE_ENV: &str = "ENZIMCODER_PROFILE_HOME_DIR";

pub fn default_app_data_dir() -> PathBuf {
    if let Some(home_override) = profile_home_override_dir() {
        return home_override;
    }

    if let Some(path) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        return path.join("enzimcoder");
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        return home.join(".local").join("share").join("enzimcoder");
    }

    PathBuf::from(".")
}

pub fn profile_home_override_dir() -> Option<PathBuf> {
    let raw = std::env::var_os(PROFILE_HOME_OVERRIDE_ENV)?;
    let path = PathBuf::from(raw);
    if path.to_string_lossy().trim().is_empty() {
        return None;
    }
    Some(path)
}

pub fn configured_profile_home_dir(app_data_dir: &Path) -> PathBuf {
    if let Some(path) = profile_home_override_dir() {
        return path;
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        return home;
    }

    app_data_dir.join("system_home")
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn format_relative_age(timestamp: i64) -> String {
    let now = unix_now();
    let diff = now.saturating_sub(timestamp);
    if diff < 60 {
        "now".to_string()
    } else if diff < 3_600 {
        format!("{}m", diff / 60)
    } else if diff < 86_400 {
        format!("{}h", diff / 3_600)
    } else if diff < 604_800 {
        format!("{}d", diff / 86_400)
    } else {
        format!("{}w", diff / 604_800)
    }
}
