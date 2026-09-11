//! # domain
//!
//! Shared domain model for Forge (ForgeNode). This crate is the base of the
//! dependency graph (§17): it depends only on `serde`, `uuid`, `time` and
//! `thiserror`, and every other backend crate builds on it.
//!
//! Modules mirror §7 of the plan:
//! - [`ids`]: typed identifiers and [`ids::Timestamp`].
//! - [`project`]: [`project::Project`] (§7.1).
//! - [`workspace`]: [`workspace::Workspace`] (§7.2).
//! - [`branch`]: [`branch::BranchRef`], the git refs the picker offers (§14.3).
//! - [`session`]: [`session::Session`] and its state machine (§7.3).
//! - [`agent`]: descriptors and shared runtime types (§7.5, §7.6).
//! - [`context`]: [`context::ContextEnvelope`] (§8.3).
//! - [`change`]: working-tree context and Juva drafts for commit/PR flows.
//! - [`pull_request`]: cached remote pull-request state.
//! - [`rebase`]: the stopped sequencer of one checkout (§14).
//! - [`job`]: [`job::Job`], a headless agent run that exits when it is done.
//! - [`share`]: [`share::ShareRule`], the files a project shares between its
//!   workspaces (§14.2).

pub mod agent;
pub mod branch;
pub mod change;
pub mod context;
pub mod diff;
pub mod external;
pub mod file;
pub mod harness;
pub mod ids;
pub mod job;
pub mod pr_review;
pub mod pr_task;
pub mod project;
pub mod pull_request;
pub mod rebase;
pub mod session;
pub mod share;
pub mod terminal;
pub mod usage;
pub mod workspace;

// Re-export the most commonly used types at the crate root.
pub use agent::{
    AcpPermissionDecision, AcpPermissionPolicy, AcpSpec, AcpToolKind, AgentCapabilities,
    AgentDescriptor, AgentProfile, ChildWorkspacePolicy, ConfigDirSpec, DetectionResult,
    DetectionStatus, EnvSource, HeadlessSpec, LaunchAgentRequest, PromptStyle, ProviderUsage,
    PtySize, ResolvedEnvironment, ResumeStyle, ReviewStyle, SchemaStyle, SpawnSpec, UsageProbe,
    UsageSource, UsageWindow, VersionProbe, WorkerTransport, RESERVED_PROFILE_VARS,
};
pub use branch::{BranchRef, RefScope, Remote};
pub use change::{ChangeContext, ChangeFile, JuvaDraft, JuvaKind};
pub use context::{ContextArtifactKind, ContextArtifactRef, ContextEnvelope, GitContextRef};
pub use diff::{
    BaseOrigin, ChangeSummary, ChangeSummaryFile, CommitLine, DiffFile, DiffStatus, ReviewSession,
    SessionChanges, WorkspaceDiff, WorkspaceReview,
};
pub use external::{ExternalAgentSession, ExternalTranscript, TranscriptStore};
pub use file::{
    FileContents, FileEntry, FileKind, FileTree, ImageContents, SearchKind, SearchMatch,
    SearchResults,
};
pub use harness::{
    HarnessAdvanceAction, HarnessArtifactKind, HarnessAttempt, HarnessEvent, HarnessFeature,
    HarnessFeatureList, HarnessStep,
};
pub use ids::{
    AgentProfileId, AgentProviderId, ClientId, ContextId, JobId, ProjectGroupId, ProjectId,
    SessionId, ShareRuleId, TerminalId, Timestamp, WorkspaceId,
};
pub use job::{Job, JobRequest, JobState};
pub use pr_review::{
    builtin_recipes, compose_review_prompt, recipe_or_default, ReviewRecipe, CUSTOM_RECIPE,
};
pub use pr_task::{builtin_tasks, task_or_default, PrTask, TaskMode};
pub use project::{is_valid_icon, InvalidIcon, Project, ProjectGroup, MAX_ICON_CHARS};
pub use pull_request::{
    PullRequest, PullRequestFailure, PullRequestFailureKind, PullRequestLabel,
    PullRequestRelations, PullRequestSource, PullRequestSourceStatus, PullRequestState,
    PullRequestViewer, ReviewDecision,
};
pub use rebase::{ConflictFile, RebaseState, SequencerOp};
pub use session::{
    Session, SessionKind, SessionRole, SessionState, SessionTitle, SessionTranscript,
    MAX_GRAPH_DEPTH,
};
pub use share::{
    ShareAction, ShareCandidate, ShareClass, ShareCleanup, ShareRule, ShareState, ShareStatusEntry,
    ShareStrategy, ShareTrigger, ShareVerb, MAX_SHARE_RULES,
};
pub use terminal::{
    Cell, CellFlags, CellPatch, Color, Cursor, CursorShape, Damage, MouseMode, Row, ScrollbackRows,
    TermModes, TerminalDelta, TerminalSnapshot, DEFAULT_SCROLLBACK_TAIL,
};
pub use usage::{DailyUsage, ProviderAnalytics, TokenTotals, UsageAnalytics, MICROS_PER_USD};
pub use workspace::{Workspace, WorkspaceKind, WorkspaceStatus};
