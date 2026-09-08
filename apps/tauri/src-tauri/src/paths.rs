//! Per-user config and data directories for the Tauri host (§15.1).
//!
//! Resolved with `directories` using the same qualifier/organization/application
//! triple as the daemon (`crates/daemon/src/paths.rs`): empty qualifier, empty
//! organization, application `Forge`. That path namespace stays short and stable
//! even though the product displays as **Forge Node**. All processes must agree,
//! because they read the same `config.toml`:
//!
//! ```text
//! config: ~/Library/Application Support/Forge/config.toml | $XDG_CONFIG_HOME/Forge/config.toml
//! data:   ~/Library/Application Support/Forge/            | $XDG_DATA_HOME/Forge/
//!         └── logs/  (app.log, daemon.log)
//! ```
//!
//! The host never depends on a second GUI crate (ADR-002),
//! so the resolution is duplicated here rather than imported — exactly as the
//! daemon already duplicates it.

use directories::ProjectDirs;
use std::path::PathBuf;

/// Filesystem namespace for ProjectDirs (not the display name "Forge Node").
pub const APP_NAME: &str = "Forge";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", APP_NAME)
}

/// Platform config directory root (§15.1), or `None` on a platform with no
/// home directory for this user. Absence is a readout, never a failure: the
/// settings screen says so and the app keeps running.
pub fn config_dir() -> Option<PathBuf> {
    Some(project_dirs()?.config_dir().to_path_buf())
}

/// `config.toml` path, shared with the daemon (§15.4).
pub fn config_file() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

/// Platform data directory root (§15.1).
pub fn data_dir() -> Option<PathBuf> {
    Some(project_dirs()?.data_dir().to_path_buf())
}

/// Logs directory holding `app.log` next to `daemon.log` (§15.1).
pub fn logs_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("logs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_lives_under_the_forge_config_dir() {
        let file = config_file().expect("a config path should resolve");
        assert!(file.ends_with("config.toml"));
        assert_eq!(file.parent(), config_dir().as_deref());
        assert!(
            file.to_string_lossy().contains(APP_NAME),
            "{} should be namespaced under {APP_NAME}",
            file.display()
        );
    }

    #[test]
    fn logs_live_under_the_data_dir() {
        let logs = logs_dir().expect("a logs path should resolve");
        assert!(logs.ends_with("logs"));
        assert_eq!(logs.parent(), data_dir().as_deref());
    }
}
