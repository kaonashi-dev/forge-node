//! The state of a stopped rebase, merge, cherry-pick or revert (§14).
//!
//! Runtime-only, like [`crate::diff::WorkspaceDiff`] and
//! [`crate::workspace::WorkspaceStatus`]: no column, no migration, no `Store`
//! field. Which paths are still unmerged is a question only the index can
//! answer, and an answer that came out of a database would describe a
//! resolution the user finished ten minutes ago.

use serde::{Deserialize, Serialize};

use crate::ids::WorkspaceId;

/// Which operation is stopped in a checkout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SequencerOp {
    /// `git rebase`.
    Rebase,
    /// `git merge` stopped on conflicts.
    Merge,
    /// `git cherry-pick`.
    CherryPick,
    /// `git revert`.
    Revert,
    /// Unknown variant from a newer peer.
    #[serde(other)]
    Unknown,
}

impl SequencerOp {
    /// What the GUI calls this operation.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Rebase => "Rebase",
            Self::Merge => "Merge",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
            Self::Unknown => "Operation",
        }
    }
}

/// One unmerged path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictFile {
    /// Path relative to the checkout root, exactly as git spells it.
    pub path: String,
    /// Git's porcelain `XY` pair (`UU`, `DU`, `AA`, …).
    pub code: String,
}

impl ConflictFile {
    /// Git's own words for what the two sides did, for a dense list.
    ///
    /// The unfamiliar pairs are the ones worth spelling out: `UU` is the case
    /// everyone recognises, `DU` and `AA` are the ones a reader misreads as an
    /// ordinary delete or add.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self.code.as_str() {
            "UU" => "both modified",
            "AA" => "both added",
            "DD" => "both deleted",
            "AU" => "added by us",
            "UA" => "added by them",
            "DU" => "deleted by us",
            "UD" => "deleted by them",
            _ => "unmerged",
        }
    }
}

/// What one checkout's sequencer looks like right now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RebaseState {
    /// Which checkout this describes.
    pub workspace_id: WorkspaceId,
    /// The operation in progress, or `None` when nothing is stopped.
    pub operation: Option<SequencerOp>,
    /// Short object id of `HEAD` — detached while a rebase replays.
    pub head: Option<String>,
    /// The branch being replayed, or the checked-out one when idle.
    pub branch: Option<String>,
    /// Short id of the commit the replay lands on.
    pub onto: Option<String>,
    /// Which commit of the replay stopped, 1-based.
    pub step: Option<u32>,
    /// How many commits the replay has in total.
    pub total: Option<u32>,
    /// Unmerged paths. Empty while resolved-but-unfinished: staging a file is
    /// what takes it off this list.
    pub conflicts: Vec<ConflictFile>,
    /// Whether the conflict list was cut at the service's cap.
    pub truncated: bool,
}

impl RebaseState {
    /// An empty state for `workspace_id` — nothing in progress.
    #[must_use]
    pub fn idle(workspace_id: WorkspaceId) -> Self {
        Self {
            workspace_id,
            ..Self::default()
        }
    }

    /// Whether an operation is stopped in this checkout.
    #[must_use]
    pub fn in_progress(&self) -> bool {
        self.operation.is_some()
    }

    /// How many paths are still unresolved.
    #[must_use]
    pub fn unresolved(&self) -> usize {
        self.conflicts.len()
    }

    /// Whether the operation is stopped with every path already staged, which
    /// is the only state in which `--continue` can move.
    #[must_use]
    pub fn ready_to_continue(&self) -> bool {
        self.in_progress() && self.conflicts.is_empty()
    }

    /// `"1 of 4"` when git tracks the replay's position, otherwise `None`.
    #[must_use]
    pub fn progress(&self) -> Option<String> {
        match (self.step, self.total) {
            (Some(step), Some(total)) if total > 0 => Some(format!("{step} of {total}")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict(code: &str) -> ConflictFile {
        ConflictFile {
            path: "a.txt".into(),
            code: code.into(),
        }
    }

    #[test]
    fn an_idle_state_is_not_in_progress() {
        let state = RebaseState::idle(WorkspaceId::new());
        assert!(!state.in_progress());
        assert!(!state.ready_to_continue());
        assert_eq!(state.unresolved(), 0);
    }

    #[test]
    fn continuing_needs_every_path_staged() {
        let mut state = RebaseState {
            operation: Some(SequencerOp::Rebase),
            conflicts: vec![conflict("UU")],
            ..RebaseState::idle(WorkspaceId::new())
        };
        assert!(!state.ready_to_continue());
        state.conflicts.clear();
        assert!(state.ready_to_continue());
    }

    #[test]
    fn progress_needs_both_halves() {
        let mut state = RebaseState::idle(WorkspaceId::new());
        assert_eq!(state.progress(), None);
        state.step = Some(1);
        assert_eq!(state.progress(), None);
        state.total = Some(4);
        assert_eq!(state.progress().as_deref(), Some("1 of 4"));
    }

    #[test]
    fn every_unmerged_pair_reads_as_words() {
        assert_eq!(conflict("UU").label(), "both modified");
        assert_eq!(conflict("DU").label(), "deleted by us");
        assert_eq!(conflict("??").label(), "unmerged");
    }
}
