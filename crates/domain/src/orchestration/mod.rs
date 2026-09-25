//! Orchestration ledger types and the pure decisions the daemon enforces.
//!
//! No I/O. The daemon loads rows, calls these functions, and performs effects.
//! Nothing here chooses the next task, retries, or merges into a default branch.

mod policy;
mod prompt;

pub use policy::*;
pub use prompt::*;

use crate::ids::{
    AgentProfileId, AgentProviderId, AttemptId, ProjectId, RunId, SessionId, TaskId, Timestamp,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};

/// Bytes, checked before a copy is stored.
pub const MAX_OBJECTIVE_BYTES: usize = 8 * 1024;
pub const MAX_BRIEF_BYTES: usize = 16 * 1024;
pub const MAX_TITLE_BYTES: usize = 200;
pub const MAX_SPEC_BYTES: usize = 32 * 1024;
pub const MAX_ACCEPTANCE_BYTES: usize = 8 * 1024;
pub const MAX_SUMMARY_BYTES: usize = 16 * 1024;
pub const MAX_VERIFICATION_BYTES: usize = 4 * 1024;
pub const MAX_RESULT_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_BOARD_VALUE_BYTES: usize = 16 * 1024;
pub const MAX_BOARD_KEYS: usize = 256;
pub const MAX_BOARD_KEY_BYTES: usize = 128;
pub const MAX_PROMPT_BYTES: usize = 48 * 1024;
pub const MAX_CONTEXT_DIGEST_BYTES: usize = 64 * 1024;
pub const INBOX_PAGE: usize = 50;
pub const MIN_ID_PREFIX: usize = 8;

pub const DEFAULT_MAX_ACTIVE_ATTEMPTS: u32 = 4;
pub const DEFAULT_MAX_TASKS: u32 = 32;
pub const DEFAULT_MAX_RUN_DEPTH: u32 = 2;
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
pub const DEFAULT_STALL_AFTER_SECS: u64 = 900;
pub const WAITING_ATTENTION_SECS: u64 = 60;
pub const STARTING_ATTENTION_SECS: u64 = 90;

/// What an agent controller may do. A human caller is never limited by this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IntegratePermission {
    /// May not merge into the integration branch or open a pull request.
    None,
    /// May merge into the integration branch. May not open a pull request.
    Merge,
    /// May merge into the integration branch and open one pull request.
    /// Nothing in Forge merges that pull request.
    #[default]
    Pr,
    #[serde(other)]
    Unrecognized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunStatus {
    Active,
    Completed,
    Cancelled,
    Interrupted,
    #[serde(other)]
    Unrecognized,
}

impl RunStatus {
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Self::Active | Self::Interrupted)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskStatus {
    Pending,
    Ready,
    Active,
    Review,
    Accepted,
    Integrated,
    Blocked,
    Cancelled,
    Failed,
    #[serde(other)]
    Unrecognized,
}

impl TaskStatus {
    /// A dependent may start only after the predecessor was accepted or merged.
    #[must_use]
    pub fn satisfies_dependency(self) -> bool {
        matches!(self, Self::Accepted | Self::Integrated)
    }

    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::Integrated | Self::Cancelled | Self::Failed | Self::Unrecognized
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkMode {
    /// Own worktree by default. One writer per workspace.
    Write,
    /// Provider read-only mode. May share a checkout.
    ReadOnly,
    #[serde(other)]
    Unrecognized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReportOutcome {
    Done,
    Failed,
    Blocked,
    #[serde(other)]
    Unrecognized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttemptPhase {
    Starting,
    Running,
    Reported,
    Rejected,
    Lost,
    #[serde(other)]
    Unrecognized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LostReason {
    ExitedWithoutReport,
    DaemonRestarted,
    Cancelled,
    SpawnFailed,
    #[serde(other)]
    Unrecognized,
}

/// Who drives the run. `None` is a human at a shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ControllerSpec {
    None,
    /// The calling session (`FORGE_SESSION_ID`).
    SelfSession,
    Agent {
        provider_id: AgentProviderId,
        profile_id: Option<AgentProfileId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IntegrationPlacement {
    /// New managed worktree `forge/run-<slug>` from `base`.
    New,
    /// The controller's workspace, or the project's main checkout.
    Current,
    Workspace(WorkspaceId),
}

/// A controller's verdict. Retry is a new attempt, not a second write on the old one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskDecision {
    Accept {
        note: Option<String>,
    },
    Reject {
        feedback: String,
        retry: bool,
    },
    Cancel {
        kill: bool,
    },
    #[serde(other)]
    Unrecognized,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttemptPlacement {
    /// New managed worktree branched from the integration HEAD.
    Worktree,
    /// The run's integration workspace.
    Integration,
    /// The controller's workspace.
    Same,
    Workspace(WorkspaceId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub project_id: ProjectId,
    pub objective: String,
    pub brief: String,
    pub brief_version: u64,
    pub controller_session_id: Option<SessionId>,
    pub integration_workspace_id: Option<WorkspaceId>,
    pub base: Option<String>,
    pub parent_attempt_id: Option<AttemptId>,
    pub status: RunStatus,
    pub revision: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub run_id: RunId,
    pub title: String,
    pub spec: String,
    pub acceptance: String,
    pub after: Vec<TaskId>,
    pub mode: WorkMode,
    pub allow_subruns: bool,
    pub status: TaskStatus,
    pub attempts_used: u32,
    pub revision: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Feedback from the latest reject, included in the next attempt's prompt.
    #[serde(default)]
    pub feedback: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptReport {
    pub outcome: ReportOutcome,
    pub summary: String,
    pub verification: Option<String>,
    pub result_path: Option<String>,
    pub reported_head: Option<String>,
    pub dirty_at_report: bool,
    pub files_changed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: AttemptId,
    pub task_id: TaskId,
    pub n: u32,
    pub session_id: Option<SessionId>,
    pub workspace_id: Option<WorkspaceId>,
    pub branch: Option<String>,
    pub base_commit: Option<String>,
    pub provider_id: AgentProviderId,
    pub profile_id: Option<AgentProfileId>,
    pub read_only: bool,
    pub phase: AttemptPhase,
    pub outcome: Option<ReportOutcome>,
    pub lost_reason: Option<LostReason>,
    pub report: Option<AttemptReport>,
    pub integrated_commit: Option<String>,
    pub created_at: Timestamp,
    pub settled_at: Option<Timestamp>,
}

impl Attempt {
    #[must_use]
    pub fn is_live(&self) -> bool {
        matches!(self.phase, AttemptPhase::Starting | AttemptPhase::Running)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardEntry {
    pub run_id: RunId,
    pub key: String,
    pub value_json: String,
    pub version: u64,
    pub updated_by: Option<SessionId>,
    pub updated_at: Timestamp,
}

/// One run as a client reads it. Attention is derived, never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunView {
    pub run: Run,
    pub tasks: Vec<Task>,
    pub attempts: Vec<Attempt>,
    pub attention: Vec<AttentionItem>,
    pub board_keys: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttentionKind {
    Question,
    Reported,
    Waiting,
    Stalled,
    Starting,
    ExitedWithoutReport,
    IntegrationConflict,
    #[serde(other)]
    Unrecognized,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionItem {
    pub kind: AttentionKind,
    pub task_id: Option<TaskId>,
    pub attempt_id: Option<AttemptId>,
    pub message_id: Option<crate::ids::ContextId>,
    pub text: Option<String>,
    pub outcome: Option<ReportOutcome>,
}

/// Rails the daemon applies. The always-on rules are not fields here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestrationLimits {
    pub max_active_attempts_per_run: u32,
    pub max_tasks_per_run: u32,
    pub max_run_depth: u32,
    pub max_attempts_per_task: u32,
    pub stall_after: std::time::Duration,
    pub waiting_attention_after: std::time::Duration,
    pub starting_attention_after: std::time::Duration,
    pub agent_may_integrate: IntegratePermission,
}

impl Default for OrchestrationLimits {
    fn default() -> Self {
        Self {
            max_active_attempts_per_run: DEFAULT_MAX_ACTIVE_ATTEMPTS,
            max_tasks_per_run: DEFAULT_MAX_TASKS,
            max_run_depth: DEFAULT_MAX_RUN_DEPTH,
            max_attempts_per_task: DEFAULT_MAX_ATTEMPTS,
            stall_after: std::time::Duration::from_secs(DEFAULT_STALL_AFTER_SECS),
            waiting_attention_after: std::time::Duration::from_secs(WAITING_ATTENTION_SECS),
            starting_attention_after: std::time::Duration::from_secs(STARTING_ATTENTION_SECS),
            agent_may_integrate: IntegratePermission::Pr,
        }
    }
}

/// A guard the daemon refused. `rail` is the stable suffix of `policy:<rail>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailRefusal {
    pub rail: &'static str,
    pub message: String,
}

impl RailRefusal {
    fn new(rail: &'static str, message: impl Into<String>) -> Self {
        Self {
            rail,
            message: message.into(),
        }
    }
}
