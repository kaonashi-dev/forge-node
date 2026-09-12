//! Worktrees a project asked Forge to forget (§14.4).
//!
//! A worktree made by another tool — a parallel agent's scratch checkout, say —
//! is adopted by the rescan, and dropping its row only lasts until the next
//! sweep. A [`WorktreeIgnore`] records the decision so the rescan leaves the
//! path alone; `RemoveWorktree` writes one itself for the worktree it forgets.
//!
//! These rules are policy: they never touch the disk. They can be written from
//! the GUI or derived from `[worktrees] ignore` in `config.toml`.

use crate::ids::{ProjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How far an ignore rule reaches from its path (§14.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IgnoreScope {
    /// The worktree at exactly this path.
    Exact,
    /// Every worktree under this directory, at any depth.
    Subtree,
}

/// One worktree-ignore rule of one project (§14.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeIgnore {
    /// The project whose rescan consults this rule.
    pub project_id: ProjectId,
    /// One path has one rule. Absolute and resolved when it was written.
    pub path: PathBuf,
    /// Whether the rule covers the path itself or its whole subtree.
    pub scope: IgnoreScope,
    /// When the rule was first written.
    pub created_at: Timestamp,
}

/// Caps one project's rule set, like `MAX_SHARE_RULES`.
pub const MAX_WORKTREE_IGNORES: usize = 200;
