//! Requests a client can send to the daemon (§10.2).
//!
//! Each [`Request`] is wrapped by the caller in
//! [`crate::ClientMessage::Request`] with a `request_id`; the daemon answers
//! with a [`crate::DaemonMessage::Response`] carrying the same `request_id` and
//! a `Result<`[`crate::response::Response`]`, `[`crate::error::ProtocolError`]`>`.
//!
//! Removed relative to v1 (§10.2): `FocusSession` (pure GUI state, ADR-003) and
//! generic `Subscribe`/`Unsubscribe` (terminal subscription is `AttachTerminal`;
//! domain events broadcast to every client).

use domain::{
    AgentProfile, AgentProfileId, AgentProviderId, ChildWorkspacePolicy, ContextEnvelope,
    HarnessAdvanceAction, HarnessArtifactKind, HarnessStep, JobId, JobRequest, JuvaKind,
    ProjectGroupId, ProjectId, PtySize, SessionId, SessionKind, SessionRole, ShareCleanup,
    ShareRule, ShareRuleId, TerminalId, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What to do with worktrees and running sessions when a project is removed
/// (§10.2). Branches are never deleted by any policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RemoveProjectPolicy {
    /// Drop the project from the list; worktrees and branches stay on disk.
    /// Rejected if the project has `Running` sessions.
    KeepEverything,
    /// Kill the project's sessions but leave its worktrees on disk.
    KillSessionsKeepWorktrees,
    /// Kill the project's sessions and remove only worktrees created by Forge
    /// (`managed_by_app`); never removes branches.
    KillSessionsRemoveManagedWorktrees,
}

/// A POSIX signal a client can ask the daemon to deliver to a session (§10.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Signal {
    /// Interrupt (Ctrl-C), signal 2.
    SigInt,
    /// Terminate, signal 15.
    SigTerm,
    /// Hangup, signal 1.
    SigHup,
    /// Kill (uncatchable), signal 9.
    SigKill,
    /// A signal this build does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

impl Signal {
    /// The POSIX signal number, or `None` for [`Signal::Unknown`].
    #[must_use]
    pub const fn number(self) -> Option<i32> {
        match self {
            Signal::SigHup => Some(1),
            Signal::SigInt => Some(2),
            Signal::SigKill => Some(9),
            Signal::SigTerm => Some(15),
            Signal::Unknown => None,
        }
    }
}

/// A request from a client to the daemon (§10.2).
///
/// Variants are grouped as in the plan: Global, Projects, Workspaces, Sessions,
/// Terminals, Agents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Request {
    // ----- Global -----
    /// Full initial state (§10.2) → [`crate::response::Response::Snapshot`].
    GetSnapshot,
    /// Ask the daemon to shut down; optionally killing live sessions first.
    StopDaemon {
        /// Kill running sessions before exiting instead of refusing.
        kill_sessions: bool,
    },
    /// Restore Forge-owned state to its defaults → `Ack`;
    /// [`crate::event::DaemonEvent::FactoryReset`] when complete.
    ///
    /// Kills sessions and jobs, removes worktrees created by Forge, and clears
    /// the metadata database. Repository contents, branches, commits,
    /// `config.toml`, logs, and repository-owned `harness/` files are retained.
    FactoryReset,
    /// Read an opaque app-state key (§15.2) →
    /// [`crate::response::Response::AppState`].
    GetAppState {
        /// The app-state key.
        key: String,
    },
    /// Write an opaque app-state key (§15.2) → `Ack`.
    SetAppState {
        /// The app-state key.
        key: String,
        /// The value to store.
        value: String,
    },
    /// Refresh all pull requests in the background → `Ack` **immediately**;
    /// [`crate::event::DaemonEvent::PullRequestsUpdated`] when it finishes.
    ///
    /// The `Ack` means the refresh started (or coalesced with one already in
    /// flight), not that the remote operation completed.
    RefreshPullRequests,
    /// Runtime statistics from a live daemon (§22) →
    /// [`crate::response::Response::DaemonStats`].
    ///
    /// Synchronous and local like [`Request::GetWorkspaceDiff`]: counts under
    /// the core lock plus the client registry, no network and no broadcast.
    GetStats,

    // ----- Harness (subagent feature workflow) -----
    /// List features from `harness/features.json` at the project root.
    ListHarnessFeatures { project_id: ProjectId },
    /// One feature row from `harness/features.json`.
    GetHarnessFeature {
        project_id: ProjectId,
        feature_id: u32,
    },
    /// Parse `harness/progress/events_<id>.jsonl`.
    GetHarnessTimeline {
        project_id: ProjectId,
        feature_id: u32,
    },
    /// Read a harness markdown artefact.
    ReadHarnessArtifact {
        project_id: ProjectId,
        feature_id: u32,
        artifact: HarnessArtifactKind,
    },
    /// Register a new harness feature (`pending`).
    ///
    /// `workspace_id` is the checkout the feature will be implemented in. The
    /// state file itself is one per repository, so this is what lets two
    /// features run in two worktrees of the same project — and what the
    /// daemon checks to refuse a second open feature in a checkout that
    /// already has one (`Conflict`). `None` leaves the feature unattached,
    /// like one registered from the harness CLI.
    RegisterHarnessFeature {
        project_id: ProjectId,
        workspace_id: Option<WorkspaceId>,
        spec_raw: String,
        title: Option<String>,
    },
    /// Register from a GitHub issue via `gh`. `workspace_id` as above.
    RegisterHarnessFromIssue {
        project_id: ProjectId,
        workspace_id: Option<WorkspaceId>,
        issue_number: u32,
    },
    /// Human gate or phase transition on disk.
    ///
    /// `revision` is the row's `revision` as the client last saw it. A decision
    /// taken against a view the machine has since moved past is a no-op that
    /// answers with the current row — the same idempotence Orca gets from
    /// `--retry-request`, and what stops a replayed *Approve* from launching a
    /// second implementer in the same worktree. `None` means "apply regardless",
    /// which only a caller with no view of its own should send.
    HarnessAdvance {
        project_id: ProjectId,
        feature_id: u32,
        #[serde(default)]
        revision: Option<u64>,
        action: HarnessAdvanceAction,
    },
    /// Persist the orchestrator session on the feature row.
    LinkHarnessSession {
        project_id: ProjectId,
        feature_id: u32,
        session_id: SessionId,
    },
    /// Run `bun harness/src/validate.ts` in the project root.
    ValidateHarness { project_id: ProjectId },

    /// Run one step of the harness cycle as a job → `Response::Job`.
    ///
    /// The daemon writes the step's `*_started` event, sets the feature's
    /// status and starts the agent; when that job exits it records the outcome
    /// and starts whatever comes next, stopping at the human gate. So a client
    /// asks for this once, at the beginning, and after an approval — the rest
    /// arrives as `JobUpdated` and `HarnessFeatureChanged`.
    ///
    /// `InvalidRequest` when the feature names no checkout, or when no
    /// headless-capable agent is configured for that step.
    /// `Conflict` when a step is already running for that feature, and
    /// `InvalidRequest` when the transition table has no row for this step from
    /// the feature's current status — which is what makes the human gate
    /// unskippable rather than merely conventional.
    RunHarnessStep {
        project_id: ProjectId,
        feature_id: u32,
        step: HarnessStep,
        /// Run the step anyway, and record a `gate_bypassed` event. The GUI
        /// never sets it; it exists for an operator unsticking a state by hand.
        #[serde(default)]
        force: bool,
    },
    /// Ask a question about the harness → `Response::Job`.
    ///
    /// The daemon puts the state of every feature in front of an agent and
    /// hands it the question, as one headless run: the answer arrives on the
    /// job's own stream like any other. Nothing here is a new channel — it is
    /// the same jobs, asked a different kind of thing.
    ///
    /// `resume_from` is the `provider_session_id` of the previous answer,
    /// which is what makes it a conversation rather than a series of
    /// strangers. Dropped when the provider cannot resume a headless run.
    AskHarness {
        project_id: ProjectId,
        question: String,
        resume_from: Option<String>,
    },

    // ----- Jobs (headless agent runs) -----
    /// Start a headless run of one provider → `Response::Job`.
    ///
    /// Unlike `CreateAgentSession` this opens no terminal and holds nothing:
    /// the process runs to completion and its result is the exit code plus the
    /// event stream it wrote. Answers as soon as the job is *accepted*, which
    /// may be before it starts — jobs queue behind a concurrency limit so a
    /// fan-out cannot drain the account's rate limit in one go — and the run
    /// itself reports through `JobUpdated` / `JobOutput`.
    ///
    /// `InvalidRequest` when the provider declares no headless mode.
    StartJob { request: JobRequest },
    /// Kill a running job → `Ack`. A queued one is dropped, a finished one is
    /// left alone.
    CancelJob { job_id: JobId },
    /// Every job this daemon has run since it started → `Response::Jobs`.
    ListJobs,
    /// Read one job's event stream from disk → `Response::JobLog`.
    ///
    /// `from_line` skips lines a client already has, so a reconnecting client
    /// catches up without re-reading a long run.
    ReadJobLog { job_id: JobId, from_line: u64 },

    // ----- Projects -----
    /// Add a project directory (§14.1) → `Ack`; `ProjectAdded` broadcast.
    AddProject {
        /// Path to the directory to add.
        path: PathBuf,
    },
    /// Add a project directory to an organizational group → `Ack`.
    AddProjectToGroup {
        /// Path to the directory to add.
        path: PathBuf,
        /// Organizational group that will contain the project.
        project_group_id: ProjectGroupId,
    },
    /// Create a named organizational group → `Ack`; `ProjectGroupCreated` broadcast.
    CreateProjectGroup {
        /// Display name for the group.
        name: String,
    },
    /// Rename an organizational group → `Ack`; `ProjectGroupUpdated` broadcast.
    RenameProjectGroup {
        /// Group to rename.
        project_group_id: ProjectGroupId,
        /// New display name.
        name: String,
    },
    /// Remove an organizational group → `Ack`; its projects move to General.
    RemoveProjectGroup {
        /// Group to remove.
        project_group_id: ProjectGroupId,
    },
    /// Move a project between organizational groups, or to General → `Ack`.
    MoveProject {
        /// Project whose organization changes.
        project_id: ProjectId,
        /// Destination group, or `None` for General.
        project_group_id: Option<ProjectGroupId>,
    },
    /// Remove a project subject to `policy` (§10.2) → `Ack`.
    RemoveProject {
        /// The project to remove.
        project_id: ProjectId,
        /// How to treat worktrees and running sessions.
        policy: RemoveProjectPolicy,
    },
    /// Re-detect git root, branch and worktrees for a project → `Ack`.
    RefreshProject {
        /// The project to refresh.
        project_id: ProjectId,
    },
    /// Rename a project → `Ack`; `ProjectUpdated` broadcast.
    RenameProject {
        /// The project to rename.
        project_id: ProjectId,
        /// The new display name.
        name: String,
    },
    /// Set or clear a project's icon → `Ack`; `ProjectUpdated` broadcast.
    SetProjectIcon {
        /// The project whose mark changes.
        project_id: ProjectId,
        /// The glyph to show, or `None` to fall back to the initials the UI
        /// derives from the name. Refused with `InvalidRequest` when it is
        /// not a single drawable mark (`domain::is_valid_icon`).
        icon: Option<String>,
    },

    // ----- Workspaces -----
    /// List a project's workspaces →
    /// [`crate::response::Response::Workspaces`].
    ListWorkspaces {
        /// The owning project.
        project_id: ProjectId,
    },
    /// Create a managed git worktree (§14.3) → `Ack`; `WorkspaceCreated`.
    CreateWorktree {
        /// The owning project.
        project_id: ProjectId,
        /// Branch to check out in the new worktree.
        branch: String,
        /// Optional base ref to branch from.
        base: Option<String>,
        /// Optional explicit name/slug for the worktree.
        name: Option<String>,
    },
    /// Remove a worktree (§14.4) → `Ack`; `WorkspaceRemoved`.
    RemoveWorktree {
        /// The worktree to remove.
        workspace_id: WorkspaceId,
        /// Force removal even with a dirty working tree.
        force: bool,
    },
    /// Set or clear a workspace's human label → `Ack`; `WorkspaceUpdated`.
    ///
    /// Independent of the git branch: `None` (or an empty string after trim)
    /// clears the override so the rail falls back to the branch again.
    RenameWorkspace {
        /// The workspace to rename.
        workspace_id: WorkspaceId,
        /// New display name, or `None` to clear it.
        display_name: Option<String>,
    },
    /// Recompute git status for a workspace on demand → `Ack`;
    /// `WorkspaceUpdated`.
    RefreshWorkspaceStatus {
        /// The workspace to refresh.
        workspace_id: WorkspaceId,
    },

    // ----- Branches and remotes -----
    /// Every local and remote-tracking branch of a project (§14.3) →
    /// [`crate::response::Response::Branches`].
    ///
    /// Reads the refs already on disk; it never touches the network. Use
    /// [`Request::FetchRemote`] first to make sure they are current.
    ListBranches {
        /// The project whose repository is read.
        project_id: ProjectId,
    },
    /// Fetch a remote in the background → `Ack` **immediately**;
    /// `RemoteRefsUpdated` when it finishes.
    ///
    /// The `Ack` says "started", not "done": a fetch can take minutes and the
    /// client's request path is synchronous, so blocking on it would freeze
    /// the GUI. The outcome arrives as an event like any other change.
    ///
    /// A fetch already running for the same project is *coalesced*, not
    /// queued: the second request acks and rides the first one's result.
    FetchRemote {
        /// The project whose repository is fetched.
        project_id: ProjectId,
        /// Which remote, or `None` for `origin` (falling back to the only
        /// remote configured when it is named something else).
        remote: Option<String>,
    },

    // ----- Juva: commit / PR drafts and mutations -----
    /// Working-tree status + bounded patch for a workspace →
    /// [`crate::response::Response::ChangeContext`].
    GetChangeContext {
        /// Workspace whose checkout is inspected.
        workspace_id: WorkspaceId,
    },
    /// Read one checkout's uncommitted changes as a patch per file →
    /// [`crate::response::Response::WorkspaceDiff`].
    ///
    /// Local and synchronous like `ListBranches`: it runs `git diff`, which
    /// opens no socket, so there is nothing to ack early and report later.
    GetWorkspaceDiff {
        /// Workspace whose checkout is inspected.
        workspace_id: WorkspaceId,
        /// Context lines around each hunk. `None` takes the service default.
        context_lines: Option<u32>,
    },
    /// What one session changed since it started →
    /// [`crate::response::Response::SessionChanges`].
    ///
    /// Local and synchronous like [`Request::GetWorkspaceDiff`], and cheaper:
    /// no patches, and a fixed number of subprocesses however many files the
    /// session touched, because the panel that reads this refreshes itself.
    ///
    /// "Since it started" is the session's recorded baseline commit, so its own
    /// commits count. Two sessions in one checkout cannot be told apart by
    /// looking at the result; the answer says how many share it.
    GetSessionChanges {
        /// Session whose changes are counted.
        session_id: SessionId,
    },
    /// One checkout's changes since its sessions began, with who those were →
    /// [`crate::response::Response::WorkspaceReview`].
    ///
    /// Local and synchronous like [`Request::GetWorkspaceDiff`]. One diff, not
    /// one per session: the base is the common ancestor of every baseline the
    /// checkout's sessions carry, and the sessions are a header over it.
    ///
    /// The prose summary is not part of this answer — it opens a socket, so it
    /// travels [`Request::DraftWithJuva`]'s ack-then-event path instead.
    GetWorkspaceReview {
        /// Workspace whose checkout is reviewed.
        workspace_id: WorkspaceId,
        /// Context lines around each hunk. `None` takes the service default.
        context_lines: Option<u32>,
    },
    /// Plain text off a session's terminal →
    /// [`crate::response::Response::SessionTranscript`].
    ///
    /// Local and synchronous: the rows are already-decoded cells the daemon
    /// holds, so this is a fold, not a parse — there are no escape sequences
    /// left in them. `max_bytes` is applied while walking back from the newest
    /// row, so the budget bounds the allocation rather than trimming it after.
    GetSessionTranscript {
        /// Session whose terminal is read.
        session_id: SessionId,
        /// How many lines to read back from the bottom. `None` takes the
        /// service default.
        max_lines: Option<u32>,
        /// Hard cap on the returned text. `None` takes the service default.
        max_bytes: Option<u32>,
    },
    /// List files under a workspace → [`crate::response::Response::FileTree`].
    ///
    /// Local and synchronous like `GetWorkspaceDiff`: the daemon reads the
    /// checkout (via `git ls-files` or a walk), so there is nothing to ack
    /// early. Paths never leave the workspace root (ADR-012).
    ListFiles {
        /// Workspace whose checkout is listed.
        workspace_id: WorkspaceId,
    },
    /// Read one file → [`crate::response::Response::FileContents`].
    ///
    /// Returns a `revision` the next [`Request::WriteFile`] must present.
    ReadFile {
        /// Workspace the path is relative to.
        workspace_id: WorkspaceId,
        /// Workspace-relative path.
        path: String,
    },
    /// Write one file → `Ack`, conditioned on `expected_revision`.
    ///
    /// Rejects with [`crate::ErrorCode::PreconditionFailed`] when the on-disk
    /// content no longer matches (an agent wrote the same path). The GUI then
    /// re-reads with [`Request::ReadFile`].
    WriteFile {
        /// Workspace the path is relative to.
        workspace_id: WorkspaceId,
        /// Workspace-relative path.
        path: String,
        /// New UTF-8 contents.
        text: String,
        /// Revision returned by the last [`Request::ReadFile`].
        expected_revision: String,
    },
    /// Create an empty file or a directory → `Ack` (ADR-012).
    ///
    /// Refuses an existing path with [`crate::ErrorCode::InvalidRequest`]
    /// rather than overwriting it. There is no revision to condition a create
    /// on, so refusing is the only answer that cannot lose an agent's work —
    /// the same property [`Request::WriteFile`]'s `expected_revision` buys.
    CreatePath {
        /// Workspace the path is relative to.
        workspace_id: WorkspaceId,
        /// Workspace-relative path. Parent directories are created with it.
        path: String,
        /// Whether to make a directory rather than an empty file.
        directory: bool,
    },
    /// Move one path to another inside the same checkout → `Ack` (ADR-012).
    ///
    /// Refuses an occupied destination, and a source that is not there.
    RenamePath {
        /// Workspace both paths are relative to.
        workspace_id: WorkspaceId,
        /// Workspace-relative path that exists now.
        from: String,
        /// Workspace-relative path it becomes.
        to: String,
    },
    /// Delete one path, recursively for a directory → `Ack` (ADR-012).
    ///
    /// There is no trash and no undo: the checkout is under git, which is the
    /// real undo. The client confirms before sending this.
    DeletePath {
        /// Workspace the path is relative to.
        workspace_id: WorkspaceId,
        /// Workspace-relative path. Never the workspace root itself.
        path: String,
    },
    /// Search files by name or content → [`crate::response::Response::SearchResults`].
    SearchFiles {
        /// Workspace to search.
        workspace_id: WorkspaceId,
        /// Query string.
        query: String,
        /// Name (fuzzy path) or content (`git grep`).
        kind: domain::SearchKind,
        /// Cap on hits. `None` takes the service default.
        limit: Option<u32>,
    },
    /// Ask Juva to draft a commit message or PR description from the current
    /// changes → [`crate::response::Response::JuvaDraft`].
    DraftWithJuva {
        /// Workspace whose checkout feeds the draft.
        workspace_id: WorkspaceId,
        /// Commit message or pull-request text.
        kind: JuvaKind,
    },
    /// Read a checkout's stopped rebase/merge/cherry-pick state →
    /// [`crate::response::Response::RebaseState`].
    ///
    /// Local and synchronous like [`Request::GetWorkspaceDiff`]: it reads the
    /// index and git's own state files, opens no socket, and is answered on the
    /// spot rather than acked and reported later.
    GetRebaseState {
        /// Workspace whose checkout is inspected.
        workspace_id: WorkspaceId,
    },
    /// Continue the stopped operation (`rebase --continue` and its siblings) →
    /// [`crate::response::Response::RebaseState`] with what git left behind.
    ///
    /// Answering with the state rather than a bare `Ack` is deliberate: a
    /// continue either finishes the replay or stops on the next commit's
    /// conflicts, and the caller has to redraw either way. Git's refusal to
    /// move while paths are unmerged is one of those outcomes, not an error.
    ContinueRebase {
        /// Workspace whose operation continues.
        workspace_id: WorkspaceId,
    },
    /// Abort the stopped operation, restoring the branch git started from →
    /// `Ack`; `WorkspaceUpdated`.
    ///
    /// Destructive: every resolution made since the operation stopped goes with
    /// it. The GUI confirms before sending this.
    AbortRebase {
        /// Workspace whose operation is abandoned.
        workspace_id: WorkspaceId,
    },
    /// Stage resolved paths (`git add --`) so a continue can move →
    /// [`crate::response::Response::RebaseState`] read back after the write.
    MarkConflictResolved {
        /// Workspace the paths belong to.
        workspace_id: WorkspaceId,
        /// Checkout-relative paths. Refused with `InvalidRequest` when one is
        /// absolute, climbs out of the checkout, or could parse as an option.
        paths: Vec<String>,
    },
    /// Stage all changes in the workspace and create a local commit → `Ack`;
    /// `WorkspaceUpdated`. Does **not** push.
    CreateCommit {
        /// Workspace to commit in.
        workspace_id: WorkspaceId,
        /// Full commit message (subject + optional body).
        message: String,
    },
    /// Push the current branch and open a GitHub pull request → `Ack`
    /// immediately; [`crate::event::DaemonEvent::PullRequestOpened`] when done.
    ///
    /// Same async shape as [`Request::FetchRemote`]: push+`gh` can take a while
    /// and the GUI command channel also carries keystrokes.
    CreatePullRequest {
        /// Workspace whose branch is pushed and opened as a PR.
        workspace_id: WorkspaceId,
        /// PR title.
        title: String,
        /// PR body (markdown).
        body: String,
        /// Base branch, or `None` for the repository default / `gh` default.
        base: Option<String>,
    },

    // ----- Sessions -----
    /// Create a plain shell session → `Ack`; `SessionCreated`.
    CreateShellSession {
        /// Workspace the session runs in.
        workspace_id: WorkspaceId,
        /// Optional parent in the session graph (§8.1).
        parent: Option<SessionId>,
        /// The session's role tag.
        role: SessionRole,
    },
    /// Create an agent CLI session → `Ack`; `SessionCreated`.
    CreateAgentSession {
        /// Workspace the session runs in.
        workspace_id: WorkspaceId,
        /// The agent provider to launch.
        provider_id: AgentProviderId,
        /// Launch profile to apply, or `None` for the bare provider (§13.4).
        profile_id: Option<AgentProfileId>,
        /// Optional parent in the session graph.
        parent: Option<SessionId>,
        /// The session's role tag.
        role: SessionRole,
        /// A provider session id to re-enter rather than start fresh (§13.5).
        /// Refused when the provider declares no way to resume.
        resume: Option<String>,
        /// A prompt to hand the agent at launch rather than have the user type
        /// it (§16.8). Refused when the provider declares no way to take one.
        initial_prompt: Option<String>,
        /// Launch the provider in its own read-only mode (§16.9), which is
        /// what an automatic pull-request review starts in. Refused with
        /// `InvalidRequest` when the provider declares no such mode, the same
        /// way `resume` and `initial_prompt` are.
        #[serde(default)]
        read_only: bool,
    },
    /// Create a child session under a parent, choosing its workspace via
    /// `workspace_policy` (§8.2) → `Ack`; `SessionCreated`.
    CreateChildSession {
        /// The parent session.
        parent_session_id: SessionId,
        /// Shell or agent.
        kind: SessionKind,
        /// Provider to launch when `kind` is [`SessionKind::Agent`].
        provider_id: Option<AgentProviderId>,
        /// Launch profile to apply, or `None` for the bare provider (§13.4).
        profile_id: Option<AgentProfileId>,
        /// The child's role tag.
        role: SessionRole,
        /// Where the child should run relative to its parent.
        workspace_policy: ChildWorkspacePolicy,
        /// Optional prompt for agent children (harness implementer/reviewer).
        initial_prompt: Option<String>,
    },
    /// Kill a session's process (SIGKILL semantics) → `Ack`; `SessionUpdated`.
    KillSession {
        /// The session to kill.
        session_id: SessionId,
    },
    /// Close (remove) a session; rejected while active without a kill (§7.3)
    /// → `Ack`.
    CloseSession {
        /// The session to close.
        session_id: SessionId,
    },
    /// Restart a terminal/exited session (§7.3) → `Ack`; `SessionUpdated`.
    RestartSession {
        /// The session to restart.
        session_id: SessionId,
    },
    /// Set or clear the user title; `None` clears it, falling back to the
    /// terminal title (§7.3) → `Ack`; `SessionUpdated`.
    RenameSession {
        /// The session to rename.
        session_id: SessionId,
        /// New user title, or `None` to clear it.
        title: Option<String>,
    },
    /// Change a session's role tag → `Ack`; `SessionUpdated`.
    SetSessionRole {
        /// The session to retag.
        session_id: SessionId,
        /// The new role.
        role: SessionRole,
    },
    /// Persist a context envelope (§8.3) → `Ack`.
    CreateContextEnvelope {
        /// The envelope to store.
        envelope: ContextEnvelope,
    },

    // ----- Terminals -----
    /// Subscribe to a terminal and fetch its snapshot (§10.5) →
    /// [`crate::response::Response::AttachAck`].
    AttachTerminal {
        /// The terminal to attach to.
        terminal_id: TerminalId,
        /// The client's current PTY size; applied if it differs.
        size: PtySize,
    },
    /// Unsubscribe from a terminal → `Ack`.
    DetachTerminal {
        /// The terminal to detach from.
        terminal_id: TerminalId,
    },
    /// Write raw input bytes to a terminal's PTY → `Ack`.
    WriteTerminalInput {
        /// The target terminal.
        terminal_id: TerminalId,
        /// Bytes to write to the PTY master.
        bytes: Vec<u8>,
    },
    /// Resize a terminal's PTY; last writer wins in the MVP (§10.4) → `Ack`.
    ResizeTerminal {
        /// The target terminal.
        terminal_id: TerminalId,
        /// The new PTY size.
        size: PtySize,
    },
    /// Fetch a block of scrollback (§10.2) →
    /// [`crate::response::Response::ScrollbackRows`].
    FetchScrollback {
        /// The target terminal.
        terminal_id: TerminalId,
        /// Absolute scrollback line index of the first row (0 = oldest).
        from_line: i64,
        /// Maximum number of rows to return.
        count: u32,
    },
    /// Deliver a signal to a session's process group (§11.3) → `Ack`.
    SendSignal {
        /// The target session.
        session_id: SessionId,
        /// The signal to deliver.
        signal: Signal,
    },

    // ----- Agents -----
    /// List agent providers with detection state →
    /// [`crate::response::Response::Providers`].
    ListAgentProviders,
    /// Read the account usage every installed provider reports (§16.2) →
    /// [`crate::response::Response::ProviderUsage`]. Providers that declare no
    /// usage probe are simply absent from the answer.
    ListProviderUsage,
    /// Aggregate what the agents on this machine have actually spent, read from
    /// their own transcripts (§16.2) →
    /// [`crate::response::Response::UsageAnalytics`].
    ///
    /// Deliberately separate from [`Request::ListProviderUsage`]: that one asks
    /// the provider how much allowance is left and can only be answered by the
    /// provider, this one counts tokens off disk and can be answered with the
    /// network down. The daemon caches the scan briefly, so a page that
    /// re-renders does not re-read a month of transcripts.
    GetUsageAnalytics {
        /// How far back to scan, in days. `None` takes the reader's default,
        /// and an over-large value is clamped rather than refused.
        window_days: Option<u16>,
    },
    /// Re-run detection for one provider, or all when `None` (§13.1) → `Ack`;
    /// `AgentDetectionChanged`.
    RefreshAgentDetection {
        /// The provider to re-detect, or `None` for all.
        provider_id: Option<AgentProviderId>,
    },
    /// Override (or clear, with `None`) a provider's executable path → `Ack`.
    SetProviderExecutable {
        /// The provider to override.
        provider_id: AgentProviderId,
        /// The executable path, or `None` to remove the override.
        path: Option<PathBuf>,
    },
    /// Create a launch profile, or replace one with the same id (§13.4) →
    /// `Ack`; `AgentProfilesChanged`.
    ///
    /// One upsert rather than a create/update pair, following
    /// `CreateContextEnvelope`: the client builds the whole domain object,
    /// including its id.
    SaveAgentProfile {
        /// The profile to store.
        profile: AgentProfile,
    },
    /// Delete a launch profile (§13.4) → `Ack`; `AgentProfilesChanged`.
    ///
    /// Sessions the profile already launched keep running and keep pointing at
    /// it; only new launches are affected.
    RemoveAgentProfile {
        /// The profile to delete.
        profile_id: AgentProfileId,
    },

    // -------------------------------------------------- shared files (§14.2) ---
    /// Ignored paths a project could share → `ShareCandidates`.
    ///
    /// A local, synchronous, bounded read like `ListBranches`: one `git status
    /// --ignored` at the source checkout plus a depth-limited walk. It never
    /// opens a socket and never writes.
    DetectShareCandidates {
        /// The project to scan.
        project_id: ProjectId,
    },
    /// Replace a project's whole rule set → `Ack`; `ProjectSharesChanged`.
    ///
    /// The set, not a row: adding, reordering, enabling and re-pointing a
    /// strategy are one edit of one list. Only removal has its own request,
    /// because it is the only edit that can delete a file.
    SetProjectShares {
        /// The project whose rules these are.
        project_id: ProjectId,
        /// The rules, in the order they should apply.
        rules: Vec<ShareRule>,
    },
    /// Drop one rule and say what happens to what it already wrote →
    /// `SharePlan` (what the cleanup did); `ProjectSharesChanged`.
    RemoveShareRule {
        /// The project the rule belongs to.
        project_id: ProjectId,
        /// The rule to drop.
        rule_id: ShareRuleId,
        /// What to do with the files it already put in each workspace.
        cleanup: ShareCleanup,
    },
    /// What applying the project's rules to this workspace would do →
    /// `SharePlan`. Writes nothing.
    PreviewShares {
        /// The workspace to plan for.
        workspace_id: WorkspaceId,
    },
    /// Per-rule state of one workspace → `ShareStatus`.
    ///
    /// One `lstat` per rule, no subprocess: cheap enough for the settings
    /// section to ask for every workspace of a project.
    GetShareStatus {
        /// The workspace to inspect.
        workspace_id: WorkspaceId,
    },
    /// Apply the project's rules to a workspace → `Ack` when the work *starts*;
    /// the outcome arrives as `SharesApplied`.
    ///
    /// Asynchronous and coalesced per workspace, like `FetchRemote`: a setup
    /// script is a subprocess, and the GUI channel that carries this also
    /// carries every keystroke.
    ApplyShares {
        /// The workspace to provision.
        workspace_id: WorkspaceId,
        /// Restrict the run to these rules; `None` applies all of them.
        only: Option<Vec<ShareRuleId>>,
    },
    /// Move a real file into the project's shared store and leave a symlink in
    /// its place → `Ack`; `ProjectSharesChanged`.
    ///
    /// Its own request because it is the only operation that changes the user's
    /// primary checkout, and it backs the original up before it moves.
    AdoptIntoShareStore {
        /// The project whose store receives the file.
        project_id: ProjectId,
        /// Path relative to the repository root.
        path: String,
    },
    /// Copy a store file back into a workspace as a real file → `Ack`.
    ///
    /// The inverse of `AdoptIntoShareStore`, and what
    /// `ShareCleanup::Materialize` does for every workspace at once.
    MaterializeFromShareStore {
        /// The project whose store holds the file.
        project_id: ProjectId,
        /// Path relative to the repository root.
        path: String,
        /// The workspace to write into; `None` means every workspace.
        workspace_id: Option<WorkspaceId>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_numbers_match_posix() {
        assert_eq!(Signal::SigInt.number(), Some(2));
        assert_eq!(Signal::SigTerm.number(), Some(15));
        assert_eq!(Signal::SigHup.number(), Some(1));
        assert_eq!(Signal::SigKill.number(), Some(9));
        assert_eq!(Signal::Unknown.number(), None);
    }

    #[test]
    fn unknown_signal_tolerates_future_variants() {
        let back: Signal = serde_json::from_str("\"SIGWINCH\"").unwrap();
        assert_eq!(back, Signal::Unknown);
    }

    #[test]
    fn get_stats_round_trips_through_messagepack() {
        let r = Request::GetStats;
        let bytes = rmp_serde::to_vec_named(&r).unwrap();
        let back: Request = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn factory_reset_round_trips_through_messagepack() {
        let request = Request::FactoryReset;
        let bytes = rmp_serde::to_vec_named(&request).unwrap();
        let back: Request = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(request, back);
    }
}
