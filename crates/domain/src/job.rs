//! Headless agent runs: one task, no terminal, an exit code at the end.
//!
//! A [`crate::session::Session`] is an agent the user talks to: it owns a PTY,
//! it lives until somebody closes it, and "is it finished?" is a question only
//! a human reading the screen can answer. A [`Job`] is the other half of the
//! same providers — the CLIs also run non-interactively (`claude -p`,
//! `codex exec`), take one prompt, stream machine-readable events and *exit*.
//!
//! That difference is the whole point. The harness needs to know when a step
//! is done, what it decided, and whether it failed, without anyone watching a
//! terminal; a process that exits answers all three. Several jobs also run at
//! once without each one occupying a tab.
//!
//! Authentication is deliberately the same as an interactive launch: the same
//! binary, started the same way, reading the same subscription login from the
//! provider's own config. Nothing here knows what an API key is.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentProviderId, JobId, SessionId, Timestamp, WorkspaceId};
use crate::session::SessionRole;

/// Where a job is in its short life.
///
/// There is no `Idle`/`Orphaned` here as there is for sessions: a job either
/// waits for a slot, runs, or is over. The three terminal states are kept
/// apart because they mean different things to the caller — a harness step
/// advances on `Succeeded`, stops on `Failed`, and does neither when the user
/// cancelled it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum JobState {
    /// Accepted, waiting for a concurrency slot.
    #[default]
    Queued,
    /// The process is running.
    Running,
    /// The process exited 0.
    Succeeded,
    /// The process exited non-zero, or could not be started at all.
    Failed,
    /// Killed on request.
    Cancelled,
}

impl JobState {
    /// Whether the job is over, whatever the outcome.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// One headless run of one provider.
///
/// The row is small on purpose: the output itself is a file
/// ([`Job::log_path`]), not a field, because a run streams for minutes and no
/// client wants it re-broadcast whole on every change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub provider_id: AgentProviderId,
    /// The checkout the process runs in — its cwd.
    pub workspace_id: WorkspaceId,
    /// What the job is for, reusing the session graph's vocabulary so a
    /// harness step reads the same whether it ran headless or in a terminal.
    pub role: SessionRole,
    /// The harness feature this job belongs to, when it belongs to one.
    pub feature_id: Option<u32>,
    /// The interactive session that asked for it, when one did. A job started
    /// by an orchestrator points at it, which is what lets the UI show the
    /// tree the harness actually ran.
    pub parent_session_id: Option<SessionId>,
    pub state: JobState,
    /// First line of the prompt, for a list that has to fit on one row.
    pub summary: String,
    /// The whole task the provider was given, verbatim.
    ///
    /// Carried on the row rather than left to the log because no provider
    /// echoes it: a stream starts with the agent's *answer*, so without this
    /// the one question a person asks of a running step — "what did you
    /// actually ask it?" — had no answer anywhere in the UI. It is written
    /// once and never changes, and a harness prompt is a few hundred bytes
    /// against a run that streams for minutes.
    pub prompt: String,
    /// The provider's *own* session id, read out of the event stream once it
    /// appears. What a follow-up job resumes from.
    pub provider_session_id: Option<String>,
    /// `None` until the process exits; `Some(-1)` when it died on a signal.
    pub exit_code: Option<i32>,
    /// Last *summarised* line of the stream, so a list can say what the job
    /// is doing without opening the log. The provider's own JSON stays in
    /// `log_path`; this field is the same vocabulary `JobOutput` carries.
    pub last_line: Option<String>,
    /// When the last line arrived — the heartbeat, measured rather than asked
    /// for.
    ///
    /// `last_line` used to be a local in `follow_job` that only reached the row
    /// in `finish_job`, so while a step ran the card that prints it said
    /// nothing and no watchdog could tell a thinking agent from a hung one.
    /// Written on the same throttle as the output events.
    #[serde(default)]
    pub last_output_at: Option<Timestamp>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    /// The full event stream, one JSON object per line, as the provider wrote
    /// it. The daemon never edits it: whatever the CLI emitted is the record.
    pub log_path: PathBuf,
}

/// What a client asks for when it starts a job.
///
/// Everything that is not here is resolved by the daemon: the executable, the
/// environment, the working directory, and how the provider spells "run this
/// non-interactively" ([`crate::agent::HeadlessSpec`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRequest {
    pub provider_id: AgentProviderId,
    pub workspace_id: WorkspaceId,
    pub role: SessionRole,
    pub feature_id: Option<u32>,
    pub parent_session_id: Option<SessionId>,
    /// The task. Passed to the CLI the way its descriptor spells a prompt.
    pub prompt: String,
    /// A provider session id to continue instead of starting fresh — the
    /// `provider_session_id` of an earlier job, usually. Refused when the
    /// provider declares no headless resume.
    pub resume_from: Option<String>,
    /// A JSON Schema the answer must match, for a step whose result something
    /// downstream has to *act* on rather than read. Dropped, not refused, when
    /// the provider declares no way to ask for one: the answer is then prose,
    /// which is what it would have been anyway.
    pub schema: Option<String>,
}

impl Job {
    /// The one-line summary a prompt gets in a list.
    ///
    /// Trimmed to a length that survives a narrow column; the whole prompt is
    /// on the row as [`Job::prompt`], so nothing is lost by cutting it here.
    #[must_use]
    pub fn summarize(prompt: &str) -> String {
        const MAX: usize = 120;
        let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let line = line.trim();
        if line.chars().count() <= MAX {
            return line.to_owned();
        }
        line.chars().take(MAX - 1).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_the_first_non_empty_line() {
        assert_eq!(
            Job::summarize("\n\n  do the thing  \nand more"),
            "do the thing"
        );
        assert_eq!(Job::summarize(""), "");
    }

    #[test]
    fn a_long_summary_is_cut_with_an_ellipsis() {
        let summary = Job::summarize(&"x".repeat(500));
        assert_eq!(summary.chars().count(), 120);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn only_the_three_outcomes_are_final() {
        assert!(!JobState::Queued.is_final());
        assert!(!JobState::Running.is_final());
        for state in [JobState::Succeeded, JobState::Failed, JobState::Cancelled] {
            assert!(state.is_final(), "{state:?} ends a job");
        }
    }
}
