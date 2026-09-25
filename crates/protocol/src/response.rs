//! Successful responses to client requests.
//!
//! A [`Response`] travels inside
//! [`crate::DaemonMessage::Response`]`{ request_id, body: Ok(..) }`; failures
//! use [`crate::error::ProtocolError`] in the `Err(..)` arm. Domain objects
//! created or mutated by a request are propagated to *all* clients via
//! [`crate::event::DaemonEvent`], so most mutating requests answer with the
//! generic [`Response::Ack`] and the client learns the result from the event.

use domain::orchestration::{BoardEntry, RunView};
use domain::{
    AgentDescriptor, AgentProfile, AttemptId, BranchRef, ChangeContext, ContextEnvelope, ContextId,
    DetectionResult, DiffFile, ExternalAgentSession, ExternalTranscript, FileContents, FileTree,
    ImageContents, JuvaDraft, Project, ProjectGroup, ProjectId, ProviderUsage, PullRequestState,
    RebaseState, Remote, RunId, ScrollbackRows, SearchResults, Session, SessionChanges, SessionId,
    SessionTranscript, ShareAction, ShareCandidate, ShareRule, ShareStatusEntry, TaskId,
    TerminalId, TerminalSnapshot, UsageAnalytics, Workspace, WorkspaceDiff, WorkspaceId,
    WorkspaceReview, WorktreeIgnore,
};
use serde::{Deserialize, Serialize};

/// An agent provider and its current detection result. Bundled in
/// [`Response::Snapshot`] and [`Response::Providers`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// The declarative descriptor of the provider.
    pub descriptor: AgentDescriptor,
    /// The latest detection outcome for the provider.
    pub detection: DetectionResult,
}

/// Session counts grouped by [`domain::SessionState`].
///
/// Fixed fields rather than a map keyed on the enum: `SessionState` is
/// `#[non_exhaustive]`, and wire consumers only need the known buckets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsByState {
    /// Sessions in [`domain::SessionState::Starting`].
    pub starting: u64,
    /// Sessions in [`domain::SessionState::Running`].
    pub running: u64,
    /// Sessions in [`domain::SessionState::Exited`].
    pub exited: u64,
    /// Sessions in [`domain::SessionState::Failed`].
    pub failed: u64,
    /// Sessions in [`domain::SessionState::Orphaned`].
    pub orphaned: u64,
}

/// Runtime statistics answering [`crate::request::Request::GetStats`].
///
/// Wire-only: never persisted, never a SQLite column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStats {
    /// Seconds since the daemon process started (`Daemon::started_at`).
    pub uptime_secs: u64,
    /// Known sessions grouped by state.
    pub sessions_by_state: SessionsByState,
    /// Open PTY terminals (`Inner.terminals.len()`).
    pub open_terminals: u64,
    /// Current IPC connections, including the caller
    /// (`ClientRegistry::client_count()`).
    pub connected_clients: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Response {
    /// Generic success for a request that returns no data.
    Ack,
    /// The two sides of a refused editor save, for `GetEditorConflict`.
    ///
    /// Carried only in the answer to an explicit ask, never on `SessionUpdated`:
    /// two documents on the broadcast channel would be exactly the payload the
    /// event queue is bounded to keep off it. `Session.editor.conflict` is the
    /// flag that says this is worth asking for.
    EditorConflict {
        /// Workspace-relative path, as the daemon opened it.
        path: String,
        /// What is on disk now.
        disk: String,
        /// The draft the editor asked to write.
        mine: String,
    },
    /// A session was created and its PTY spawned, answering `CreateShellSession`
    /// / `CreateAgentSession` / `CreateChildSession`. Carries the ids the daemon
    /// just minted so the caller can `AttachTerminal` immediately instead of
    /// reloading the whole snapshot to guess which session is new.
    /// The matching `SessionCreated`/`SessionUpdated` broadcasts still keep every
    /// other client in sync.
    SessionCreated {
        /// The new session.
        session_id: SessionId,
        /// The terminal its PTY was spawned on, ready to attach.
        terminal_id: TerminalId,
    },
    /// Full initial state, answering `GetSnapshot`.
    Snapshot {
        /// All known organizational project groups.
        project_groups: Vec<ProjectGroup>,
        /// All known projects.
        projects: Vec<Project>,
        /// All known workspaces.
        workspaces: Vec<Workspace>,
        /// All known sessions.
        sessions: Vec<Session>,
        /// Agent providers with detection state.
        providers: Vec<ProviderInfo>,
        /// In provider then name order.
        agent_profiles: Vec<AgentProfile>,
        /// Every project's sharing rules, in application order. The
        /// rows are small and the settings section needs them without a round
        /// trip, exactly like `agent_profiles`.
        worktree_shares: Vec<ShareRule>,
        /// Every project's worktree-ignore rules. Same reasoning as
        /// `worktree_shares`: the GUI edits them and must show the truth after
        /// a rescan collects a tombstone without a round trip.
        worktree_ignores: Vec<WorktreeIgnore>,
        app_state: Vec<(String, String)>,
        /// Agent sessions discovered on disk that the daemon did not launch
        /// (e.g. Claude Code transcripts). Read-only history.
        external_agents: Vec<ExternalAgentSession>,
        /// Cached remote pull-request state. Producing a snapshot does not
        /// perform network I/O.
        ///
        /// Boxed so this variant does not dwarf the rest of `Response`.
        pull_requests: Box<PullRequestState>,
        /// Active runs only, with tasks and current attempts. Bounded by the
        /// runs that are still open.
        #[serde(default)]
        runs: Vec<RunView>,
        /// The last per-provider account usage the daemon read, served
        /// straight from its cache. Producing a snapshot performs **no** network
        /// I/O — a background sweeper keeps this current — so a client learns the
        /// usage without a probe on its load path. Empty until the first sweep.
        usage: Vec<ProviderUsage>,
    },
    /// A project's workspaces, answering `ListWorkspaces`.
    Workspaces(Vec<Workspace>),
    /// Answers `ListBranches`.
    Branches {
        /// Local and remote-tracking branches, newest commit first.
        branches: Vec<BranchRef>,
        /// The remotes configured in the repository; empty for a local-only
        /// repo, which is how the GUI knows to hide the fetch controls.
        remotes: Vec<Remote>,
        /// The repository's default branch (`origin/HEAD`, else `HEAD`'s), the
        /// base a new branch starts from when the user does not pick one.
        default_branch: Option<String>,
    },
    /// Ignored paths a project could share, answering `DetectShareCandidates`.
    ShareCandidates {
        /// The project scanned.
        project_id: ProjectId,
        /// What the scan found, rules first.
        candidates: Vec<ShareCandidate>,
        /// The scan stopped at its budget, so the list is a floor.
        truncated: bool,
    },
    /// What a share run did, or would do, answering `PreviewShares` and
    /// `RemoveShareRule`.
    SharePlan {
        /// The workspace the plan is for; `None` when it spans a project.
        workspace_id: Option<WorkspaceId>,
        /// One line per rule considered.
        actions: Vec<ShareAction>,
    },
    /// Per-rule state of one workspace, answering `GetShareStatus`.
    ShareStatus {
        /// The workspace inspected.
        workspace_id: WorkspaceId,
        /// One entry per enabled rule of the project.
        entries: Vec<ShareStatusEntry>,
    },
    /// A project's worktree-ignore rules, answering `ListWorktreeIgnores`
    WorktreeIgnores(Vec<WorktreeIgnore>),
    /// Agent providers with detection state, answering `ListAgentProviders`.
    Providers(Vec<ProviderInfo>),
    /// The value of an app-state key, answering `GetAppState`; `None` if unset.
    AppState {
        /// The stored value, or `None` if the key is absent.
        value: Option<String>,
    },
    /// Terminal snapshot at the attach sequence, answering `AttachTerminal`
    AttachAck {
        /// The current grid snapshot.
        snapshot: TerminalSnapshot,
    },
    /// Answers `FetchScrollback`.
    ScrollbackRows(ScrollbackRows),
    /// Answers `ListProviderUsage`.
    ProviderUsage(Vec<ProviderUsage>),
    /// Working-tree change context, answering `GetChangeContext`.
    ChangeContext(ChangeContext),
    /// Juva draft text, answering `DraftWithJuva`.
    JuvaDraft(JuvaDraft),
    /// One checkout's uncommitted changes, answering `GetWorkspaceDiff`.
    WorkspaceDiff(WorkspaceDiff),
    /// One session's changes since its baseline, answering
    /// `GetSessionChanges`.
    SessionChanges(SessionChanges),
    /// One checkout's changes since its sessions began, answering
    /// `GetWorkspaceReview`. Boxed for the reason `UsageAnalytics` is: it
    /// carries a whole diff and would otherwise set the size of every
    /// `Response` on the wire path.
    WorkspaceReview(Box<WorkspaceReview>),
    /// Plain text off a session's terminal, answering
    /// `GetSessionTranscript`.
    SessionTranscript(SessionTranscript),
    /// Context envelopes for one session, answering `ListContextEnvelopes`.
    ContextEnvelopes(Vec<ContextEnvelope>),
    /// A discovered run's conversation, answering `GetExternalTranscript`.
    ExternalTranscript(ExternalTranscript),
    /// One checkout's stopped rebase/merge state, answering `GetRebaseState`,
    /// `ContinueRebase` and `MarkConflictResolved`.
    RebaseState(RebaseState),
    /// Files under a workspace, answering `ListFiles` (ADR-012).
    FileTree(FileTree),
    DirectoryListing(domain::DirectoryListing),
    /// One file's contents, answering `ReadFile` (ADR-012).
    FileContents(FileContents),
    /// One image's bytes, answering `ReadImage` (ADR-012).
    ImageContents(ImageContents),
    /// Search hits, answering `SearchFiles` (ADR-012).
    SearchResults(SearchResults),
    /// Aggregated transcript analytics, answering `GetUsageAnalytics`
    /// Boxed: it is by far the largest variant here, and an
    /// unboxed one would grow every `Response` on the wire path.
    UsageAnalytics(Box<UsageAnalytics>),
    /// Answers `GetStats`.
    DaemonStats(DaemonStats),
    OrchestrationStatus(OrchestrationLimitsView),
    RunCreated {
        run_id: RunId,
        controller_session_id: Option<SessionId>,
        integration_workspace_id: Option<WorkspaceId>,
    },
    Runs(Vec<RunView>),
    RunView(Box<RunView>),
    TaskCreated {
        task_id: TaskId,
    },
    AttemptStarted {
        attempt_id: AttemptId,
        task_id: TaskId,
        n: u32,
        session_id: SessionId,
        terminal_id: TerminalId,
        workspace_id: WorkspaceId,
        branch: Option<String>,
        base_commit: Option<String>,
    },
    IntegrationResult {
        state: String,
        integrated_commit: Option<String>,
        conflicts: Vec<String>,
    },
    CleanupResult {
        residual: Vec<String>,
    },
    /// `CloseRun`. `residual` is everything cleanup could not remove.
    /// An empty list is the only "cleanup finished" answer.
    RunClosed {
        residual: Vec<String>,
    },
    MessagesPosted {
        ids: Vec<ContextId>,
    },
    Inbox {
        messages: Vec<ContextEnvelope>,
        more: bool,
    },
    RunStateList(Vec<BoardEntry>),
    RunStateValue(BoardEntry),
    TaskReview {
        task_id: TaskId,
        session_id: Option<SessionId>,
        base_commit: Option<String>,
        head: Option<String>,
        reported_head: Option<String>,
        dirty: bool,
        files: Vec<String>,
        summary: String,
        /// Unified patches, present only when the review asked for them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        patches: Vec<DiffFile>,
    },
}

/// The `[orchestration]` rails a `forgectl status` prints. Durations are seconds.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OrchestrationLimitsView {
    pub enabled: bool,
    pub max_active_attempts_per_run: u32,
    pub max_tasks_per_run: u32,
    pub max_run_depth: u32,
    pub max_attempts_per_task: u32,
    pub stall_after_secs: u64,
    pub agent_may_integrate: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_round_trips_through_messagepack() {
        let r = Response::Ack;
        let bytes = rmp_serde::to_vec_named(&r).unwrap();
        let back: Response = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn session_created_round_trips_through_messagepack() {
        let r = Response::SessionCreated {
            session_id: domain::SessionId::new(),
            terminal_id: domain::TerminalId::new(),
        };
        let bytes = rmp_serde::to_vec_named(&r).unwrap();
        let back: Response = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn usage_analytics_round_trips_through_messagepack() {
        let r = Response::UsageAnalytics(Box::new(UsageAnalytics {
            providers: vec![domain::ProviderAnalytics {
                provider_id: domain::AgentProviderId::new("claude"),
                tokens: domain::TokenTotals {
                    input: 1,
                    output: 2,
                    cache_write: 3,
                    cache_read: 4,
                    reasoning: 1,
                },
                sessions: 1,
                turns: 2,
                cost_micros: 42,
                unpriced_turns: 0,
                top_model: Some("claude-opus-5".to_owned()),
                worked_secs: 90,
                first_activity: domain::Timestamp::from_unix_secs(1_000),
                last_activity: domain::Timestamp::from_unix_secs(2_000),
            }],
            daily: vec![domain::DailyUsage {
                date: "2026-08-26".to_owned(),
                tokens: 10,
            }],
            window_days: 30,
            scanned: 1,
            skipped: 0,
            collected_at: domain::Timestamp::now(),
        }));
        let bytes = rmp_serde::to_vec_named(&r).unwrap();
        let back: Response = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn daemon_stats_round_trips_through_messagepack() {
        let r = Response::DaemonStats(DaemonStats {
            uptime_secs: 42,
            sessions_by_state: SessionsByState {
                starting: 1,
                running: 2,
                exited: 3,
                failed: 4,
                orphaned: 5,
            },
            open_terminals: 2,
            connected_clients: 1,
        });
        let bytes = rmp_serde::to_vec_named(&r).unwrap();
        let back: Response = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(r, back);
    }
}
