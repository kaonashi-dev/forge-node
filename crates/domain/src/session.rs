//! Session domain type, state machine, roles and title rules (§7.3, ADR-010).

use crate::ids::{AgentProfileId, AgentProviderId, SessionId, TerminalId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Maximum depth of the session graph in the MVP (validated in the daemon,
/// ADR-010). `root` is depth 1.
pub const MAX_GRAPH_DEPTH: u32 = 8;

/// A session is either a plain shell or an agent CLI running in a PTY.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SessionKind {
    Shell,
    Agent,
}

/// Unified session state machine (§7.3). This is the single source of truth;
/// `SessionState::Exited` also carries the exit code/signal so there is one
/// update path (`SessionUpdated`), never a separate `SessionExited` event.
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
    /// Was `Running` when the daemon restarted; the PTY did not survive (§3.3).
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

    /// Validates the transitions of §7.3:
    ///
    /// ```text
    /// Starting → Running | Failed
    /// Running  → Exited | Orphaned
    /// Exited | Failed | Orphaned → Starting   (via RestartSession)
    /// ```
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

/// Role tags used by the (future) orchestrator; free-form `Custom` for the rest.
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

/// Title of a session (§7.3). The user title takes precedence over the
/// terminal-reported (OSC 0/2) title.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitle {
    /// Set via `RenameSession`; has precedence.
    pub user: Option<String>,
    /// Latest OSC 0/2 title received from the terminal.
    pub terminal: Option<String>,
}

impl SessionTitle {
    /// OSC 0/2 payload as stored on the session: empty or whitespace is a reset.
    ///
    /// `Some("")` would otherwise beat the fallback in [`Self::resolve`] and
    /// leave a rail row with no name — the leftover check-and-dot after an
    /// agent exits and clears its title.
    #[must_use]
    pub fn from_osc(raw: Option<&str>) -> Option<String> {
        raw.map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }

    /// Title rule (§7.3): show `user`, else `terminal`, else the caller-provided
    /// fallback (`"{provider}"` or `"Shell"`). Empty or whitespace counts as
    /// unset, so an OSC reset cannot blank the row.
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
    /// `None` in `Failed`/`Orphaned` or while a restart is pending (§7.3).
    pub terminal_id: Option<TerminalId>,
    pub agent_provider_id: Option<AgentProviderId>,
    /// The launch profile this session ran with, when it was started from one
    /// (§13.4). Kept even after the profile is deleted: the row is history, and
    /// the UI falls back to the provider's own name.
    pub agent_profile_id: Option<AgentProfileId>,
    pub title: SessionTitle,
    pub state: SessionState,
    pub created_at: Timestamp,
    /// Foreground command a shell session was running when the daemon went
    /// down, so a restart re-runs it rather than opening a fresh prompt.
    /// `None` is a fresh shell; agent sessions never set it, since they
    /// relaunch from their provider and profile instead.
    pub launch_command: Option<String>,
    /// Last moment this session's PTY produced output or received input.
    ///
    /// Runtime state like `terminal_id`: it is never a column, and a session
    /// loaded from SQLite starts at `ended_at` or `created_at`. Persisting it
    /// would mean a row write per second of terminal traffic to describe a PTY
    /// that cannot outlive the daemon anyway (§3.3, §15.2).
    ///
    /// The daemon bumps it in memory (coalesced to at most once per second per
    /// terminal) and the idle policy reads it; see [`Session::idle_for`].
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
