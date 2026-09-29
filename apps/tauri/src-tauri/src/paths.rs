//! Per-user config, data and log directories for the Tauri host.
//!
//! Resolved like the daemon's (`crates/daemon/src/paths.rs`) without depending
//! on that crate: `FORGE_CONFIG_DIR` / `FORGE_DATA_DIR` when set, else
//! `directories` with an empty qualifier and organization and application
//! `Forge`, a short stable namespace though the product displays as **Forge
//! Node**. The host only shows and reveals these paths; the daemon reads
//! `config.toml` and writes `logs/daemon.<date>.log`.
//!
//! ```text
//! config: ~/Library/Application Support/Forge/config.toml | $XDG_CONFIG_HOME/forge/config.toml
//! data:   ~/Library/Application Support/Forge/            | $XDG_DATA_HOME/forge/
//! ```

use directories::ProjectDirs;
use std::path::PathBuf;

/// Filesystem namespace for ProjectDirs (not the display name "Forge Node").
pub const APP_NAME: &str = "Forge";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", APP_NAME)
}

/// `value` when it is set and non-empty, as the daemon reads its overrides, so
/// a dev daemon started by `scripts/dev daemon` and this host name the same
/// scratch state.
fn overridden(
    value: Option<std::ffi::OsString>,
    fallback: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match value {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => fallback(),
    }
}

/// Platform config directory root, or `None` on a platform with no
/// home directory for this user. Absence is a readout, never a failure: the
/// settings screen says so and the app keeps running.
pub fn config_dir() -> Option<PathBuf> {
    overridden(std::env::var_os("FORGE_CONFIG_DIR"), || {
        Some(project_dirs()?.config_dir().to_path_buf())
    })
}

/// `config.toml` path, the one the daemon reads.
pub fn config_file() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

pub fn data_dir() -> Option<PathBuf> {
    overridden(std::env::var_os("FORGE_DATA_DIR"), || {
        Some(project_dirs()?.data_dir().to_path_buf())
    })
}

/// Where the daemon writes `daemon.<date>.log`.
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
        // Case-insensitively: `directories` keeps the name as given on macOS
        // but lowercases it for XDG, so the same triple yields `.../Forge/` on
        // one platform and `.../forge/` on the other. What must hold is the
        // namespace, not its spelling.
        assert!(
            file.to_string_lossy()
                .to_lowercase()
                .contains(&APP_NAME.to_lowercase()),
            "{} should be namespaced under {APP_NAME}",
            file.display()
        );
    }

    #[test]
    fn an_empty_override_is_no_override() {
        use std::ffi::OsString;
        let fallback = || Some(PathBuf::from("/default"));
        assert_eq!(
            overridden(Some(OsString::from("/scratch")), fallback),
            Some(PathBuf::from("/scratch"))
        );
        assert_eq!(
            overridden(Some(OsString::new()), fallback),
            Some(PathBuf::from("/default"))
        );
        assert_eq!(overridden(None, fallback), Some(PathBuf::from("/default")));
    }

    #[test]
    fn logs_live_under_the_data_dir() {
        let logs = logs_dir().expect("a logs path should resolve");
        assert!(logs.ends_with("logs"));
        assert_eq!(logs.parent(), data_dir().as_deref());
    }
}
