//! Resolved filesystem layout, carried over the wire by `GetPaths`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which layout the daemon resolved at startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Layout {
    /// `~/.forge-node` for config and state.
    #[default]
    Home,
    /// Today's platform directories, kept until the user migrates.
    Legacy,
    /// A layout this build does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

/// Every path Forge resolves, from one pure function.
///
/// Runtime-only: never persisted, never a SQLite column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgePaths {
    /// Which layout produced these paths.
    pub layout: Layout,
    /// Config and state root, `~/.forge-node` unless overridden.
    pub home: PathBuf,
    /// The user's `config.toml`.
    pub config_file: PathBuf,
    /// The SQLite metadata database.
    pub db: PathBuf,
    /// `daemon.<date>.log` lives here.
    pub logs: PathBuf,
    /// Orchestration result files (`<run>/<attempt>.md`).
    pub runs: PathBuf,
    /// Regenerated agent attention assets.
    pub agent_plugins: PathBuf,
    /// The worktrees root when `[worktrees] root` is empty.
    pub default_worktrees_root: PathBuf,
    /// The legacy data directory, when one still exists.
    pub legacy: Option<PathBuf>,
}
