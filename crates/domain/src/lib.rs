//! Shared serializable model: ids, projects, workspaces, sessions, agents,
//! and the terminal grid wire types.
//!
//! No I/O, Git, or PTY. Depends on `serde`, `uuid`, `time`, `thiserror`, and
//! `compact_str`. Glossary: `docs/domain.md`.

pub mod agent;
pub mod branch;
pub mod change;
pub mod context;
pub mod diff;
pub mod editor_frame;
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
pub mod worktree_ignore;

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
pub use editor_frame::{
    editor_modifiers, EditorDecoration, EditorDecorationKind, EditorFold, EditorFrame,
    EditorInputEvent, EditorKey, EditorMark, EditorPlace, EditorPointer, EditorRange, EditorRow,
    EditorScope, EditorSeverity, EditorSpan, MAX_EDITOR_INPUT_EVENTS, MAX_EDITOR_TEXT_BYTES,
};
pub use external::{ExternalAgentSession, ExternalTranscript, TranscriptStore};
pub use file::{
    DirectoryListing, FileContents, FileEntry, FileKind, FileTree, ImageContents, SearchKind,
    SearchMatch, SearchResults, SymlinkTarget,
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
    EditorState, Session, SessionKind, SessionRole, SessionState, SessionTitle, SessionTranscript,
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
pub use worktree_ignore::{IgnoreScope, WorktreeIgnore, MAX_WORKTREE_IGNORES};
