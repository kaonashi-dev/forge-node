//! Unsolicited events the daemon pushes to clients (§10.3).
//!
//! Domain events (projects/workspaces/sessions) are low-volume and broadcast to
//! every connected client. Terminal deltas go only to clients that ran
//! `AttachTerminal`; non-subscribers get coalesced `TerminalActivity` instead
//! (§10.4). Each event travels inside [`crate::DaemonMessage::Event`].
//!
//! `SessionExited` from v1 is folded into `SessionUpdated` so session state has
//! a single update path (§10.3).

use domain::{
    AgentProfile, DetectionResult, Job, JobId, JuvaDraft, Project, ProjectGroup, ProjectGroupId,
    ProjectId, ProviderUsage, PullRequestState, Session, SessionId, ShareAction, ShareRule,
    ShareTrigger, TerminalDelta, TerminalId, TerminalSnapshot, Workspace, WorkspaceId,
    WorktreeIgnore,
};
use serde::{Deserialize, Serialize};

/// Severity of a [`DaemonEvent::DaemonNotice`] (§10.3), used to decide how the
/// GUI surfaces it as a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NoticeLevel {
    /// A warning worth surfacing but not fatal.
    Warning,
    /// An error the user should see.
    Error,
    /// Informational only.
    Info,
    /// A level this build does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

/// An event pushed by the daemon (§10.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DaemonEvent {
    /// Every Forge-owned metadata row was cleared by `FactoryReset`.
    ///
    /// Clients must discard their replica and bootstrap again. Repository
    /// contents and harness files were not removed.
    FactoryReset,
    /// An organizational project group was created.
    ProjectGroupCreated(ProjectGroup),
    /// An organizational project group changed.
    ProjectGroupUpdated(ProjectGroup),
    /// An organizational project group was removed.
    ProjectGroupRemoved {
        /// The removed group.
        project_group_id: ProjectGroupId,
    },
    /// A project was added; carries its full state.
    ProjectAdded(Project),
    /// A project changed (name, git root, ...); carries its full state.
    ProjectUpdated(Project),
    /// A project was removed.
    ProjectRemoved {
        /// The removed project.
        project_id: ProjectId,
    },
    /// A workspace was created; carries its full state.
    WorkspaceCreated(Workspace),
    /// A workspace changed (branch, status, ...); carries its full state.
    WorkspaceUpdated(Workspace),
    /// A workspace was removed.
    WorkspaceRemoved {
        /// The removed workspace.
        workspace_id: WorkspaceId,
    },
    /// A session was created; carries its full state.
    SessionCreated(Session),
    /// A session changed: state, title, role, or parent (§10.3).
    SessionUpdated(Session),
    /// A session was dropped from the model by `CloseSession`/`RemoveProject`.
    ///
    /// **Correction to the plan:** §10.3 does not list this event, yet
    /// `CloseSession` (§7.3) deletes the session from the model; without it a
    /// client replica would keep a ghost entry until its next `GetSnapshot`.
    /// The enum is `#[non_exhaustive]`, so adding it is additive and safe.
    SessionRemoved {
        /// The removed session.
        session_id: SessionId,
    },
    /// Changed rows for an attached terminal (§10.5). Sent only to subscribers.
    TerminalDelta {
        /// The terminal the delta belongs to.
        terminal_id: TerminalId,
        /// The changed rows, cursor and modes at the new `seq`.
        delta: TerminalDelta,
    },
    /// A fresh snapshot after a subscriber fell behind (§10.5).
    TerminalResync {
        /// The terminal being resynced.
        terminal_id: TerminalId,
        /// The full grid snapshot to reset to.
        snapshot: TerminalSnapshot,
    },
    /// Coalesced activity indicator for non-subscribers (§10.4), for unread
    /// badges.
    TerminalActivity {
        /// The terminal that produced output.
        terminal_id: TerminalId,
    },
    /// The terminal rang the bell.
    TerminalBell {
        /// The terminal that rang.
        terminal_id: TerminalId,
    },
    /// Per-provider usage was re-read (§16.2). Carries the whole set, so a
    /// client that missed one keeps a consistent picture rather than a merge
    /// of two moments.
    ProviderUsageChanged {
        /// Usage for every provider that reported any.
        usage: Vec<ProviderUsage>,
    },
    /// The set of launch profiles changed (§13.4). Carries all of them, like
    /// the usage and detection events: a client that missed one still ends up
    /// with a consistent list rather than a merge of two moments.
    AgentProfilesChanged {
        /// Every saved profile, in provider then name order.
        profiles: Vec<AgentProfile>,
    },
    /// A project's sharing rules changed (§14.2). Carries the project's whole
    /// set, like `AgentProfilesChanged`: a client that missed one edit still
    /// ends up with a consistent list rather than a merge of two moments.
    ProjectSharesChanged {
        /// The project whose rules these are.
        project_id: ProjectId,
        /// Every rule of that project, in application order.
        rules: Vec<ShareRule>,
    },
    /// A project's worktree-ignore rules changed (§14.4). Carries the whole
    /// set, like `ProjectSharesChanged`: the rescan's GC collects a tombstone
    /// on its own, and a client that missed one edit still ends up consistent.
    ProjectWorktreeIgnoresChanged {
        /// The project whose rules these are.
        project_id: ProjectId,
        /// Every rule of that project.
        rules: Vec<WorktreeIgnore>,
    },
    /// A workspace finished being provisioned (§14.2).
    ///
    /// `ApplyShares` and `CreateWorktree` ack when the work *starts*; this is
    /// where it lands. `error` is set when the run could not be attempted at
    /// all — a per-rule failure is a `Skip`/note inside `actions`, because a
    /// worktree with three of four shares is still a usable worktree.
    SharesApplied {
        /// The workspace that was provisioned.
        workspace_id: WorkspaceId,
        /// Why the run happened.
        trigger: ShareTrigger,
        /// One line per rule considered.
        actions: Vec<ShareAction>,
        /// Set when the run itself could not proceed.
        error: Option<String>,
    },
    /// Agent detection results changed (§13.1).
    AgentDetectionChanged {
        /// The updated detection results.
        results: Vec<DetectionResult>,
    },
    /// A background `FetchRemote` finished (§14.1).
    ///
    /// Carries the outcome rather than the refs themselves: the client asks
    /// `ListBranches` again when it cares, which keeps this event small and
    /// keeps one source of truth for the list. `error` is `None` on success.
    RemoteRefsUpdated {
        /// The project whose repository was fetched.
        project_id: ProjectId,
        /// The remote that was fetched.
        remote: String,
        /// How many refs git reported as updated. `0` means "already current".
        updated: u32,
        /// Git's own message when the fetch failed — an authentication or
        /// network error the user needs to read verbatim.
        error: Option<String>,
    },
    /// A background `CreatePullRequest` finished.
    PullRequestOpened {
        /// Workspace whose branch was pushed / opened.
        workspace_id: WorkspaceId,
        /// PR URL on success.
        url: Option<String>,
        /// Failure message when `url` is `None`.
        error: Option<String>,
    },
    /// A `DraftWithJuva` finished (§16.7, §16.8).
    ///
    /// The draft always arrives: when the `[juva]` endpoint is off, unreachable
    /// or slow, this carries the deterministic local draft and `fell_back` says
    /// so. The caller asked for text, and there is always text.
    JuvaDraftReady {
        /// Workspace the draft is about.
        workspace_id: WorkspaceId,
        /// The text to put in front of the user.
        draft: JuvaDraft,
        /// The endpoint was configured but did not answer usefully.
        fell_back: bool,
    },
    /// A background `RefreshPullRequests` finished. The state is a complete
    /// replacement so clients cannot retain a mixture of refreshes.
    PullRequestsUpdated {
        /// Complete pull-request state after the refresh.
        state: PullRequestState,
    },
    /// A watched file changed on disk (ADR-012).
    ///
    /// Emitted so an open editor buffer can offer reload-or-keep rather than
    /// being overwritten in silence by an agent. The GUI re-reads with
    /// `ReadFile`; this event carries no content.
    FileChanged {
        /// Workspace the path belongs to.
        workspace_id: WorkspaceId,
        /// Workspace-relative path.
        path: String,
    },
    /// A headless run was accepted, started, or reached its end; carries its
    /// full state, like `SessionUpdated`.
    ///
    /// This is the event a harness step waits on: a job in a final state has
    /// finished, with an exit code that says how, and nobody had to read a
    /// terminal to find out.
    JobUpdated(Box<Job>),
    /// Lines a running job wrote, coalesced and summarised for reading.
    ///
    /// Broadcast to every client, like `TerminalActivity` and unlike
    /// `TerminalDelta`: a job has no subscription because it has no grid to
    /// keep in sync, and its output is text a client either follows live or
    /// reads later with `ReadJobLog`.
    ///
    /// Summarised, not verbatim: this event exists to be *watched*, and a raw
    /// `stream-json` line is not something a person reads. The provider's own
    /// words stay in the file at `Job::log_path`.
    JobOutput {
        /// The job that produced them.
        job_id: JobId,
        /// Index of the first line in this batch, counting from 0.
        from_line: u64,
        /// The lines, verbatim.
        lines: Vec<String>,
    },
    /// A harness feature row changed on disk because a step finished.
    ///
    /// The daemon advances the cycle by itself now that a step is a job that
    /// exits (`harness_runner`), so a client that is not the one who started
    /// it still learns that a spec is ready for its gate, or that a feature is
    /// done. Carries the whole row, like `SessionUpdated`.
    HarnessFeatureChanged {
        /// The project whose `harness/features.json` changed.
        project_id: ProjectId,
        /// The row after the change.
        feature: Box<domain::HarnessFeature>,
    },
    /// A daemon-level notice to surface as a notification (§10.3).
    DaemonNotice {
        /// The severity of the notice.
        level: NoticeLevel,
        /// The human-readable message.
        message: String,
    },
    /// The daemon is shutting down (§9.1).
    DaemonShuttingDown {
        /// Why the daemon is shutting down.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_round_trips_through_messagepack() {
        let ev = DaemonEvent::DaemonNotice {
            level: NoticeLevel::Warning,
            message: "path missing".to_string(),
        };
        let bytes = rmp_serde::to_vec_named(&ev).unwrap();
        let back: DaemonEvent = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn unknown_notice_level_tolerates_future_variants() {
        let back: NoticeLevel = serde_json::from_str("\"Critical\"").unwrap();
        assert_eq!(back, NoticeLevel::Unknown);
    }
}
