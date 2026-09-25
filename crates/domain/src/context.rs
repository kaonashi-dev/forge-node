//!
//! `summary` and `instructions` are the primary fields; artifacts and git
//! context are optional. Cross-session delivery goes through `SendContext`
//! (persist + optional PTY paste or child spawn) and `ListContextEnvelopes`.

use crate::ids::{ContextId, RunId, SessionId, TaskId, Timestamp};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ContextArtifactKind {
    Text,
    Plan,
    Review,
    FileReference,
    DiffReference,
    CommitReference,
    TerminalExcerpt,
    StructuredJson,
}

/// A reference to an artifact carried in an envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextArtifactRef {
    pub kind: ContextArtifactKind,
    /// Free-form payload or a pointer, depending on `kind`.
    pub value: String,
}

/// A reference to Git state relevant to an envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitContextRef {
    pub repo_path: PathBuf,
    pub branch: Option<String>,
    pub commit: Option<String>,
}

/// Why an orchestration envelope was stored. Absent on a plain handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextKind {
    Message,
    Question,
    Answer,
    Feedback,
    Pointer,
    #[serde(other)]
    Unrecognized,
}

/// An explicit, auditable transfer of context between sessions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextEnvelope {
    pub id: ContextId,
    /// `None` when a human with no session sent it. The socket is the trust boundary.
    pub source_session_id: Option<SessionId>,
    pub target_session_id: Option<SessionId>,
    pub summary: Option<String>,
    pub instructions: Option<String>,
    pub artifacts: Vec<ContextArtifactRef>,
    pub git_context: Option<GitContextRef>,
    pub created_at: Timestamp,
    /// Set when the envelope belongs to a run. Older rows leave this empty.
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub task_id: Option<TaskId>,
    #[serde(default)]
    pub kind: Option<ContextKind>,
    #[serde(default)]
    pub in_reply_to: Option<ContextId>,
    #[serde(default)]
    pub acked_at: Option<Timestamp>,
}
