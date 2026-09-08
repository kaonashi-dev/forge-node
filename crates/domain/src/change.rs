//! Working-tree change context and Juva draft kinds.
//!
//! These types travel on the wire so the GUI can preview what Juva saw and
//! what it proposes before any mutation runs. They are not persisted.

use serde::{Deserialize, Serialize};

/// What Juva should draft from a workspace's current changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum JuvaKind {
    /// A git commit message for the dirty working tree.
    CommitMessage,
    /// A pull-request title and body for commits ahead of the base.
    PullRequest,
    /// A prose review of what a checkout changed, for the Review tab (§16.7).
    ChangeReview,
    /// Unknown variant from a newer peer.
    #[serde(other)]
    Unknown,
}

/// One path that differs from HEAD (staged, unstaged, or untracked).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeFile {
    /// Path relative to the worktree root.
    pub path: String,
    /// Short status letter(s), porcelain-style (`M`, `A`, `??`, …).
    pub status: String,
}

/// Snapshot of what is about to be committed or described in a PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeContext {
    /// Current branch, when not detached.
    pub branch: Option<String>,
    /// Suggested PR base (`origin/HEAD` / default branch), when known.
    pub default_branch: Option<String>,
    /// Whether the worktree has uncommitted changes.
    pub dirty: bool,
    /// Commits ahead of upstream, when an upstream is configured.
    pub ahead: Option<u32>,
    /// Commits behind upstream, when an upstream is configured.
    pub behind: Option<u32>,
    /// Changed paths (capped; see `truncated`).
    pub files: Vec<ChangeFile>,
    /// Combined `git diff` / `git diff --cached` text for Juva.
    pub patch: String,
    /// True when `patch` was cut short to stay within the wire budget.
    pub truncated: bool,
}

/// Text Juva produced for the user to edit before applying.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JuvaDraft {
    /// Which kind of text this is.
    pub kind: JuvaKind,
    /// Primary line: commit subject or PR title.
    pub title: String,
    /// Optional body (commit body or PR description).
    pub body: String,
    /// The change context used to produce the draft.
    pub context: ChangeContext,
}

impl JuvaDraft {
    /// Single editable blob: title, then a blank line, then body when present.
    #[must_use]
    pub fn as_editable(&self) -> String {
        if self.body.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{}\n\n{}", self.title.trim_end(), self.body.trim_end())
        }
    }

    /// Split an edited draft back into title + body (first paragraph = title).
    #[must_use]
    pub fn from_editable(kind: JuvaKind, text: &str, context: ChangeContext) -> Self {
        let text = text.trim();
        let (title, body) = match text.split_once("\n\n") {
            Some((title, body)) => (title.trim().to_string(), body.trim().to_string()),
            None => {
                let mut lines = text.lines();
                let title = lines.next().unwrap_or("").trim().to_string();
                let rest: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
                (title, rest)
            }
        };
        Self {
            kind,
            title,
            body,
            context,
        }
    }
}
