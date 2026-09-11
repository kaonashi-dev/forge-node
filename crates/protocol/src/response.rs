//! Successful responses to client requests (§10.2/§10.1).
//!
//! A [`Response`] travels inside
//! [`crate::DaemonMessage::Response`]`{ request_id, body: Ok(..) }`; failures
//! use [`crate::error::ProtocolError`] in the `Err(..)` arm. Domain objects
//! created or mutated by a request are propagated to *all* clients via
//! [`crate::event::DaemonEvent`], so most mutating requests answer with the
//! generic [`Response::Ack`] and the client learns the result from the event.

use domain::{
    AgentDescriptor, AgentProfile, BranchRef, ChangeContext, ContextEnvelope, DetectionResult,
    ExternalAgentSession, ExternalTranscript, FileContents, FileTree, HarnessEvent, HarnessFeature,
    HarnessFeatureList, Job, JuvaDraft, Project, ProjectGroup, ProjectId, ProviderUsage,
    PullRequestState, RebaseState, Remote, ScrollbackRows, SearchResults, Session, SessionChanges,
    SessionId, SessionTranscript, ShareAction, ShareCandidate, ShareRule, ShareStatusEntry,
    TerminalId, TerminalSnapshot, UsageAnalytics, Workspace, WorkspaceDiff, WorkspaceId,
    WorkspaceReview,
};
use serde::{Deserialize, Serialize};

/// An agent provider and its current detection result (§7.5, §13.1). Bundled in
/// [`Response::Snapshot`] and [`Response::Providers`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// The declarative descriptor of the provider.
    pub descriptor: AgentDescriptor,
    /// The latest detection outcome for the provider.
    pub detection: DetectionResult,
}

/// Session counts grouped by [`domain::SessionState`] (§22).
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

/// Runtime statistics answering [`crate::request::Request::GetStats`] (§22).
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

/// A successful response body (§10.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Response {
    /// Generic success for a request that returns no data.
    Ack,
    /// One headless run, after `StartJob` or a state change.
    Job(Box<Job>),
    /// Every headless run this daemon knows of, oldest first.
    Jobs(Vec<Job>),
    /// A slice of one job's event stream, in the readable form.
    ///
    /// The same summarised lines the live `JobOutput` event carries, so a step
    /// opened after it started reads as one stream rather than two. The
    /// **verbatim** record is the file at `Job::log_path`, which the daemon
    /// never edits.
    JobLog {
        /// Index of the first line returned, counting from 0.
        from_line: u64,
        /// The lines, one per line of the provider's stream.
        lines: Vec<String>,
        /// Whether the job has since finished, so a follower knows to stop.
        finished: bool,
    },
    /// A session was created and its PTY spawned, answering `CreateShellSession`
    /// / `CreateAgentSession` / `CreateChildSession`. Carries the ids the daemon
    /// just minted so the caller can `AttachTerminal` immediately instead of
    /// reloading the whole snapshot to guess which session is new (§10.2, L3).
    /// The matching `SessionCreated`/`SessionUpdated` broadcasts still keep every
    /// other client in sync.
    SessionCreated {
        /// The new session.
        session_id: SessionId,
        /// The terminal its PTY was spawned on, ready to attach.
        terminal_id: TerminalId,
    },
    /// Full initial state, answering `GetSnapshot` (§10.2).
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
        /// Saved launch profiles, in provider then name order (§13.4).
        agent_profiles: Vec<AgentProfile>,
        /// Every project's sharing rules (§14.2), in application order. The
        /// rows are small and the settings section needs them without a round
        /// trip, exactly like `agent_profiles`.
        worktree_shares: Vec<ShareRule>,
        /// Opaque app-state key/value pairs (§15.2).
        app_state: Vec<(String, String)>,
        /// Agent sessions discovered on disk that the daemon did not launch
        /// (e.g. Claude Code transcripts). Read-only history.
        external_agents: Vec<ExternalAgentSession>,
        /// Cached remote pull-request state. Producing a snapshot does not
        /// perform network I/O.
        pull_requests: PullRequestState,
        /// Headless runs this daemon has started, finished ones included, so a
        /// reconnecting client sees what happened while it was away. Empty
        /// after a daemon restart: a job is a process, and none survive it.
        jobs: Vec<Job>,
        /// The last per-provider account usage the daemon read (§16.2), served
        /// straight from its cache. Producing a snapshot performs **no** network
        /// I/O — a background sweeper keeps this current — so a client learns the
        /// usage without a probe on its load path. Empty until the first sweep.
        usage: Vec<ProviderUsage>,
    },
    /// A project's workspaces, answering `ListWorkspaces`.
    Workspaces(Vec<Workspace>),
    /// A project's branches, answering `ListBranches` (§14.3).
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
    /// Agent providers with detection state, answering `ListAgentProviders`.
    Providers(Vec<ProviderInfo>),
    /// The value of an app-state key, answering `GetAppState`; `None` if unset.
    AppState {
        /// The stored value, or `None` if the key is absent.
        value: Option<String>,
    },
    /// Terminal snapshot at the attach sequence, answering `AttachTerminal`
    /// (§10.5).
    AttachAck {
        /// The current grid snapshot.
        snapshot: TerminalSnapshot,
    },
    /// A block of scrollback rows, answering `FetchScrollback` (§10.2).
    ScrollbackRows(ScrollbackRows),
    /// Per-provider account usage, answering `ListProviderUsage` (§16.2).
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
    /// `ContinueRebase` and `MarkConflictResolved` (§14).
    RebaseState(RebaseState),
    /// Files under a workspace, answering `ListFiles` (ADR-012).
    FileTree(FileTree),
    /// One file's contents, answering `ReadFile` (ADR-012).
    FileContents(FileContents),
    /// Search hits, answering `SearchFiles` (ADR-012).
    SearchResults(SearchResults),
    /// Aggregated transcript analytics, answering `GetUsageAnalytics`
    /// (§16.2). Boxed: it is by far the largest variant here, and an
    /// unboxed one would grow every `Response` on the wire path.
    UsageAnalytics(Box<UsageAnalytics>),
    /// Runtime statistics, answering `GetStats` (§22).
    DaemonStats(DaemonStats),
    /// Harness features, answering `ListHarnessFeatures`.
    HarnessFeatureList(HarnessFeatureList),
    /// One harness feature, answering `GetHarnessFeature`.
    HarnessFeature(HarnessFeature),
    /// Event log, answering `GetHarnessTimeline`.
    HarnessTimeline(Vec<HarnessEvent>),
    /// Markdown artefact text, answering `ReadHarnessArtifact`.
    HarnessArtifact {
        /// File contents, empty when missing.
        text: String,
    },
    /// Result of `ValidateHarness`.
    HarnessValidate {
        /// Whether validate.ts exited 0.
        ok: bool,
        /// Combined stdout/stderr.
        output: String,
    },
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
