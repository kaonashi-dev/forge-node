//! Workspace domain type (§7.2).

use crate::ids::{ProjectId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The kind of workspace a session runs in. The main checkout is a `Workspace`
/// like any other — never a special case elsewhere in the code (P4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WorkspaceKind {
    /// The repository's main checkout, or a plain (non-Git) project directory.
    Main,
    /// A Git worktree.
    GitWorktree,
    // FutureRemote is added when remote workspaces exist.
}

/// What `git status` last said about a workspace's working tree (§14.1).
///
/// **Runtime state, never a column** — the same rule as `Session::terminal_id`
/// and `Session::last_activity_at`. It is recomputed by
/// `RefreshWorkspaceStatus`, and a workspace loaded from SQLite starts with
/// [`WorkspaceStatus::default`] (all `None`/`false`), which reads as "not
/// measured yet" rather than "clean". The `serde` default keeps a client built
/// before this field existed decodable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceStatus {
    /// At least one tracked change or untracked file.
    pub dirty: bool,
    /// Commits ahead of upstream, when an upstream is configured.
    pub ahead: Option<u32>,
    /// Commits behind upstream, when an upstream is configured.
    pub behind: Option<u32>,
    /// When the status was last read. `None` means never.
    pub measured_at: Option<Timestamp>,
}

impl WorkspaceStatus {
    /// Whether git has been asked at all yet.
    #[must_use]
    pub fn is_measured(&self) -> bool {
        self.measured_at.is_some()
    }

    /// Whether there is anything worth drawing next to the branch name.
    #[must_use]
    pub fn has_signal(&self) -> bool {
        self.dirty || self.ahead.is_some_and(|n| n > 0) || self.behind.is_some_and(|n| n > 0)
    }
}

/// A directory where sessions actually run: main checkout or worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub project_id: ProjectId,
    pub kind: WorkspaceKind,
    pub path: PathBuf,
    pub branch: Option<String>,
    /// Optional human label, independent of the git branch.
    ///
    /// When set, the sidebar leads with this and keeps the branch as a
    /// secondary line — the same split Orca's `displayName` vs `branch` gives
    /// a reader scanning several worktrees of one project. `None` falls back
    /// to the branch (or the directory name when detached). Cleared by
    /// `RenameWorkspace` with an empty string. `serde(default)` keeps a
    /// snapshot written before this field existed decodable.
    #[serde(default)]
    pub display_name: Option<String>,
    /// `true` only for worktrees created by Forge itself (§14.4 safety).
    pub managed_by_app: bool,
    pub created_at: Timestamp,
    /// Last known working-tree status. Runtime-only; see [`WorkspaceStatus`].
    #[serde(default)]
    pub status: WorkspaceStatus,
}

impl Workspace {
    #[must_use]
    pub fn is_worktree(&self) -> bool {
        matches!(self.kind, WorkspaceKind::GitWorktree)
    }

    /// What the sidebar (and any other scanner) should lead with.
    ///
    /// Prefers a user-chosen [`Self::display_name`], then the branch, then the
    /// directory basename for a detached worktree, then a plain `"main"` for a
    /// main checkout that somehow has none of those.
    #[must_use]
    pub fn label(&self) -> String {
        if let Some(name) = self
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            return name.to_owned();
        }
        match (&self.branch, self.kind) {
            (Some(branch), _) => branch.clone(),
            (None, WorkspaceKind::GitWorktree) => self
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| self.path.display().to_string()),
            (None, _) => "main".to_string(),
        }
    }

    /// The git branch to show under [`Self::label`], when it would add
    /// information.
    ///
    /// Returns `None` when the label *is* the branch (or there is no branch):
    /// repeating `hind` under `hind` is noise, not orientation.
    #[must_use]
    pub fn secondary_label(&self) -> Option<&str> {
        let branch = self.branch.as_deref()?;
        let primary = self
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        match primary {
            Some(name) if name != branch => Some(branch),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProjectId;

    fn workspace(branch: Option<&str>, display_name: Option<&str>) -> Workspace {
        Workspace {
            id: WorkspaceId::new(),
            project_id: ProjectId::new(),
            kind: WorkspaceKind::GitWorktree,
            path: PathBuf::from("/tmp/hind"),
            branch: branch.map(str::to_owned),
            display_name: display_name.map(str::to_owned),
            managed_by_app: true,
            created_at: Timestamp::now(),
            status: WorkspaceStatus::default(),
        }
    }

    #[test]
    fn label_prefers_display_name_then_branch() {
        assert_eq!(
            workspace(Some("kaonashi-dev/hind"), Some("Fix payments")).label(),
            "Fix payments"
        );
        assert_eq!(
            workspace(Some("kaonashi-dev/hind"), None).label(),
            "kaonashi-dev/hind"
        );
    }

    #[test]
    fn secondary_label_only_when_display_name_differs() {
        assert_eq!(
            workspace(Some("kaonashi-dev/hind"), Some("Fix payments")).secondary_label(),
            Some("kaonashi-dev/hind")
        );
        assert_eq!(
            workspace(Some("hind"), Some("hind")).secondary_label(),
            None
        );
        assert_eq!(workspace(Some("hind"), None).secondary_label(), None);
    }
}
