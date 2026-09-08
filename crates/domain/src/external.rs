//! Externally-discovered agent sessions.
//!
//! Read-only history of agent CLI sessions found on disk that the daemon never
//! launched — e.g. Claude Code transcripts under
//! `~/.claude/projects/<slug>/*.jsonl`. They are surfaced in the history panel
//! next to daemon-owned sessions so a run started outside Forge (in a plain
//! shell, or before the project was added) stays reachable.
//!
//! Unlike [`crate::Session`], these are not nodes of the session graph: they
//! have no PTY, no lifecycle and no row in the daemon DB. They are recomputed
//! from disk on each snapshot. There is no PTY to attach to, but the run itself
//! is not a dead end: the GUI hands the recorded `session_id` back to the CLI
//! that wrote it, which re-enters the conversation in a session of its own
//! (§13.5).

use crate::ids::{ProjectId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One agent session transcript discovered on disk (§ future, ADR-010 note).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalAgentSession {
    /// The provider's own session id (e.g. Claude Code's `sessionId`). Stable
    /// across snapshots, so the GUI can key cards on it — and the id its CLI
    /// resumes by (§13.5).
    pub session_id: String,
    /// The Forge project this transcript belongs to, matched by recorded `cwd`.
    pub project_id: ProjectId,
    /// The workspace whose directory the transcript was recorded in, when it
    /// is one Forge knows. A run inside a worktree resolves to that worktree,
    /// which is what lets the history panel scope to a single workspace.
    pub workspace_id: Option<WorkspaceId>,
    /// Provider slug, e.g. `"claude"`. Drives the card glyph/label.
    pub provider: String,
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
    /// First recorded activity (falls back to the file's mtime).
    pub started_at: Timestamp,
    /// Last recorded activity (falls back to the file's mtime).
    pub last_activity: Timestamp,
}
