//! Context envelope types (§8.3).
//!
//! `summary` and `instructions` are the primary fields; artifacts and git
//! context are optional. Cross-session delivery goes through `SendContext`
//! (persist + optional PTY paste or child spawn) and `ListContextEnvelopes`.

use crate::ids::{ContextId, SessionId, Timestamp};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The kind of artifact a context envelope carries (§8.3).
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

/// An explicit, auditable transfer of context between sessions (§8.3, §23).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextEnvelope {
    pub id: ContextId,
    pub source_session_id: SessionId,
    pub target_session_id: Option<SessionId>,
    pub summary: Option<String>,
    pub instructions: Option<String>,
    pub artifacts: Vec<ContextArtifactRef>,
    pub git_context: Option<GitContextRef>,
    pub created_at: Timestamp,
}
