//! The working-tree diff of one checkout, as the GUI's diff view needs it.
//!
//! Runtime-only state, like [`crate::pull_request::PullRequest`] and
//! [`crate::workspace::Workspace::status`]: it is computed on demand from the
//! checkout on disk, never stored and never authoritative. A diff that came
//! out of a database would describe a working tree that no longer exists.

use serde::{Deserialize, Serialize};

use crate::ids::{SessionId, Timestamp, WorkspaceId};

/// What happened to one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DiffStatus {
    /// The path is new: untracked, or added to the index.
    Added,
    /// The path exists on both sides.
    Modified,
    /// The path is gone from the working tree.
    Deleted,
    /// The path is unmerged: a stopped rebase, merge or cherry-pick left both
    /// sides in it. See [`crate::rebase::RebaseState`].
    Conflicted,
}

impl DiffStatus {
    /// The single letter git itself uses, for a dense file list.
    #[must_use]
    pub fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Conflicted => "U",
        }
    }
}

/// One changed path and the unified patch that describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    /// Repository-relative path, exactly as git spells it.
    pub path: String,
    /// How the path changed.
    pub status: DiffStatus,
    /// Added lines.
    pub additions: u32,
    /// Removed lines.
    pub deletions: u32,
    /// Unified patch text. Empty when there is nothing to render — a binary
    /// file, or one whose patch exceeded the wire budget.
    pub patch: String,
    /// Whether git reported the content as binary.
    pub binary: bool,
    /// Whether [`DiffFile::patch`] was dropped for exceeding a budget. A patch
    /// is dropped whole rather than cut: half a patch is not a patch.
    pub truncated: bool,
}

/// Everything uncommitted in one checkout: staged, unstaged and untracked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDiff {
    /// Which checkout this describes.
    pub workspace_id: WorkspaceId,
    /// Current branch, when not detached.
    pub branch: Option<String>,
    /// Changed paths in git's own order.
    pub files: Vec<DiffFile>,
    /// Whether files, or one file's patch, were left out of the answer.
    pub truncated: bool,
}

impl WorkspaceDiff {
    /// Whether the checkout has nothing uncommitted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Added and removed lines across every file.
    #[must_use]
    pub fn totals(&self) -> (u32, u32) {
        self.files.iter().fold((0, 0), |(added, removed), file| {
            (added + file.additions, removed + file.deletions)
        })
    }
}

/// One changed path without its patch, for a summary that has no room for one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSummaryFile {
    /// Repository-relative path, exactly as git spells it.
    pub path: String,
    /// How the path changed.
    pub status: DiffStatus,
    /// Added lines. `0` for a binary file, which has no lines to count.
    pub additions: u32,
    /// Removed lines.
    pub deletions: u32,
    /// Whether git reported the content as binary.
    pub binary: bool,
}

/// One commit, as dense as a list can be and still be readable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitLine {
    /// Abbreviated hash, as `git log --format=%h` spells it.
    pub short_id: String,
    /// Subject line only; a body belongs in the commit, not in a list.
    pub subject: String,
}

/// Why the base of a [`ChangeSummary`] is what it is.
///
/// Shown rather than hidden: falling back to `HEAD` silently would present a
/// much smaller diff as if it were the whole story.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BaseOrigin {
    /// A recorded baseline that git still resolves.
    Recorded,
    /// Nothing recorded a baseline, so this is `HEAD`.
    Missing,
    /// A baseline was recorded but git cannot resolve it any more — a rebase,
    /// an amend, a branch that was rewritten. This is `HEAD`.
    Unreachable,
}

/// What changed since some base, without the patches.
///
/// Runtime-only like [`WorkspaceDiff`], and deliberately cheaper: it costs a
/// fixed number of subprocesses whatever the number of changed files, because
/// the surface that reads it refreshes on its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSummary {
    /// Current branch, when not detached.
    pub branch: Option<String>,
    /// The base these numbers are against, abbreviated. `None` when the
    /// repository has no commits at all.
    pub base: Option<String>,
    /// Changed paths in git's own order.
    pub files: Vec<ChangeSummaryFile>,
    /// Commits between the base and `HEAD`, newest first, capped.
    pub commits: Vec<CommitLine>,
    /// How many commits there are, which can exceed `commits.len()`.
    pub commit_count: u32,
    /// Whether files or commits were left out of the answer.
    pub truncated: bool,
}

impl ChangeSummary {
    /// Added and removed lines across every file.
    #[must_use]
    pub fn totals(&self) -> (u32, u32) {
        self.files.iter().fold((0, 0), |(added, removed), file| {
            (added + file.additions, removed + file.deletions)
        })
    }

    /// Whether nothing changed and nothing was committed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.commit_count == 0
    }
}

/// One session's changes since its own baseline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionChanges {
    /// The session this describes.
    pub session_id: SessionId,
    /// Where the base came from, so the reader knows what the numbers cover.
    pub origin: BaseOrigin,
    /// The changes themselves.
    pub summary: ChangeSummary,
    /// How many other live sessions share this checkout. Non-zero means the
    /// numbers can include work this session did not do.
    pub sharing_sessions: u32,
}

/// One session's row in a review header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSession {
    pub session_id: SessionId,
    /// Resolved display title, as the tab strip shows it.
    pub title: String,
    /// Provider slug, or `None` for a plain shell.
    pub provider: Option<String>,
    /// Whether the session is still running.
    pub active: bool,
    /// This session's own baseline, abbreviated. `None` when it has none.
    pub base: Option<String>,
    /// Commits between this session's baseline and `HEAD`.
    pub commit_count: u32,
    pub created_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

/// One checkout's changes since its sessions began, plus who those were.
///
/// One diff, not one per session: two agents writing the same checkout cannot
/// be told apart by looking at the result, so the sessions are a header over a
/// single answer rather than sections that would each claim a share of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceReview {
    /// Which checkout this describes.
    pub workspace_id: WorkspaceId,
    /// The common ancestor of every session baseline, abbreviated.
    pub base: Option<String>,
    /// Where that base came from.
    pub origin: BaseOrigin,
    /// The sessions of this checkout, newest first.
    pub sessions: Vec<ReviewSession>,
    /// Commits between the base and `HEAD`, newest first, capped.
    pub commits: Vec<CommitLine>,
    /// The diff itself, from the base.
    pub diff: WorkspaceDiff,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, additions: u32, deletions: u32) -> DiffFile {
        DiffFile {
            path: path.to_string(),
            status: DiffStatus::Modified,
            additions,
            deletions,
            patch: String::new(),
            binary: false,
            truncated: false,
        }
    }

    #[test]
    fn totals_add_up_across_files() {
        let diff = WorkspaceDiff {
            files: vec![file("a", 3, 1), file("b", 4, 0)],
            ..WorkspaceDiff::default()
        };
        assert_eq!(diff.totals(), (7, 1));
        assert!(!diff.is_empty());
        assert!(WorkspaceDiff::default().is_empty());
    }

    #[test]
    fn a_status_has_gits_own_letter() {
        assert_eq!(DiffStatus::Added.letter(), "A");
        assert_eq!(DiffStatus::Modified.letter(), "M");
        assert_eq!(DiffStatus::Deleted.letter(), "D");
    }

    #[test]
    fn a_diff_round_trips_through_serde() {
        let diff = WorkspaceDiff {
            workspace_id: WorkspaceId::new(),
            branch: Some("main".to_string()),
            files: vec![file("a", 1, 1)],
            truncated: false,
        };
        let text = serde_json::to_string(&diff).unwrap();
        let back: WorkspaceDiff = serde_json::from_str(&text).unwrap();
        assert_eq!(diff, back);
    }
}
