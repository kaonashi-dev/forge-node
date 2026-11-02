//! Socket and storage-path resolution shared by the CLI, the daemon, and the
//! GUI locator.
//!
//! `FORGE_SOCKET` wins for the socket. Otherwise the platform runtime directory
//! is used when the full path fits in `sun_path`, and `/tmp/forge-$UID/daemon.sock`
//! otherwise. [`resolve`] is the one pure function that decides the config and
//! state home, the default worktrees root, and whether a legacy install still
//! owns the paths.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use protocol::{ForgePaths, Layout};

pub const MAX_SOCKET_PATH_LEN: usize = 100;

#[derive(Clone, Debug)]
pub struct SocketQuery {
    pub forge_socket: Option<PathBuf>,
    pub tmpdir: Option<PathBuf>,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub uid: u32,
}

impl SocketQuery {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            forge_socket: std::env::var_os("FORGE_SOCKET").map(PathBuf::from),
            tmpdir: std::env::var_os("TMPDIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            uid: current_uid(),
        }
    }
}

/// Why a daemon socket path could not be resolved.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SocketPathError {
    /// No candidate path fits in a Unix socket address.
    #[error("daemon socket path is too long: {0}")]
    TooLong(PathBuf),
}

/// The socket a client should open.
pub fn resolve_socket(query: &SocketQuery) -> Result<PathBuf, SocketPathError> {
    if let Some(path) = &query.forge_socket {
        return Ok(path.clone());
    }
    #[cfg(target_os = "linux")]
    if let Some(dir) = &query.xdg_runtime_dir {
        let path = dir.join("forge/daemon.sock");
        if fits(&path) {
            return Ok(path);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(dir) = &query.tmpdir {
        let path = dir.join("forge/daemon.sock");
        if fits(&path) {
            return Ok(path);
        }
    }
    let path = PathBuf::from(format!("/tmp/forge-{}/daemon.sock", query.uid));
    if fits(&path) {
        Ok(path)
    } else {
        Err(SocketPathError::TooLong(path))
    }
}

pub fn socket_path() -> Result<PathBuf, SocketPathError> {
    resolve_socket(&SocketQuery::from_env())
}

fn fits(path: &Path) -> bool {
    path.as_os_str().len() < MAX_SOCKET_PATH_LEN
}

fn current_uid() -> u32 {
    nix::unistd::Uid::current().as_raw()
}

/// The legacy platform directories, probed once before resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyDirs {
    /// `~/Library/Application Support/Forge` or `$XDG_DATA_HOME/forge`.
    pub data: PathBuf,
    /// `~/Library/Application Support/Forge` or `$XDG_CONFIG_HOME/forge`.
    pub config: PathBuf,
}

/// Everything [`resolve`] needs, with no environment or filesystem reads.
#[derive(Clone, Debug, Default)]
pub struct PathInputs {
    /// The user's home directory (`directories::BaseDirs`).
    pub home: Option<PathBuf>,
    /// `FORGE_HOME`.
    pub forge_home: Option<OsString>,
    /// `FORGE_DATA_DIR`, read for compatibility.
    pub data_dir: Option<OsString>,
    /// `FORGE_CONFIG_DIR`, read for compatibility.
    pub config_dir: Option<OsString>,
    /// Present only while the legacy data directory still holds an `app.db`.
    pub legacy: Option<LegacyDirs>,
    /// Whether `$HOME/.forge-node/app.db` already exists.
    pub home_db_exists: bool,
}

impl PathInputs {
    /// Read the environment and probe the two `app.db` locations.
    #[must_use]
    pub fn from_env() -> Self {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        let legacy = directories::ProjectDirs::from("", "", "Forge").and_then(|dirs| {
            let data = dirs.data_dir().to_path_buf();
            data.join("app.db").is_file().then(|| LegacyDirs {
                data,
                config: dirs.config_dir().to_path_buf(),
            })
        });
        let home_db_exists = home
            .as_ref()
            .is_some_and(|home| home.join(".forge-node").join("app.db").is_file());
        Self {
            home,
            forge_home: non_empty(std::env::var_os("FORGE_HOME")),
            data_dir: non_empty(std::env::var_os("FORGE_DATA_DIR")),
            config_dir: non_empty(std::env::var_os("FORGE_CONFIG_DIR")),
            legacy,
            home_db_exists,
        }
    }
}

/// Why a Forge path could not be resolved.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PathError {
    /// `FORGE_HOME` must be absolute, or start with `~/`.
    #[error("FORGE_HOME must be an absolute path or start with `~/`, got {0:?}")]
    RelativeForgeHome(PathBuf),
    /// The default layout needs a home directory.
    #[error("could not determine the user's home directory")]
    NoHome,
}

/// A `[worktrees] root` that is neither absolute nor `~`-anchored.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("worktrees root must be absolute or start with `~/`: {0:?}")]
pub struct RelativeRoot(pub String);

/// Resolve every Forge path from these inputs.
///
/// # Errors
/// [`PathError::RelativeForgeHome`] for a relative `FORGE_HOME`, and
/// [`PathError::NoHome`] when the default layout has no home directory.
pub fn resolve(inputs: &PathInputs) -> Result<ForgePaths, PathError> {
    if let Some(raw) = inputs
        .forge_home
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        let raw = PathBuf::from(raw);
        let text = raw.to_string_lossy();
        let home = if let Some(rest) = text.strip_prefix("~/") {
            let user_home = inputs.home.as_deref().ok_or(PathError::NoHome)?;
            user_home.join(rest)
        } else {
            raw.clone()
        };
        if !home.is_absolute() {
            return Err(PathError::RelativeForgeHome(raw));
        }
        return Ok(home_layout(home.clone(), home.join("worktrees"), None));
    }

    let mut paths = match (&inputs.legacy, inputs.home_db_exists) {
        (Some(legacy), false) => legacy_layout(legacy),
        _ => {
            let user_home = inputs.home.as_deref().ok_or(PathError::NoHome)?;
            home_layout(
                user_home.join(".forge-node"),
                user_home.join("forge-node").join("worktrees"),
                inputs.legacy.as_ref().map(|legacy| legacy.data.clone()),
            )
        }
    };

    if let Some(raw) = inputs.data_dir.as_deref().filter(|value| !value.is_empty()) {
        let home = PathBuf::from(raw);
        paths.layout = Layout::Home;
        paths.db = home.join("app.db");
        paths.logs = home.join("logs");
        paths.runs = home.join("runs");
        paths.agent_plugins = home.join("agent-plugins");
        paths.default_worktrees_root = home.join("worktrees");
        paths.legacy = None;
        paths.home = home;
    }
    if let Some(raw) = inputs
        .config_dir
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        paths.config_file = PathBuf::from(raw).join("config.toml");
    }
    Ok(paths)
}

/// The worktrees root the daemon will actually use.
///
/// A trimmed-empty `configured` value selects `default`, `~` and `~/…` expand
/// against `home`, and an absolute value is used as written.
///
/// # Errors
/// [`RelativeRoot`] when the value is relative (or `~`-anchored with no known
/// home directory).
pub fn worktrees_root(
    configured: &str,
    home: Option<&Path>,
    default: &Path,
) -> Result<PathBuf, RelativeRoot> {
    let value = configured.trim();
    if value.is_empty() {
        return Ok(default.to_path_buf());
    }
    if value == "~" {
        return home
            .map(Path::to_path_buf)
            .ok_or_else(|| RelativeRoot(value.to_owned()));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home
            .map(|home| home.join(rest))
            .ok_or_else(|| RelativeRoot(value.to_owned()));
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(RelativeRoot(value.to_owned()))
    }
}

fn home_layout(home: PathBuf, worktrees: PathBuf, legacy: Option<PathBuf>) -> ForgePaths {
    ForgePaths {
        layout: Layout::Home,
        config_file: home.join("config.toml"),
        db: home.join("app.db"),
        logs: home.join("logs"),
        runs: home.join("runs"),
        agent_plugins: home.join("agent-plugins"),
        default_worktrees_root: worktrees,
        legacy,
        home,
    }
}

fn legacy_layout(legacy: &LegacyDirs) -> ForgePaths {
    let home = legacy.data.clone();
    ForgePaths {
        layout: Layout::Legacy,
        config_file: legacy.config.join("config.toml"),
        db: home.join("app.db"),
        logs: home.join("logs"),
        runs: home.join("runs"),
        agent_plugins: home.join("agent-plugins"),
        default_worktrees_root: home.join("worktrees"),
        legacy: Some(home.clone()),
        home,
    }
}

fn non_empty(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use std::ffi::OsString;

    fn inputs(home: &Path) -> PathInputs {
        PathInputs {
            home: Some(home.to_path_buf()),
            ..PathInputs::default()
        }
    }

    #[test]
    fn resolve_defaults_to_mixed_home() {
        let home = Path::new("/Users/dev");
        let paths = resolve(&inputs(home)).expect("resolve");

        assert_eq!(paths.layout, Layout::Home);
        assert_eq!(paths.home, home.join(".forge-node"));
        assert_eq!(
            paths.default_worktrees_root,
            home.join("forge-node/worktrees")
        );
        assert_eq!(paths.config_file, home.join(".forge-node/config.toml"));
        assert_eq!(paths.db, home.join(".forge-node/app.db"));
        assert_eq!(paths.logs, home.join(".forge-node/logs"));
        assert_eq!(paths.runs, home.join(".forge-node/runs"));
        assert_eq!(paths.agent_plugins, home.join(".forge-node/agent-plugins"));
        assert_eq!(paths.legacy, None);
    }

    #[test]
    fn resolve_detects_legacy() {
        let home = Path::new("/Users/dev");
        let legacy = LegacyDirs {
            data: PathBuf::from("/Users/dev/Library/Application Support/Forge"),
            config: PathBuf::from("/Users/dev/Library/Application Support/Forge"),
        };
        let paths = resolve(&PathInputs {
            legacy: Some(legacy.clone()),
            ..inputs(home)
        })
        .expect("resolve");

        assert_eq!(paths.layout, Layout::Legacy);
        assert_eq!(paths.home, legacy.data);
        assert_eq!(paths.config_file, legacy.config.join("config.toml"));
        assert_eq!(paths.db, legacy.data.join("app.db"));
        assert_eq!(paths.logs, legacy.data.join("logs"));
        assert_eq!(paths.runs, legacy.data.join("runs"));
        assert_eq!(paths.agent_plugins, legacy.data.join("agent-plugins"));
        assert_eq!(paths.default_worktrees_root, legacy.data.join("worktrees"));
        assert_eq!(paths.legacy, Some(legacy.data));
    }

    #[test]
    fn a_home_database_ends_legacy_detection_and_keeps_the_old_path_visible() {
        let home = Path::new("/Users/dev");
        let legacy = LegacyDirs {
            data: PathBuf::from("/Users/dev/Library/Application Support/Forge"),
            config: PathBuf::from("/Users/dev/Library/Application Support/Forge"),
        };
        let paths = resolve(&PathInputs {
            legacy: Some(legacy.clone()),
            home_db_exists: true,
            ..inputs(home)
        })
        .expect("resolve");

        assert_eq!(paths.layout, Layout::Home);
        assert_eq!(paths.home, home.join(".forge-node"));
        assert_eq!(paths.legacy, Some(legacy.data));
    }

    #[test]
    fn forge_home_moves_everything() {
        let home = Path::new("/Users/dev");
        let paths = resolve(&PathInputs {
            forge_home: Some(OsString::from("/tmp/x")),
            legacy: Some(LegacyDirs {
                data: PathBuf::from("/legacy"),
                config: PathBuf::from("/legacy"),
            }),
            ..inputs(home)
        })
        .expect("resolve");

        assert_eq!(paths.layout, Layout::Home);
        assert_eq!(paths.home, PathBuf::from("/tmp/x"));
        assert_eq!(paths.config_file, PathBuf::from("/tmp/x/config.toml"));
        assert_eq!(paths.db, PathBuf::from("/tmp/x/app.db"));
        assert_eq!(paths.logs, PathBuf::from("/tmp/x/logs"));
        assert_eq!(paths.runs, PathBuf::from("/tmp/x/runs"));
        assert_eq!(paths.agent_plugins, PathBuf::from("/tmp/x/agent-plugins"));
        assert_eq!(
            paths.default_worktrees_root,
            PathBuf::from("/tmp/x/worktrees")
        );
        assert_eq!(paths.legacy, None);
    }

    #[test]
    fn forge_home_tilde_expands() {
        let home = Path::new("/Users/dev");
        let paths = resolve(&PathInputs {
            forge_home: Some(OsString::from("~/state")),
            ..inputs(home)
        })
        .expect("resolve");
        assert_eq!(paths.home, home.join("state"));
        assert_eq!(paths.default_worktrees_root, home.join("state/worktrees"));
    }

    #[test]
    fn relative_forge_home_is_an_error() {
        let error = resolve(&PathInputs {
            forge_home: Some(OsString::from("relative/path")),
            ..inputs(Path::new("/Users/dev"))
        })
        .expect_err("a relative FORGE_HOME must be refused");
        assert!(
            matches!(error, PathError::RelativeForgeHome(_)),
            "{error:?}"
        );
        assert!(error.to_string().contains("FORGE_HOME"));

        let bare = resolve(&PathInputs {
            forge_home: Some(OsString::from("~")),
            ..inputs(Path::new("/Users/dev"))
        });
        assert!(matches!(bare, Err(PathError::RelativeForgeHome(_))));
    }

    #[test]
    fn compat_data_dir_replaces_the_data_paths_and_the_lock() {
        let home = Path::new("/Users/dev");
        let paths = resolve(&PathInputs {
            data_dir: Some(OsString::from("/tmp/forge-data")),
            ..inputs(home)
        })
        .expect("resolve");

        assert_eq!(paths.layout, Layout::Home);
        assert_eq!(paths.home, PathBuf::from("/tmp/forge-data"));
        assert_eq!(paths.db, PathBuf::from("/tmp/forge-data/app.db"));
        assert_eq!(paths.logs, PathBuf::from("/tmp/forge-data/logs"));
        assert_eq!(paths.runs, PathBuf::from("/tmp/forge-data/runs"));
        assert_eq!(
            paths.agent_plugins,
            PathBuf::from("/tmp/forge-data/agent-plugins")
        );
        assert_eq!(
            paths.default_worktrees_root,
            PathBuf::from("/tmp/forge-data/worktrees")
        );
        assert_eq!(paths.config_file, home.join(".forge-node/config.toml"));
        assert_eq!(
            paths.home.join("app.db.lock"),
            Path::new("/tmp/forge-data/app.db.lock")
        );
        assert_eq!(paths.legacy, None);
    }

    #[test]
    fn compat_config_dir_replaces_only_the_config_file() {
        let home = Path::new("/Users/dev");
        let paths = resolve(&PathInputs {
            config_dir: Some(OsString::from("/tmp/forge-config")),
            ..inputs(home)
        })
        .expect("resolve");

        assert_eq!(paths.layout, Layout::Home);
        assert_eq!(paths.home, home.join(".forge-node"));
        assert_eq!(
            paths.config_file,
            PathBuf::from("/tmp/forge-config/config.toml")
        );
        assert_eq!(paths.db, home.join(".forge-node/app.db"));
    }

    #[test]
    fn compat_overrides_apply_together() {
        let paths = resolve(&PathInputs {
            data_dir: Some(OsString::from("/tmp/forge-data")),
            config_dir: Some(OsString::from("/tmp/forge-config")),
            ..inputs(Path::new("/Users/dev"))
        })
        .expect("resolve");

        assert_eq!(paths.home, PathBuf::from("/tmp/forge-data"));
        assert_eq!(
            paths.default_worktrees_root,
            PathBuf::from("/tmp/forge-data/worktrees")
        );
        assert_eq!(
            paths.config_file,
            PathBuf::from("/tmp/forge-config/config.toml")
        );
    }

    #[test]
    fn empty_overrides_count_as_unset() {
        let home = Path::new("/Users/dev");
        let paths = resolve(&PathInputs {
            forge_home: Some(OsString::new()),
            data_dir: Some(OsString::new()),
            config_dir: Some(OsString::new()),
            ..inputs(home)
        })
        .expect("resolve");
        assert_eq!(paths.home, home.join(".forge-node"));
    }

    #[test]
    fn worktrees_root_defaults_and_expands() {
        let home = Path::new("/Users/dev");
        let default = home.join("forge-node/worktrees");

        assert_eq!(
            worktrees_root("  ", Some(home), &default).expect("empty"),
            default
        );
        assert_eq!(
            worktrees_root("~/wt", Some(home), &default).expect("tilde"),
            home.join("wt")
        );
        assert_eq!(
            worktrees_root("/abs/wt", Some(home), &default).expect("absolute"),
            PathBuf::from("/abs/wt")
        );
        assert_eq!(
            worktrees_root("~", Some(home), &default).expect("bare tilde"),
            home
        );
        assert!(matches!(
            worktrees_root("relative", Some(home), &default),
            Err(RelativeRoot(_))
        ));
    }
}
