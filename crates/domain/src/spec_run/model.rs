//! A spec run as the daemon keeps it and every client sees it. Metadata only: the spec, the
//! plan, the logs and the report stay on disk under the run's directory.

use serde::{Deserialize, Serialize};

use crate::{
    AgentProfileId, AgentProviderId, ProjectActionId, ProjectId, ReviewId, RunId, SessionId,
    SpecRunId, TaskId, Timestamp, WorkspaceId,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpecStage {
    #[default]
    Drafting,
    /// G1: the person approves the spec.
    SpecGate,
    Planning,
    /// G2: the person approves the plan and the coder's autonomy.
    PlanGate,
    Executing,
    Verifying,
    Reviewing,
    /// G3: the person marks manual checks and picks the next action.
    ResultGate,
    Done,
    Rejected,
    /// The drafter ended without a spec (needs_details, intended_behavior, …).
    Exited,
}

impl SpecStage {
    #[must_use]
    pub fn is_gate(self) -> bool {
        matches!(self, Self::SpecGate | Self::PlanGate | Self::ResultGate)
    }

    #[must_use]
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Done | Self::Rejected | Self::Exited)
    }

    /// Whether the coder may be running: every stage after G2.
    #[must_use]
    pub fn past_plan_gate(self) -> bool {
        matches!(
            self,
            Self::Executing | Self::Verifying | Self::Reviewing | Self::ResultGate | Self::Done
        )
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drafting => "drafting",
            Self::SpecGate => "spec_gate",
            Self::Planning => "planning",
            Self::PlanGate => "plan_gate",
            Self::Executing => "executing",
            Self::Verifying => "verifying",
            Self::Reviewing => "reviewing",
            Self::ResultGate => "result_gate",
            Self::Done => "done",
            Self::Rejected => "rejected",
            Self::Exited => "exited",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// At most three criteria and no protocol or schema change: G1 and G2 are one gate and the
    /// plan lives in the spec.
    Short,
    #[default]
    Standard,
    /// Several capabilities tested apart: one ledger task each.
    Full,
}

/// What the coder hands over at the end (D1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    #[default]
    NoCommit,
    /// Commits on its worktree branch.
    Commit,
    /// Like `NoCommit`, and Forge opens a draft pull request after G3.
    DraftPr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPick {
    pub provider: AgentProviderId,
    #[serde(default)]
    pub profile: Option<AgentProfileId>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Placement {
    /// A new managed worktree on `branch` (D2 default).
    Worktree {
        branch: String,
        #[serde(default)]
        base: Option<String>,
    },
    /// The run's own checkout; what was already modified is left out of the review.
    Checkout,
}

/// What the person picks at G2, seeded from the launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecOptions {
    pub placement: Placement,
    #[serde(default)]
    pub delivery: Delivery,
    pub rounds: u32,
    #[serde(default)]
    pub forge_review: bool,
    #[serde(default)]
    pub session_per_stage: bool,
    #[serde(default)]
    pub publish_spec: bool,
    #[serde(default)]
    pub publish_report: bool,
}

pub const MIN_ROUNDS: u32 = 1;
pub const MAX_ROUNDS: u32 = 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// `inline`, `file`, `claude_artifact`, `link` or `issue`.
    pub kind: String,
    /// A path, a URL, an issue reference, or empty for inline text.
    #[serde(default)]
    pub reference: String,
    /// Of the bytes Forge saved as `source.md`; `None` when only the agent can read the source.
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecRole {
    Drafter,
    Planner,
    Coder,
    Check,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecSession {
    pub role: SpecRole,
    pub session_id: SessionId,
    pub stage: SpecStage,
    #[serde(default)]
    pub round: u32,
}

/// The repository gate a run checks last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCheck {
    pub command: String,
    /// The project action marked as the gate, when that is where it came from.
    #[serde(default)]
    pub action_id: Option<ProjectActionId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRecord {
    /// `AC-n`, or `gate`.
    pub criterion: String,
    /// The ledger task it checked, by position; rounds restart with every task.
    #[serde(default)]
    pub task: usize,
    pub round: u32,
    pub command: String,
    /// `None` when the check could not run, which is not a failure of the code.
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub not_verified: Option<String>,
    pub duration_ms: u64,
    /// Relative to the run's directory: `checks/<AC>-r<round>.log`.
    pub log: String,
}

impl CheckRecord {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Done,
    DoneWithConcerns,
    NeedsDecision,
    Blocked,
}

impl RunOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::DoneWithConcerns => "done_with_concerns",
            Self::NeedsDecision => "needs_decision",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecRunView {
    pub id: SpecRunId,
    pub project_id: ProjectId,
    /// The checkout the run was started from.
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub stage: SpecStage,
    #[serde(default)]
    pub route: Route,
    pub source: SourceRef,
    pub planner: AgentPick,
    pub coder: AgentPick,
    pub options: SpecOptions,
    #[serde(default)]
    pub sessions: Vec<SpecSession>,
    /// Where the coder works, once G2 placed it.
    #[serde(default)]
    pub coder_workspace_id: Option<WorkspaceId>,
    #[serde(default)]
    pub base_commit: Option<String>,
    /// Paths already modified in the checkout when the coder was placed there; the review
    /// leaves them out.
    #[serde(default)]
    pub dirty_at_launch: Vec<String>,
    #[serde(default)]
    pub ledger_run: Option<RunId>,
    #[serde(default)]
    pub tasks: Vec<TaskId>,
    #[serde(default)]
    pub current_task: usize,
    /// Review rounds started so far; the first coder attempt is round 1.
    #[serde(default)]
    pub round: u32,
    #[serde(default)]
    pub gate: Option<GateCheck>,
    #[serde(default)]
    pub checks: Vec<CheckRecord>,
    /// Where the coder's commits contradict the delivery chosen at G2.
    #[serde(default)]
    pub deviations: Vec<String>,
    /// The Forge Review started after the report, when G2 asked for one.
    #[serde(default)]
    pub review_id: Option<ReviewId>,
    /// SHA-256 of the approved spec as saved.
    #[serde(default)]
    pub spec_sha256: Option<String>,
    /// SHA-256 of the saved plan; with the spec's, what every later read is checked against.
    #[serde(default)]
    pub plan_sha256: Option<String>,
    #[serde(default)]
    pub has_research: bool,
    #[serde(default)]
    pub has_plan: bool,
    #[serde(default)]
    pub has_report: bool,
    #[serde(default)]
    pub spec_artifact_url: Option<String>,
    #[serde(default)]
    pub report_artifact_url: Option<String>,
    #[serde(default)]
    pub outcome: Option<RunOutcome>,
    /// Manual checks the person marked at G3.
    #[serde(default)]
    pub manual_checked: Vec<String>,
    /// Why the run waits on the person outside a gate (a lost coder, an undelivered prompt).
    #[serde(default)]
    pub attention: Option<String>,
    /// The last text a gate decision sent to an agent.
    #[serde(default)]
    pub last_reason: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl SpecRunView {
    #[must_use]
    pub fn needs_attention(&self) -> bool {
        self.stage.is_gate() || (self.attention.is_some() && !self.stage.is_closed())
    }

    #[must_use]
    pub fn session(&self, role: SpecRole) -> Option<SessionId> {
        self.sessions
            .iter()
            .rev()
            .find(|s| s.role == role)
            .map(|s| s.session_id)
    }

    #[must_use]
    pub fn owns_session(&self, id: SessionId) -> bool {
        self.sessions.iter().any(|s| s.session_id == id)
    }
}

/// What a spec run starts from. A link or an artifact is only named: Forge never fetches it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpecRunSource {
    Inline {
        text: String,
    },
    /// Absolute, as the client resolved it.
    File {
        path: String,
    },
    ClaudeArtifact {
        url: String,
    },
    Link {
        url: String,
    },
    /// A GitHub issue: `#123`, `123` or its link, read with `gh issue view`.
    Issue {
        reference: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpecArtifactKind {
    Spec,
    Research,
    Plan,
    /// The drafter ended without a spec.
    Exit,
}

impl SpecArtifactKind {
    #[must_use]
    pub fn file(self) -> &'static str {
        match self {
            Self::Spec => "spec.md",
            Self::Research => "research.md",
            Self::Plan => "plan.md",
            Self::Exit => "exit.md",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    Spec,
    Plan,
    Result,
}

impl GateKind {
    #[must_use]
    pub fn stage(self) -> SpecStage {
        match self {
            Self::Spec => SpecStage::SpecGate,
            Self::Plan => SpecStage::PlanGate,
            Self::Result => SpecStage::ResultGate,
        }
    }
}

/// What the person picks after G3.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NextAction {
    /// Keep the changes as they are; commit and pull requests stay the person's.
    Finish,
    /// Open the draft pull request chosen at G2, with the report as the base of its body.
    OpenDraftPr,
    /// Another coder round with this text.
    AnotherRound { text: String },
    /// Remove the coder's worktree.
    DiscardWorktree,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GateDecision {
    Approve {
        /// G1 on the short route and G2: the coder's autonomy.
        #[serde(default)]
        options: Option<SpecOptions>,
        /// G2: keep the resolved gate as the project's gate action.
        #[serde(default)]
        save_gate_action: bool,
        /// G3: the manual checks the person ran.
        #[serde(default)]
        manual_checked: Vec<String>,
        /// G3.
        #[serde(default)]
        next: Option<NextAction>,
    },
    /// Back to the stage that produced the artifact, with this text for its agent.
    Changes {
        reason: String,
    },
    Reject {
        reason: String,
    },
}

/// A spec run with the content the GUI must not read from disk itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecRunDetail {
    pub run: SpecRunView,
    #[serde(default)]
    pub spec: Option<String>,
    #[serde(default)]
    pub research: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub report: Option<String>,
    #[serde(default)]
    pub exit: Option<String>,
    /// Commands Forge runs, and the gate, as approved.
    #[serde(default)]
    pub planned_checks: Vec<(String, String)>,
    /// Manual validations from the spec, for G3.
    #[serde(default)]
    pub manual_checks: Vec<String>,
}
