//! Discovered transcripts have no PTY or daemon DB row; resuming launches a session.

use crate::ids::{AgentProfileId, ProjectId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where a run is recorded, and so whether it can be removed on its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TranscriptStore {
    /// Its own file, plus any sibling artifacts, which can be removed alone.
    #[default]
    File,
    /// A database holding every other run of this provider (opencode ≥ 1.17).
    /// Forge opens it read-only, so a single run cannot be deleted from it.
    SharedDatabase,
}

/// A discovered run's conversation, folded to plain text.
///
/// Mirrors [`crate::SessionTranscript`] field for field so a handoff can treat
/// the two the same. `turns` rather than `lines` is the one honest difference:
/// a transcript file has no terminal rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalTranscript {
    /// The run this was read from.
    pub session_id: String,
    /// Oldest turn first, each prefixed by who spoke.
    pub text: String,
    /// Turns `text` covers.
    pub turns: u32,
    /// Whether older turns were dropped to stay inside the caller's budget.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalAgentSession {
    /// Provider-owned resume id, stable across snapshots.
    pub session_id: String,
    /// The Forge project this transcript belongs to, matched by recorded `cwd`.
    pub project_id: ProjectId,
    /// The workspace whose directory the transcript was recorded in, when it
    /// is one Forge knows. A run inside a worktree resolves to that worktree,
    /// which is what lets the history panel scope to a single workspace.
    pub workspace_id: Option<WorkspaceId>,
    /// Provider slug, e.g. `"claude"`. Drives the card glyph/label.
    pub provider: String,
    /// Resume must use this profile's account; `None` selects the default account.
    #[serde(default)]
    pub profile_id: Option<AgentProfileId>,
    /// Display title: the provider's generated title when present, else the
    /// first user prompt, else the session id.
    pub title: String,
    /// Git branch recorded in the transcript, if any.
    pub branch: Option<String>,
    /// The tail of the conversation: the agent's last readable message, folded
    /// to one paragraph and capped. `None` when the transcript holds no plain
    /// prose (a run that only ever called tools, or one still on its first
    /// prompt).
    pub preview: Option<String>,
    /// The model that produced the last agent message, as the provider names
    /// it (`claude-opus-5`, `gpt-5.3-codex`, …).
    pub model: Option<String>,
    /// Conversation turns in the transcript: prompts the user typed plus
    /// replies the agent wrote. Tool traffic is not counted — it would swamp
    /// the number without saying anything about the size of the conversation.
    pub message_count: u32,
    /// Subagent runs this session spawned, each of which has its own
    /// transcript. `0` for a run that never delegated.
    pub subagent_count: u32,
    /// Absolute path to the transcript file, for the GUI to open when the run
    /// cannot be resumed — an uninstalled provider, or one with no resume.
    pub transcript_path: PathBuf,
    /// Whether [`ExternalAgentSession::transcript_path`] is this run's alone.
    /// The GUI greys `Delete` for a shared store rather than letting the
    /// daemon refuse on confirm.
    #[serde(default)]
    pub store: TranscriptStore,
    /// First recorded activity (falls back to the file's mtime).
    pub started_at: Timestamp,
    /// Last recorded activity (falls back to the file's mtime).
    pub last_activity: Timestamp,
}
