use crate::ids::{AgentProfileId, AgentProviderId, SessionId, TerminalId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Maximum depth of the session graph in the MVP (validated in the daemon,
/// ADR-010). `root` is depth 1.
pub const MAX_GRAPH_DEPTH: u32 = 8;

/// A session is a plain shell, an agent CLI or a file-editing terminal process
/// running in a PTY.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SessionKind {
    Shell,
    Agent,
    /// `forge-editor` in a PTY, with a control channel reporting [`EditorState`].
    Editor,
}

/// Exit status travels through `SessionUpdated`, not a separate exit event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SessionState {
    /// Request accepted, PTY not yet reporting a live process.
    Starting,
    /// Process is alive.
    Running,
    /// Process exited on its own or via kill.
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// Spawn failed (binary missing, cwd invalid, ...).
    Failed { reason: String },
    /// Was `Running` when the daemon restarted; the PTY did not survive.
    Orphaned,
}

impl SessionState {
    /// A state is *terminal* when no process is (or will be) attached without
    /// an explicit `RestartSession`.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SessionState::Exited { .. } | SessionState::Failed { .. } | SessionState::Orphaned
        )
    }

    /// Whether the session currently occupies a live/soon-to-be-live process.
    /// `CloseSession` is rejected in these states without an explicit kill.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, SessionState::Starting | SessionState::Running)
    }

    #[must_use]
    pub fn can_transition_to(&self, next: &SessionState) -> bool {
        use SessionState::{Exited, Failed, Orphaned, Running, Starting};
        matches!(
            (self, next),
            (Starting, Running)
                | (Starting, Failed { .. })
                | (Running, Exited { .. })
                | (Running, Orphaned)
                | (Exited { .. }, Starting)
                | (Failed { .. }, Starting)
                | (Orphaned, Starting)
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SessionRole {
    #[default]
    Generic,
    Orchestrator,
    Planner,
    Researcher,
    Executor,
    Reviewer,
    Tester,
    Custom(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitle {
    /// Set via `RenameSession`; has precedence.
    pub user: Option<String>,
    /// Latest OSC 0/2 title received from the terminal.
    pub terminal: Option<String>,
}

impl SessionTitle {
    /// Empty or whitespace OSC 0/2 payloads reset the title to allow a fallback.
    #[must_use]
    pub fn from_osc(raw: Option<&str>) -> Option<String> {
        raw.map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }

    /// Prefer user, then terminal, then fallback; whitespace-only titles are unset.
    #[must_use]
    pub fn resolve<'a>(&'a self, fallback: &'a str) -> &'a str {
        present_title(self.user.as_deref())
            .or_else(|| present_title(self.terminal.as_deref()))
            .unwrap_or(fallback)
    }
}

fn present_title(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.trim().is_empty())
}

/// Metadata of the buffer an editor session holds.
///
/// Published by the editor process over its control channel and stored on the
/// session so it rides `SessionUpdated` and every snapshot. It carries no
/// document text: the draft lives in the editor process, and this is what the
/// GUI needs to draw a path and a position.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorState {
    /// Workspace-relative path, display metadata only. The daemon owns
    /// resolution; the editor never touches the checkout.
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based.
    pub column: u32,
    pub dirty: bool,
    pub read_only: bool,
    /// `editor_core::DocumentVersion` as a number: monotonic per mutation.
    /// Kept opaque here so `domain` does not depend on the editor crate.
    pub document_version: u64,
    /// 1-based first line on screen, and how much of the buffer is on it.
    ///
    /// The editor owns its viewport — the GUI paints a passive cell grid and
    /// has no second copy of the text — so a scrollbar can only be drawn from
    /// what the editor reports here.
    #[serde(default)]
    pub top_line: u32,
    /// Logical lines the viewport shows, at least 1 once a state has arrived.
    #[serde(default)]
    pub visible_lines: u32,
    /// Lines in the buffer, the thumb's denominator.
    #[serde(default)]
    pub total_lines: u32,
    /// The caret's line as text, clamped by the editor before it is sent.
    ///
    /// The one piece of document text on a session, and it is here for the
    /// screen reader: the GUI paints a passive cell grid, so without it the
    /// only way to say what line a person is on is to read it back out of the
    /// cells the editor drew. Never the buffer, never a selection's text.
    #[serde(default)]
    pub caret_line: String,
    /// Bytes the primary caret has selected.
    #[serde(default)]
    pub selection_length: u32,
    /// How many carets there are; more than one is announced.
    #[serde(default)]
    pub cursor_count: u32,
    /// The editor's transient message — a find tally, a refusal — as its own
    /// status row shows it. The GUI announces it; a person who cannot see the
    /// canvas cannot see that row.
    #[serde(default)]
    pub status: String,
    /// A save was refused because the file moved under the buffer.
    ///
    /// The daemon's word, not the editor's: the editor only learns a reason
    /// string, while the daemon is the one that saw the revision mismatch. A
    /// flag and never the texts — the draft lives in the editor process, and
    /// the two sides of the comparison are fetched with `GetEditorConflict`
    /// when somebody asks to see them, not carried on every `SessionUpdated`.
    #[serde(default)]
    pub conflict: bool,
}

/// A persistent unit of work in the domain; a node of the session graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub workspace_id: WorkspaceId,
    pub kind: SessionKind,
    pub role: SessionRole,
    /// `None` for graph roots.
    pub parent_session_id: Option<SessionId>,
    /// Equals `id` for roots (ADR-010).
    pub root_session_id: SessionId,
    /// `None` in `Failed`/`Orphaned` or while a restart is pending.
    pub terminal_id: Option<TerminalId>,
    /// Buffer metadata for an `Editor` session, `None` for every other kind.
    ///
    /// Runtime state like `terminal_id`: never a column, `None` on load. The
    /// daemon writes it from the editor's control channel, coalesced, and the
    /// GUI reads it off the snapshot rather than parsing ANSI.
    pub editor: Option<EditorState>,
    pub agent_provider_id: Option<AgentProviderId>,
    /// Retained after profile deletion; the UI falls back to the provider name.
    pub agent_profile_id: Option<AgentProfileId>,
    pub title: SessionTitle,
    pub state: SessionState,
    pub created_at: Timestamp,
    /// Foreground command a shell session was running when the daemon went
    /// down, so a restart re-runs it rather than opening a fresh prompt.
    /// `None` is a fresh shell; agent sessions never set it, since they
    /// relaunch from their provider and profile instead.
    pub launch_command: Option<String>,
    /// Latest PTY input/output, coalesced to once per second for idle checks.
    /// Runtime-only; reconstructed from `ended_at` or `created_at` on load.
    pub last_activity_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    /// The commit this session started from, for reading back what it changed.
    ///
    /// Resolved once with `git rev-parse HEAD` when the session is created and
    /// never again: a restart continues the same unit of work, so resetting it
    /// would drop everything the session had already done. `None` when there
    /// was no HEAD to read — a folder workspace, or a repository with no
    /// commits yet.
    pub base_commit: Option<String>,
}

impl Session {
    /// `true` when this session is a graph root (`root_session_id == id`).
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.parent_session_id.is_none() && self.root_session_id == self.id
    }

    /// How long this session has gone without terminal input or output.
    ///
    /// Only meaningful while the session is active; a terminal one has stopped
    /// producing activity by definition, so callers filter on
    /// [`SessionState::is_active`] first. Returns [`Duration::ZERO`] rather
    /// than panicking if `last_activity_at` is in the future (clock skew).
    #[must_use]
    pub fn idle_for(&self, now: Timestamp) -> Duration {
        let elapsed = now.as_offset() - self.last_activity_at.as_offset();
        Duration::try_from(elapsed).unwrap_or(Duration::ZERO)
    }

    /// How long this session has existed.
    ///
    /// Unlike [`idle_for`](Self::idle_for) this keeps growing while the session
    /// is busy: it answers "open for too long", not "unused".
    #[must_use]
    pub fn age(&self, now: Timestamp) -> Duration {
        let elapsed = now.as_offset() - self.created_at.as_offset();
        Duration::try_from(elapsed).unwrap_or(Duration::ZERO)
    }
}

/// Plain text read back off a session's terminal, for handing to another agent.
///
/// Runtime-only, like [`crate::diff::WorkspaceDiff`]: it is folded out of the
/// engine's rows on demand and never stored. The rows are already-decoded
/// cells, so there are no escape sequences in `text` — the daemon's VT engine
/// consumed them on the way in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTranscript {
    /// The session this was read from.
    pub session_id: SessionId,
    /// The captured text, oldest line first.
    pub text: String,
    /// How many terminal lines `text` covers.
    pub lines: u32,
    /// Whether older lines were dropped to stay inside the caller's budget.
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_transitions_are_accepted() {
        assert!(SessionState::Starting.can_transition_to(&SessionState::Running));
        assert!(
            SessionState::Starting.can_transition_to(&SessionState::Failed { reason: "x".into() })
        );
        assert!(
            SessionState::Running.can_transition_to(&SessionState::Exited {
                code: Some(0),
                signal: None
            })
        );
        assert!(SessionState::Running.can_transition_to(&SessionState::Orphaned));
        assert!(SessionState::Orphaned.can_transition_to(&SessionState::Starting));
        assert!(SessionState::Exited {
            code: None,
            signal: Some(9)
        }
        .can_transition_to(&SessionState::Starting));
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        // Cannot go straight from Starting to Exited.
        assert!(
            !SessionState::Starting.can_transition_to(&SessionState::Exited {
                code: Some(0),
                signal: None
            })
        );
        // Cannot resurrect Running directly.
        assert!(!SessionState::Exited {
            code: Some(0),
            signal: None
        }
        .can_transition_to(&SessionState::Running));
        // Orphaned only restarts.
        assert!(!SessionState::Orphaned.can_transition_to(&SessionState::Running));
    }

    #[test]
    fn title_rule_prefers_user_then_terminal_then_fallback() {
        let mut t = SessionTitle::default();
        assert_eq!(t.resolve("Shell"), "Shell");
        t.terminal = Some("~/code".into());
        assert_eq!(t.resolve("Shell"), "~/code");
        t.user = Some("auth refactor".into());
        assert_eq!(t.resolve("Shell"), "auth refactor");
    }

    #[test]
    fn empty_or_whitespace_titles_are_unset() {
        let empty_terminal = SessionTitle {
            terminal: Some(String::new()),
            ..SessionTitle::default()
        };
        assert_eq!(empty_terminal.resolve("cursor"), "cursor");
        let blank_terminal = SessionTitle {
            terminal: Some("   ".into()),
            ..SessionTitle::default()
        };
        assert_eq!(blank_terminal.resolve("cursor"), "cursor");
        let empty_user = SessionTitle {
            user: Some(String::new()),
            terminal: Some("~/code".into()),
        };
        assert_eq!(empty_user.resolve("cursor"), "~/code");
        assert_eq!(SessionTitle::from_osc(Some("")), None);
        assert_eq!(SessionTitle::from_osc(Some("  ")), None);
        assert_eq!(
            SessionTitle::from_osc(Some(" Cursor ")).as_deref(),
            Some("Cursor")
        );
    }

    #[test]
    fn every_kind_has_a_distinct_tag() {
        let tags: Vec<String> = [SessionKind::Shell, SessionKind::Agent, SessionKind::Editor]
            .into_iter()
            .map(|kind| serde_json::to_string(&kind).unwrap())
            .collect();
        assert_eq!(tags, ["\"Shell\"", "\"Agent\"", "\"Editor\""]);
    }

    #[test]
    fn editor_state_round_trips_with_its_session() {
        let state = EditorState {
            path: "src/main.rs".into(),
            line: 3,
            column: 12,
            dirty: true,
            read_only: false,
            document_version: 7,
            top_line: 1,
            visible_lines: 24,
            total_lines: 200,
            caret_line: "fn main() {}".into(),
            selection_length: 4,
            cursor_count: 2,
            status: "alpha: 3/41".into(),
            conflict: true,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<EditorState>(&json).unwrap(), state);
        assert_eq!(EditorState::default().document_version, 0);
        assert!(!EditorState::default().conflict, "a fresh buffer is clean");
    }

    #[test]
    fn editor_state_is_runtime_only() {
        // Present on the wire so a snapshot recovers it; persistence skips it
        // (see `a_session_row_never_stores_editor_state`).
        let id = SessionId::new();
        let session = Session {
            id,
            workspace_id: WorkspaceId::new(),
            kind: SessionKind::Editor,
            role: SessionRole::Generic,
            parent_session_id: None,
            root_session_id: id,
            terminal_id: None,
            editor: Some(EditorState {
                path: "src/main.rs".into(),
                line: 1,
                column: 1,
                dirty: false,
                read_only: true,
                document_version: 1,
                top_line: 1,
                visible_lines: 24,
                total_lines: 200,
                caret_line: "fn main() {}".into(),
                selection_length: 0,
                cursor_count: 1,
                status: String::new(),
                conflict: false,
            }),
            agent_provider_id: None,
            agent_profile_id: None,
            title: SessionTitle::default(),
            state: SessionState::Running,
            created_at: Timestamp::now(),
            launch_command: None,
            last_activity_at: Timestamp::now(),
            ended_at: None,
            base_commit: None,
        };
        let json = serde_json::to_value(&session).unwrap();
        assert!(
            json.get("editor").and_then(|v| v.get("path")).is_some(),
            "the snapshot must carry buffer metadata"
        );
        assert!(
            json.get("terminal_id").is_some(),
            "runtime-only is not the same as omitted from the wire"
        );
    }

    #[test]
    fn is_terminal_and_is_active_are_disjoint() {
        for s in [
            SessionState::Starting,
            SessionState::Running,
            SessionState::Exited {
                code: None,
                signal: None,
            },
            SessionState::Failed { reason: "e".into() },
            SessionState::Orphaned,
        ] {
            assert_ne!(s.is_terminal(), s.is_active(), "{s:?}");
        }
    }
}
