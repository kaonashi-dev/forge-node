//! Domain events are broadcast; terminal deltas go only to attached subscribers.

use domain::orchestration::{Attempt, Run, Task};
use domain::{
    AgentProfile, DetectionResult, EditorFrame, JuvaDraft, Project, ProjectGroup, ProjectGroupId,
    ProjectId, ProviderUsage, PullRequestState, RunId, Session, SessionId, ShareAction, ShareRule,
    ShareTrigger, TerminalDelta, TerminalId, TerminalSnapshot, Workspace, WorkspaceId,
    WorktreeIgnore,
};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DaemonEvent {
    /// Every Forge-owned metadata row was cleared by `FactoryReset`.
    ///
    /// Clients must discard their replica and bootstrap again. Repository
    /// contents were not removed.
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
    SessionUpdated(Session),
    /// Clients must drop the session without waiting for another snapshot.
    SessionRemoved {
        /// The removed session.
        session_id: SessionId,
    },
    /// A program in a terminal asked to put text on the clipboard (OSC 52).
    ///
    /// The *store* half only: OSC 52's read is never answered, because that
    /// would hand the person's clipboard to whatever is running in the PTY.
    /// Clamped in `terminal-core` before it is stored, so this is bounded by
    /// `MAX_CLIPBOARD_BYTES` and not by what a program felt like sending.
    ClipboardStore {
        terminal_id: TerminalId,
        text: String,
    },
    /// The window a DOM editor surface should mount.
    ///
    /// The editor's frame, on the rung `TerminalDelta` occupies for a
    /// terminal: a window of lines rather than a grid of cells, coalesced by
    /// the editor's own emit floor and dropped rather than queued when a
    /// client's queue is full — the next frame is a whole window, so a lagging
    /// surface recovers by catching the one after it and never by replaying.
    /// Broadcast like a domain event: an editor session has one surface, and
    /// the frame is already the size of a viewport.
    EditorFrame {
        /// The editor session the window belongs to.
        session_id: SessionId,
        frame: EditorFrame,
    },
    /// Sent only to attached subscribers.
    TerminalDelta {
        /// The terminal the delta belongs to.
        terminal_id: TerminalId,
        /// The changed rows, cursor and modes at the new `seq`.
        delta: TerminalDelta,
    },
    /// Replaces the replica after a subscriber falls behind.
    TerminalResync {
        /// The terminal being resynced.
        terminal_id: TerminalId,
        /// The full grid snapshot to reset to.
        snapshot: TerminalSnapshot,
    },
    /// Coalesced activity for non-subscribers' unread badges.
    TerminalActivity {
        /// The terminal that produced output.
        terminal_id: TerminalId,
    },
    /// The terminal rang the bell.
    TerminalBell {
        /// The terminal that rang.
        terminal_id: TerminalId,
    },
    /// Replaces the whole usage set; clients must not merge readings.
    ProviderUsageChanged {
        /// Usage for every provider that reported any.
        usage: Vec<ProviderUsage>,
    },
    /// Replaces the whole profile set.
    AgentProfilesChanged {
        /// Every saved profile, in provider then name order.
        profiles: Vec<AgentProfile>,
    },
    /// Replaces this project's whole sharing-rule set.
    ProjectSharesChanged {
        /// The project whose rules these are.
        project_id: ProjectId,
        /// Every rule of that project, in application order.
        rules: Vec<ShareRule>,
    },
    /// Replaces this project's whole ignore-rule set, including collected tombstones.
    ProjectWorktreeIgnoresChanged {
        /// The project whose rules these are.
        project_id: ProjectId,
        /// Every rule of that project.
        rules: Vec<WorktreeIgnore>,
    },
    /// Completion after `ApplyShares`/`CreateWorktree` ack; per-rule failures are
    /// `Skip` actions, while `error` means provisioning itself could not proceed.
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
    AgentDetectionChanged {
        /// The updated detection results.
        results: Vec<DetectionResult>,
    },
    /// A background `FetchRemote` finished.
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
    /// A `DraftWithJuva` finished.
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
        /// Workspace-relative path; empty means reconcile after lost watcher events.
        path: String,
    },
    DaemonNotice {
        /// The severity of the notice.
        level: NoticeLevel,
        /// The human-readable message.
        message: String,
    },
    DaemonShuttingDown {
        /// Why the daemon is shutting down.
        reason: String,
    },
    RunUpdated(Run),
    TaskUpdated(Task),
    AttemptUpdated(Attempt),
    RunRemoved {
        run_id: RunId,
    },
    /// No body. Clients re-read the inbox.
    MailboxChanged {
        session_id: Option<SessionId>,
        run_id: Option<RunId>,
        unread: u32,
    },
    /// No value. Clients re-read the key.
    RunStateChanged {
        run_id: RunId,
        key: String,
        version: u64,
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
