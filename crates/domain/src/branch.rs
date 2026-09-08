//! Git refs as the GUI needs to see them (§14, branches plan §3).
//!
//! A [`BranchRef`] is one row of the branch picker: enough to sort the list, to
//! label it, and to know whether choosing it would fail before trying. It is a
//! *reported* value, never authoritative — the daemon re-reads git on every
//! `ListBranches` rather than caching, so a stale row is a stale answer to an
//! old question, not a wrong model.

use crate::ids::{Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Where a ref lives: in `refs/heads` or under some remote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RefScope {
    /// A local branch (`refs/heads/<name>`).
    Local,
    /// A remote-tracking branch (`refs/remotes/<remote>/<name>`).
    Remote {
        /// The remote it belongs to, e.g. `origin`.
        remote: String,
    },
}

/// One branch offered by the picker (§14.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRef {
    /// Short branch name, without `refs/heads/` or the remote prefix:
    /// `feature/auth` for both `refs/heads/feature/auth` and
    /// `refs/remotes/origin/feature/auth`.
    pub name: String,
    /// Local or remote, and which remote.
    pub scope: RefScope,
    /// The upstream this branch tracks (`origin/main`), when it has one.
    /// Always `None` for a remote ref — a remote-tracking branch is an
    /// upstream, it does not have one.
    pub upstream: Option<String>,
    /// Commit date of the tip, used to sort the list newest-first.
    pub committed_at: Option<Timestamp>,
    /// Subject line of the tip commit, as a hint in the picker.
    pub subject: Option<String>,
    /// The workspace that currently has this branch checked out, if any.
    ///
    /// Git refuses to check the same branch out twice, so a row with this set
    /// is offered *disabled*: the picker can say which worktree holds it
    /// instead of letting the user discover it as a `Conflict` error (§14.3).
    pub checked_out_in: Option<WorkspaceId>,
}

impl BranchRef {
    /// Whether this ref lives on a remote.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        matches!(self.scope, RefScope::Remote { .. })
    }

    /// The name to pass as a git start-point: `origin/feature/x` for a remote
    /// ref, the bare name for a local one.
    #[must_use]
    pub fn start_point(&self) -> String {
        match &self.scope {
            RefScope::Local => self.name.clone(),
            RefScope::Remote { remote } => format!("{remote}/{}", self.name),
        }
    }
}

/// A configured git remote (§14.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// The remote's name, e.g. `origin`.
    pub name: String,
    /// Its fetch URL.
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_point_qualifies_remote_refs_only() {
        let local = BranchRef {
            name: "main".into(),
            scope: RefScope::Local,
            upstream: Some("origin/main".into()),
            committed_at: None,
            subject: None,
            checked_out_in: None,
        };
        assert_eq!(local.start_point(), "main");
        assert!(!local.is_remote());

        let remote = BranchRef {
            name: "feature/auth".into(),
            scope: RefScope::Remote {
                remote: "origin".into(),
            },
            upstream: None,
            committed_at: None,
            subject: None,
            checked_out_in: None,
        };
        assert_eq!(remote.start_point(), "origin/feature/auth");
        assert!(remote.is_remote());
    }
}
