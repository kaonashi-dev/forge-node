//! Harness feature state mirrored from `harness/features.json` (runtime-only on
//! the wire — no SQLite column).

use crate::ids::{SessionId, WorkspaceId};
use serde::{Deserialize, Serialize};

/// One row from `harness/features.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessFeature {
    pub id: u32,
    pub slug: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub spec_raw: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub review_rounds: Option<u32>,
    #[serde(default)]
    pub gate_attempts: Option<u32>,
    #[serde(default)]
    pub crates: Option<Vec<String>>,
    #[serde(default)]
    pub acceptance: Option<Vec<String>>,
    #[serde(default)]
    pub source_issue: Option<u32>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// Live orchestrator session, set by the GUI when the feature starts.
    #[serde(default)]
    pub orchestrator_session_id: Option<SessionId>,
    /// The checkout this feature is being implemented in.
    ///
    /// The state itself is one file per *repository* (see
    /// `harness_service`), so two features can run side by side in two
    /// worktrees of the same project; this is what tells them apart, and what
    /// the diff and commit actions of the Feature tab are aimed at. `None` on
    /// rows registered outside Forge, where there is no checkout to name.
    #[serde(default)]
    pub workspace_id: Option<WorkspaceId>,
    /// The same checkout as a path, for whoever is reading `features.json`
    /// by hand — a `WorkspaceId` means nothing outside the daemon's model.
    #[serde(default)]
    pub workspace_path: Option<String>,
    /// How many times this row has been written, so a decision taken against
    /// a stale view of it is refused instead of applied twice.
    ///
    /// A client reads a feature, a person clicks *Approve*, and in between the
    /// review that was already running settled the row. Replaying that click
    /// used to append a second `human_gate_resolved` and launch a second
    /// implementer in the same worktree. The client sends the revision it saw;
    /// a mismatch is a no-op that answers with the current row.
    #[serde(default)]
    pub revision: Option<u64>,
    /// Every run of a step, in order — the harness's Dispatch row.
    ///
    /// `review_rounds` and `gate_attempts` are counters and say nothing about
    /// *which* provider produced a result, how long it took, or whether the
    /// job that was started ever settled. That last one is what a daemon
    /// restart needs: an attempt with no `settled_at` and no live job is a step
    /// that died with its daemon, and it is recoverable precisely because it is
    /// written down.
    #[serde(default)]
    pub attempts: Option<Vec<HarnessAttempt>>,
    /// Why the feature is `blocked`, in one sentence.
    ///
    /// Also the last `feature_blocked` event's `reason`, but a client showing
    /// a blocked card should not have to read the timeline to render it.
    #[serde(default)]
    pub blocked_reason: Option<String>,
}

/// One run of one step: who ran it, when, and how it ended.
///
/// Orca's Dispatch at feature granularity. A retry is a *new* attempt rather
/// than a mutation of the previous one, so the trail reads as "the implementer
/// was started three times" and not as "the implementer is on attempt 3".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessAttempt {
    pub step: HarnessStep,
    /// 1-based, counted per step and reset when the step is left behind.
    pub n: u32,
    /// The headless run, when this attempt was one.
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    /// `cli` today; `acp` once the second transport lands.
    #[serde(default)]
    pub transport: Option<String>,
    pub started_at: String,
    #[serde(default)]
    pub settled_at: Option<String>,
    /// `succeeded` | `failed` | `cancelled` | `blocked`, unset while it runs.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Why it ended that way, when that needs saying.
    #[serde(default)]
    pub detail: Option<String>,
    /// The provider's own session id, when the stream named one.
    ///
    /// What a retry passes as `resume_from` so the next attempt re-enters the
    /// same conversation instead of throwing away research already paid for.
    #[serde(default)]
    pub provider_session_id: Option<String>,
}

impl HarnessAttempt {
    /// Whether this attempt is still on record as running.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.settled_at.is_none()
    }
}

impl HarnessFeature {
    /// Whether the feature still occupies its checkout: anything that is not
    /// finished or abandoned. A second feature may not start in a checkout
    /// that already has one of these.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !matches!(self.status.as_str(), "done" | "blocked")
    }
}

/// Parsed `harness/features.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessFeatureList {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub features: Vec<HarnessFeature>,
    #[serde(default)]
    pub initialized: bool,
}

/// One line from `harness/progress/events_<id>.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessEvent {
    pub ts: String,
    pub kind: String,
    /// Remaining fields from the JSON line, serialized for display.
    #[serde(default)]
    pub detail: String,
    /// The headless run this event started, when it started one.
    ///
    /// Lifted out of `detail` because it is the one field a client *acts* on
    /// rather than prints: jobs live in the daemon's memory, so a restart
    /// forgets every row while the transcripts stay on disk. This id is what
    /// still points at one, and reading it here keeps JSON parsing in the
    /// crate that already does it.
    #[serde(default)]
    pub job: Option<crate::ids::JobId>,
}

/// Which markdown artefact to read under `harness/progress/` or `harness/specs/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum HarnessArtifactKind {
    Gate,
    Context,
    Impl,
    Review,
    Requirements,
    Design,
    Tasks,
    Current,
    #[serde(other)]
    Unknown,
}

/// Human or UI-driven step in the harness cycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum HarnessAdvanceAction {
    ApproveSpec,
    ReviseSpec,
    Block {
        reason: String,
    },
    StartImplement,
    StartReview,
    /// Run the step the feature was blocked on again, from `blocked`.
    ///
    /// The counterpart of the retry budget: once the machine has spent it, the
    /// only thing that may start another attempt is a person who has looked at
    /// why it failed.
    RetryStep,
    /// Take a `done` or `blocked` feature back to `pending`.
    Reopen,
}

/// One step of the harness cycle, as a thing that can be *run*.
///
/// The names are the roles the harness has always had; what is new is that
/// each is a job with an exit code rather than an agent somebody watches. The
/// human gate is deliberately not here: approving a spec is not a step that
/// can be run, it is the one moment the machine stops and waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum HarnessStep {
    /// Research the codebase and write the spec, then stop at the gate.
    Spec,
    /// Implement the approved spec and run the gate command.
    Implement,
    /// Judge the implementation and return a verdict.
    Review,
}

impl HarnessStep {
    /// The status the feature takes while this step runs.
    #[must_use]
    pub fn running_status(self) -> &'static str {
        match self {
            Self::Spec => "pending",
            Self::Implement => "in_progress",
            Self::Review => "in_review",
        }
    }

    /// The event name appended to the feature's log when it starts.
    #[must_use]
    pub fn started_event(self) -> &'static str {
        match self {
            Self::Spec => "spec_started",
            Self::Implement => "impl_started",
            Self::Review => "review_started",
        }
    }
}

impl HarnessFeature {
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "in_progress" | "in_review")
    }

    #[must_use]
    pub fn needs_gate(&self) -> bool {
        self.status == "spec_ready"
    }
}
