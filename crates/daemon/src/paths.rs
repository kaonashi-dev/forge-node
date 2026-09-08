//! Filesystem paths: socket + lockfile (ADR-004) and data/config/runtime dirs
//! (§15.1).
//!
//! The socket path is the delicate one: a Unix domain socket path must fit in
//! `sun_path` (104 bytes on macOS, 108 on Linux). We require the *full* path to
//! be under 100 bytes and fall back to `/tmp/forge-$UID/…` when the preferred
//! location would be too long, failing explicitly rather than silently
//! truncating (ADR-004).

use std::path::PathBuf;

/// Filesystem namespace for ProjectDirs (not the display name "Forge Node").
pub const APP_NAME: &str = "Forge";
/// Directory / socket namespace (resolves the plan's `{app}`).
pub const APP_DIR: &str = "forge";
/// Conservative cap on the socket path length (ADR-004: sun_path ≤ 104 on macOS).
pub const MAX_SOCKET_PATH_LEN: usize = 100;

/// Errors resolving paths.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("socket path too long: {len} bytes (max {max}) at {path}")]
    SocketPathTooLong {
        path: String,
        len: usize,
        max: usize,
    },
    #[error("no writable runtime directory found for the daemon socket")]
    NoRuntimeDir,
    #[error("refusing to use {path}: {reason}")]
    NotPrivate { path: String, reason: String },
    #[error("could not determine platform data directories")]
    NoDataDir,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The current user's numeric id, used for the `/tmp` fallback namespace.
fn uid() -> u32 {
    nix::unistd::Uid::current().as_raw()
}

/// Preferred runtime directory that holds the socket and lockfile.
///
/// - Linux: `$XDG_RUNTIME_DIR/forge`, else `/tmp/forge-$UID`.
/// - macOS: `$TMPDIR/forge` (per-user), else `/tmp/forge-$UID`.
fn preferred_runtime_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            if !dir.is_empty() {
                return Some(PathBuf::from(dir).join(APP_DIR));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(dir) = std::env::var_os("TMPDIR") {
            if !dir.is_empty() {
                return Some(PathBuf::from(dir).join(APP_DIR));
            }
        }
    }
    None
}

/// The `/tmp/forge-$UID` fallback runtime directory (ADR-004).
fn fallback_runtime_dir() -> PathBuf {
    PathBuf::from(format!("/tmp/{APP_DIR}-{}", uid()))
}

/// Resolve the runtime directory whose socket path fits [`MAX_SOCKET_PATH_LEN`].
///
/// Returns the directory and the socket path within it. Tries the preferred
/// location first, then the `/tmp` fallback, and errors if neither fits.
pub fn resolve_runtime_dir() -> Result<PathBuf, PathError> {
    let candidates = preferred_runtime_dir()
        .into_iter()
        .chain(std::iter::once(fallback_runtime_dir()));

    let mut last_too_long: Option<PathError> = None;
    for dir in candidates {
        let sock = dir.join("daemon.sock");
        let len = sock.as_os_str().len();
        if len < MAX_SOCKET_PATH_LEN {
            return Ok(dir);
        }
        last_too_long = Some(PathError::SocketPathTooLong {
            path: sock.display().to_string(),
            len,
            max: MAX_SOCKET_PATH_LEN,
        });
    }
    Err(last_too_long.unwrap_or(PathError::NoRuntimeDir))
}

/// Absolute path to the daemon socket (validated length, ADR-004).
///
/// When `FORGE_SOCKET` is set, that path is used as-is (same override the GUI
/// honors), so a CLI or test can point at a temporary socket without touching
/// the developer's singleton.
pub fn socket_path() -> Result<PathBuf, PathError> {
    socket_path_from(std::env::var_os("FORGE_SOCKET"))
}

/// Resolve the socket given an optional `FORGE_SOCKET`-style override.
fn socket_path_from(forge_socket: Option<std::ffi::OsString>) -> Result<PathBuf, PathError> {
    if let Some(path) = forge_socket {
        return Ok(PathBuf::from(path));
    }
    Ok(resolve_runtime_dir()?.join("daemon.sock"))
}

/// Absolute path to the singleton lockfile, next to the socket (§9.2).
pub fn lock_path() -> Result<PathBuf, PathError> {
    Ok(resolve_runtime_dir()?.join("daemon.lock"))
}

/// Platform data directory root (`~/Library/Application Support/Forge` on macOS,
/// `$XDG_DATA_HOME/forge` on Linux) (§15.1).
///
/// When `FORGE_DATA_DIR` is set and non-empty, that directory is used as-is:
/// a dev daemon runs against scratch state instead of the daily database.
pub fn data_dir() -> Result<PathBuf, PathError> {
    if let Some(dir) = std::env::var_os("FORGE_DATA_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let dirs = directories::ProjectDirs::from("", "", APP_NAME).ok_or(PathError::NoDataDir)?;
    Ok(dirs.data_dir().to_path_buf())
}

/// Platform config directory root (§15.1).
///
/// Same override shape as [`data_dir`]: `FORGE_CONFIG_DIR` points a dev
/// daemon at a scratch config instead of the daily one.
pub fn config_dir() -> Result<PathBuf, PathError> {
    if let Some(dir) = std::env::var_os("FORGE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let dirs = directories::ProjectDirs::from("", "", APP_NAME).ok_or(PathError::NoDataDir)?;
    Ok(dirs.config_dir().to_path_buf())
}

/// `config.toml` path (§15.4).
pub fn config_file() -> Result<PathBuf, PathError> {
    Ok(config_dir()?.join("config.toml"))
}

/// SQLite database path (§15.1).
pub fn db_path() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("app.db"))
}

/// Default managed-worktree root (§14.2).
pub fn worktrees_root() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("worktrees"))
}

/// Logs directory (`app.log`, `daemon.log`) (§15.1).
pub fn logs_dir() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("logs"))
}

/// Create `dir` with `0700` permissions, or verify an existing one is private
/// (ADR-004).
///
/// A pre-existing directory is *not* trusted. The `/tmp/{app}-$UID` fallback
/// lives in a world-writable directory, so another local user can create it
/// first; the daemon would then bind its socket inside a directory that user
/// owns, and they could unlink it and bind their own in its place. Since the
/// path already exists, the `0700` mode on `DirBuilder` never applied either.
///
/// So: a directory we do not own is refused, and one we own but that is group- or
/// world-accessible is tightened back to `0700`.
pub fn ensure_private_dir(dir: &std::path::Path) -> Result<(), PathError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let meta = match std::fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            return Ok(());
        }
        Err(e) => return Err(PathError::Io(e)),
    };

    // A symlink here would redirect the socket somewhere we never checked.
    if !meta.is_dir() {
        return Err(PathError::NotPrivate {
            path: dir.display().to_string(),
            reason: "exists but is not a directory".to_string(),
        });
    }
    let owner = meta.uid();
    if owner != uid() {
        return Err(PathError::NotPrivate {
            path: dir.display().to_string(),
            reason: format!("owned by uid {owner}, expected {}", uid()),
        });
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        tracing::warn!(
            path = %dir.display(),
            found = format!("{mode:o}"),
            "runtime directory was not private; tightening to 0700"
        );
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn socket_path_is_within_length_budget() {
        // On any sane test host at least the /tmp fallback must fit.
        let sock = socket_path_from(None).expect("a socket path should resolve");
        assert!(
            sock.as_os_str().len() < MAX_SOCKET_PATH_LEN,
            "socket path {} is {} bytes",
            sock.display(),
            sock.as_os_str().len()
        );
        assert!(sock.ends_with("daemon.sock"));
    }

    #[test]
    fn forge_socket_override_wins_over_default_resolution() {
        let sock = socket_path_from(Some("/tmp/forge-stats-test.sock".into())).unwrap();
        assert_eq!(sock, PathBuf::from("/tmp/forge-stats-test.sock"));
    }

    #[test]
    fn lock_sits_next_to_socket() {
        let sock = socket_path_from(None).unwrap();
        let lock = lock_path().unwrap();
        assert_eq!(sock.parent(), lock.parent());
        assert!(lock.ends_with("daemon.lock"));
    }

    #[test]
    fn ensure_private_dir_creates_and_tightens() {
        let tmp = tempfile::tempdir().unwrap();

        // Created fresh: 0700 from the start.
        let fresh = tmp.path().join("fresh");
        ensure_private_dir(&fresh).unwrap();
        assert_eq!(mode_of(&fresh), 0o700);

        // Pre-existing and group/world-accessible: tightened, not left as-is.
        let loose = tmp.path().join("loose");
        std::fs::create_dir(&loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o777)).unwrap();
        ensure_private_dir(&loose).unwrap();
        assert_eq!(mode_of(&loose), 0o700, "an existing dir must be tightened");

        // A file (or symlink) where a directory belongs is refused outright.
        let not_a_dir = tmp.path().join("file");
        std::fs::write(&not_a_dir, b"x").unwrap();
        assert!(matches!(
            ensure_private_dir(&not_a_dir),
            Err(PathError::NotPrivate { .. })
        ));
    }

    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn fallback_dir_is_uid_namespaced() {
        let d = fallback_runtime_dir();
        assert!(d.to_string_lossy().contains(&format!("{APP_DIR}-")));
    }

    /// Restores the previous value on drop, so a failing assert cannot leak
    /// an override into a neighbouring test in the same process.
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn data_and_config_dirs_honor_forge_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let config = tmp.path().join("cfg");
        let _data = EnvGuard::set("FORGE_DATA_DIR", &data);
        let _config = EnvGuard::set("FORGE_CONFIG_DIR", &config);
        assert_eq!(data_dir().unwrap(), data);
        assert_eq!(config_dir().unwrap(), config);
        assert_eq!(db_path().unwrap(), data.join("app.db"));
        assert_eq!(config_file().unwrap(), config.join("config.toml"));
        assert_eq!(worktrees_root().unwrap(), data.join("worktrees"));
        // One test, not two: Rust runs tests on threads sharing one
        // environment, so a second test touching the same variable races this
        // one.
        std::env::set_var("FORGE_DATA_DIR", "");
        assert_ne!(data_dir().unwrap(), PathBuf::from(""));
    }
}
