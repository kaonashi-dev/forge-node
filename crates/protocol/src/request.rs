//! Client request vocabulary, independent of transport or execution.
//! [`crate::ClientMessage::Request`] carries the identity echoed in the reply;
//! [`crate::response::Response`] defines successful results and `AttachTerminal`
//! establishes terminal subscriptions.

use domain::{
    AgentProfile, AgentProfileId, AgentProviderId, ChildWorkspacePolicy, ContextEnvelope,
    EditorFindCommand, EditorInputEvent, JuvaKind, ProjectGroupId, ProjectId, PtySize, SessionId,
    SessionKind, SessionRole, ShareCleanup, ShareRule, ShareRuleId, TerminalId, WorkspaceId,
    WorktreeIgnore,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What to do with worktrees and running sessions when a project is removed
/// Branches are never deleted by any policy.
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

/// How [`Request::SendContext`] should spawn the receiving child.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendContextSpawn {
    /// Shell or agent.
    pub kind: SessionKind,
    /// Provider when `kind` is [`SessionKind::Agent`].
    pub provider_id: Option<AgentProviderId>,
    /// Launch profile, or `None` for the bare provider.
    pub profile_id: Option<AgentProfileId>,
    /// Role tag on the child.
    pub role: SessionRole,
    /// Where the child runs relative to its parent.
    pub workspace_policy: ChildWorkspacePolicy,
}

/// A POSIX signal a client can ask the daemon to deliver to a session.
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

///
/// Variants are grouped as in the plan: Global, Projects, Workspaces, Sessions,
/// Terminals, Agents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Request {
    // ----- Global -----
    /// Full initial state → [`crate::response::Response::Snapshot`].
    GetSnapshot,
    /// Ask the daemon to shut down; optionally killing live sessions first.
    StopDaemon {
        /// Kill running sessions before exiting instead of refusing.
        kill_sessions: bool,
    },
    /// Restore Forge-owned state to its defaults → `Ack`;
    /// [`crate::event::DaemonEvent::FactoryReset`] when complete.
    ///
    /// Kills sessions, removes worktrees created by Forge, and clears the
    /// metadata database. Repository contents, branches, commits,
    /// `config.toml` and logs are retained.
    FactoryReset,
    /// Read an opaque app-state key →
    /// [`crate::response::Response::AppState`].
    GetAppState {
        /// The app-state key.
        key: String,
    },
    /// Write an opaque app-state key → `Ack`.
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
    /// Runtime statistics from a live daemon →
    /// [`crate::response::Response::DaemonStats`].
    ///
    /// Synchronous and local like [`Request::GetWorkspaceDiff`]: counts under
    /// the core lock plus the client registry, no network and no broadcast.
    GetStats,

    // ----- Projects -----
    /// Add a project directory → `Ack`; `ProjectAdded` broadcast.
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
    /// Remove a project subject to `policy` → `Ack`.
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
    /// Create a managed git worktree → `Ack`; `WorkspaceCreated`.
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
    /// Remove a worktree → `Ack`; `WorkspaceRemoved`.
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
    /// Every local and remote-tracking branch of a project →
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
    /// A discovered agent run's conversation as plain text →
    /// [`crate::response::Response::ExternalTranscript`].
    ///
    /// The on-disk counterpart of [`Request::GetSessionTranscript`]: an
    /// `ExternalAgentSession` has no PTY, so the transcript file is the only
    /// record. Named by identity rather than by path — the daemon resolves it
    /// against the runs it discovered, so a client cannot ask for an arbitrary
    /// file.
    GetExternalTranscript {
        /// The provider's own session id, as `ExternalAgentSession::session_id`.
        session_id: String,
        /// Provider slug; a session id is only unique within one.
        provider: String,
        /// The account the run was found in, `None` for the default one.
        profile_id: Option<AgentProfileId>,
        /// Turns to read back from the end. `None` takes the service default.
        max_turns: Option<u32>,
        /// Hard cap on the returned text. `None` takes the service default.
        max_bytes: Option<u32>,
    },
    /// Remove a discovered agent run's transcript from disk →
    /// [`crate::response::Response::Ack`].
    ///
    /// Resolved by identity like [`Request::GetExternalTranscript`], and
    /// refused with `InvalidRequest` when the run is recorded in a store shared
    /// with every other run of its provider, which cannot be edited one run at
    /// a time. Deletes the transcript and its sibling artifacts, never the
    /// checkout the run happened in.
    DeleteExternalSession {
        /// The provider's own session id.
        session_id: String,
        /// Provider slug.
        provider: String,
        /// The account the run was found in, `None` for the default one.
        profile_id: Option<AgentProfileId>,
    },
    /// Replace this connection's non-recursive directory watches; empty directories stop them.
    WatchFiles {
        workspace_id: WorkspaceId,
        directories: Vec<String>,
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
    /// Immediate children on disk → [`crate::response::Response::DirectoryListing`].
    /// Local and synchronous; dependency directories and `.git` are omitted.
    ListDirectory {
        /// Workspace the path is relative to.
        workspace_id: WorkspaceId,
        /// Workspace-relative directory; empty means the root.
        path: String,
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
    /// Read one image → [`crate::response::Response::ImageContents`].
    ///
    /// For a Markdown preview. Only image extensions are served, and a file
    /// over the service budget is refused rather than cut.
    ReadImage {
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
        /// `None` selects the bare provider.
        profile_id: Option<AgentProfileId>,
        /// Optional parent in the session graph.
        parent: Option<SessionId>,
        /// The session's role tag.
        role: SessionRole,
        /// Provider-owned resume id, not a Forge session id.
        /// Refused when the provider declares no way to resume.
        resume: Option<String>,
        /// A prompt to hand the agent at launch rather than have the user type
        /// it. Refused when the provider declares no way to take one.
        initial_prompt: Option<String>,
        /// Launch the provider in its own read-only mode, which is
        /// what an automatic pull-request review starts in. Refused with
        /// `InvalidRequest` when the provider declares no such mode, the same
        /// way `resume` and `initial_prompt` are.
        #[serde(default)]
        read_only: bool,
    },
    /// Open a file in a daemon-supervised `forge-editor` process
    /// → `SessionCreated`; buffer state arrives as `SessionUpdated`.
    ///
    /// The daemon reads the file through `fs-service` and hands the text to the
    /// editor over its control channel; the editor never opens the checkout —
    /// a save travels back the same way and the daemon writes it, so
    /// `read_only` is a real choice rather than the only supported one.
    CreateEditorSession {
        /// Workspace whose checkout owns the file.
        workspace_id: WorkspaceId,
        /// Workspace-relative path of the file to open.
        path: String,
        /// 1-based line to reveal, or `None` for the start of the file.
        line: Option<u32>,
        /// Open the buffer read-only: no save request is accepted from it.
        #[serde(default)]
        read_only: bool,
        /// Save on a pause, without being asked. The caller's own preference —
        /// the daemon holds no opinion about it and only carries it across.
        #[serde(default)]
        autosave: bool,
    },
    /// Turn saving-on-a-pause on or off for a live editor session → `Ack`.
    SetEditorAutosave {
        session_id: SessionId,
        autosave: bool,
    },
    /// Move the caret in a live editor session → `Ack`.
    ///
    /// The way a second jump into an already-open file behaves: it moves the
    /// caret in that session rather than opening a rival one, the same rule
    /// `editorReveal` follows in the GUI. `PreconditionFailed` when the
    /// editor's command queue is saturated — the caller retries.
    RevealInEditorSession {
        session_id: SessionId,
        /// 1-based.
        line: u32,
        /// 1-based display column, or `None` to leave the column alone.
        column: Option<u32>,
    },
    /// What the person did in a DOM editor surface → `Ack`.
    ///
    /// The surface has no PTY, so a keystroke is a named key and not an
    /// escape sequence on `RuntimeCommand::Input`. Batched: one request per
    /// input burst, never one per key. `events` is clamped to
    /// [`domain::MAX_EDITOR_INPUT_EVENTS`] and each `Text` to
    /// [`domain::MAX_EDITOR_TEXT_BYTES`] **before** anything is allocated for
    /// it. `PreconditionFailed` when the editor's command queue is saturated.
    SendEditorInput {
        session_id: SessionId,
        events: Vec<EditorInputEvent>,
    },
    /// Which lines a DOM editor surface is showing → `Ack`.
    ///
    /// The surface owns its scroll container and its line height, so it is the
    /// only side that can say what fits; the editor answers with an
    /// `EditorFrame`. `PreconditionFailed` when the command queue is
    /// saturated — the caller's next scroll frame replaces this one anyway.
    SetEditorView {
        session_id: SessionId,
        /// 0-based first line to mount, overscan included.
        first_line: u32,
        /// How many lines to mount. Clamped by the editor.
        line_count: u32,
    },
    /// Drive an editor session's find panel → `Ack`.
    ///
    /// The editor owns the query and the search; the answer is the `find` on
    /// the session's next `EditorState`. A `Set` pattern longer than
    /// [`domain::MAX_EDITOR_FIND_PATTERN_BYTES`] is `InvalidRequest`.
    /// `PreconditionFailed` when the editor's command queue is saturated.
    EditorFind {
        session_id: SessionId,
        command: EditorFindCommand,
    },
    /// The two sides of a refused editor save → `EditorConflict`.
    ///
    /// A synchronous read like `GetWorkspaceDiff`: the daemon answers from the
    /// draft it refused and the bytes that were on disk instead. `NotFound`
    /// when that session has no standing conflict, which is the ordinary case.
    GetEditorConflict { session_id: SessionId },
    /// Reload an editor's buffer from disk, discarding the draft → `Ack`.
    ///
    /// The *take disk* half of resolving a conflict. `PreconditionFailed` when
    /// the editor's command queue is saturated.
    ReloadEditorBuffer { session_id: SessionId },
    /// Write the editor's draft over whatever is on disk now → `Ack`.
    ///
    /// The *keep mine* half. The daemon already learned the disk's revision
    /// when it refused, so this is the second `Ctrl-S` by another name.
    OverwriteEditorBuffer { session_id: SessionId },
    /// Create a child session under a parent, choosing its workspace via
    /// `workspace_policy` → `Ack`; `SessionCreated`.
    CreateChildSession {
        /// The parent session.
        parent_session_id: SessionId,
        /// Shell or agent.
        kind: SessionKind,
        /// Provider to launch when `kind` is [`SessionKind::Agent`].
        provider_id: Option<AgentProviderId>,
        /// `None` selects the bare provider.
        profile_id: Option<AgentProfileId>,
        /// The child's role tag.
        role: SessionRole,
        /// Where the child should run relative to its parent.
        workspace_policy: ChildWorkspacePolicy,
        /// Optional prompt for an agent child.
        initial_prompt: Option<String>,
    },
    /// Kill a session's process (SIGKILL semantics) → `Ack`; `SessionUpdated`.
    KillSession {
        /// The session to kill.
        session_id: SessionId,
    },
    /// Close (remove) a session; rejected while active without a kill
    /// → `Ack`.
    CloseSession {
        /// The session to close.
        session_id: SessionId,
    },
    /// Restart a terminal/exited session → `Ack`; `SessionUpdated`.
    RestartSession {
        /// The session to restart.
        session_id: SessionId,
    },
    /// Set or clear the user title; `None` clears it, falling back to the
    /// terminal title → `Ack`; `SessionUpdated`.
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
    /// Persist a context envelope → `Ack`.
    CreateContextEnvelope {
        /// The envelope to store.
        envelope: ContextEnvelope,
    },
    /// Deliver context to an existing session, or spawn a child that starts
    /// with it → `Ack` when targeting an existing session;
    /// [`crate::response::Response::SessionCreated`] when spawning.
    ///
    /// Exactly one of `target_session_id` or `spawn` must be set. The daemon
    /// always persists an envelope; a running target also receives a framed
    /// paste on its PTY when it has a terminal.
    SendContext {
        /// Session that is handing context over.
        source_session_id: SessionId,
        /// Existing session to receive the envelope (and a PTY paste when live).
        target_session_id: Option<SessionId>,
        /// Spawn a child under the source instead of targeting an existing one.
        spawn: Option<SendContextSpawn>,
        /// Short human summary of what is being handed over.
        summary: Option<String>,
        /// Instructions for the receiving agent or human.
        instructions: Option<String>,
        /// When true, fold a bounded transcript excerpt into the envelope and
        /// the prompt / PTY paste.
        #[serde(default)]
        include_transcript: bool,
        /// Cap on transcript bytes when `include_transcript` is set.
        max_transcript_bytes: Option<u32>,
    },
    /// Envelopes where `session_id` is the source or the target →
    /// [`crate::response::Response::ContextEnvelopes`].
    ListContextEnvelopes {
        /// Session whose inbox and outbox are listed.
        session_id: SessionId,
    },

    // ----- Terminals -----
    /// Subscribe to a terminal and fetch its snapshot →
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
    /// Resize a terminal's PTY; last writer wins → `Ack`.
    ResizeTerminal {
        /// The target terminal.
        terminal_id: TerminalId,
        /// The new PTY size.
        size: PtySize,
    },
    /// Fetch a block of scrollback →
    /// [`crate::response::Response::ScrollbackRows`].
    FetchScrollback {
        /// The target terminal.
        terminal_id: TerminalId,
        /// Absolute scrollback line index of the first row (0 = oldest).
        from_line: i64,
        /// Maximum number of rows to return.
        count: u32,
    },
    /// Deliver a signal to a session's process group → `Ack`.
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
    /// Read the account usage every installed provider reports →
    /// [`crate::response::Response::ProviderUsage`]. Providers that declare no
    /// usage probe are simply absent from the answer.
    ListProviderUsage,
    /// Aggregate what the agents on this machine have actually spent, read from
    /// their own transcripts →
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
    /// Re-run detection for one provider, or all when `None` → `Ack`;
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
    /// Create a launch profile, or replace one with the same id →
    /// `Ack`; `AgentProfilesChanged`.
    ///
    /// One upsert rather than a create/update pair, following
    /// `CreateContextEnvelope`: the client builds the whole domain object,
    /// including its id.
    SaveAgentProfile {
        /// The profile to store.
        profile: AgentProfile,
    },
    /// Delete a launch profile → `Ack`; `AgentProfilesChanged`.
    ///
    /// Sessions the profile already launched keep running and keep pointing at
    /// it; only new launches are affected.
    RemoveAgentProfile {
        /// The profile to delete.
        profile_id: AgentProfileId,
    },

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

    /// A project's worktree-ignore rules → `WorktreeIgnores`.
    ///
    /// A local read: the daemon keeps the rules loaded, so this is a clone, not
    /// a query and certainly not a scan.
    ListWorktreeIgnores {
        /// The project whose rules to return.
        project_id: ProjectId,
    },
    /// Replace a project's whole ignore set → `Ack`;
    /// `ProjectWorktreeIgnoresChanged`, then a rescan that drops the rows the
    /// new rules cover.
    ///
    /// The set, not a row: the GUI edits the list as a list, and the rule at a
    /// path *is* the path. `created_at` is filled in by the daemon.
    SetWorktreeIgnores {
        /// The project whose rules these are.
        project_id: ProjectId,
        /// The rules, in the order they should be kept.
        rules: Vec<WorktreeIgnore>,
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
