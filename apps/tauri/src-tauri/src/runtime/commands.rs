use domain::{AgentProfileId, AgentProviderId, ProjectId, PtySize, SessionId, WorkspaceId};
use serde::Deserialize;

use super::input::KeyPress;

/// What removing a project does to its sessions and its worktrees.
///
/// A local spelling of `client::RemoveProjectPolicy` rather than the wire enum
/// itself: everything the WebView sends is `snake_case`, the wire enum is not,
/// and the wire enum is `#[non_exhaustive]` — so the mapping is written out
/// once here and a variant added upstream cannot silently become a WebView
/// vocabulary word nobody chose.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRemovalPolicy {
    /// Forget the project. Worktrees and branches stay on disk; refused by the
    /// daemon while any of its sessions are running.
    KeepEverything,
    /// Kill the project's sessions; leave every worktree on disk.
    KillSessions,
    /// Kill the sessions and remove the worktrees Forge itself created.
    KillSessionsAndWorktrees,
}

impl From<ProjectRemovalPolicy> for client::RemoveProjectPolicy {
    fn from(policy: ProjectRemovalPolicy) -> Self {
        match policy {
            ProjectRemovalPolicy::KeepEverything => Self::KeepEverything,
            ProjectRemovalPolicy::KillSessions => Self::KillSessionsKeepWorktrees,
            ProjectRemovalPolicy::KillSessionsAndWorktrees => {
                Self::KillSessionsRemoveManagedWorktrees
            }
        }
    }
}

/// User intent from the WebView.
///
/// One channel, drained on one thread, which is why nothing on it may block:
/// a synchronous network write queued behind a keystroke would freeze typing
/// (AGENTS.md, "every request that opens a socket acks when the work starts").
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeCommand {
    /// A key press for the attached terminal.
    ///
    /// `id` is the WebView's own counter, echoed back on the frame that
    /// rendered the press so it can close its latency sample (§2.9).
    Input {
        key: KeyPress,
        #[serde(default)]
        id: u64,
    },
    /// Text an input method committed. Typed, so never bracketed.
    InputText {
        text: String,
        #[serde(default)]
        id: u64,
    },
    /// Text from the clipboard, bracketed when the terminal asked for it.
    Paste {
        text: String,
        #[serde(default)]
        id: u64,
    },
    /// A mouse event for a program that asked to read the mouse (§11.6).
    ///
    /// Only sent while the terminal is in a reporting mode: the pane keeps the
    /// mouse for selection otherwise, and the *encoder* is what decides
    /// whether a given event is reportable in the mode that is actually set.
    Mouse {
        /// `left`, `middle`, `right`, `wheel_up`, `wheel_down`.
        button: String,
        /// `press`, `release`, `motion`.
        kind: String,
        /// 0-based cell coordinates; the protocol's 1-based values are the
        /// encoder's business.
        col: u16,
        row: u16,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        alt: bool,
        #[serde(default)]
        shift: bool,
    },
    /// Move the viewport `lines` into history (positive) or back towards the
    /// live output (negative).
    Scroll {
        lines: i64,
    },
    /// Jump back to the live output.
    ScrollToBottom,
    /// Re-send the whole viewport.
    ///
    /// The host connects and attaches before the WebView exists, so the frame
    /// that came with the attach had no canvas to reach. The pane asks for one
    /// when it mounts rather than waiting for the next byte of output, which on
    /// an idle shell never comes.
    Repaint,
    /// Copy a dragged range, answered with a `runtime:clipboard` event.
    ///
    /// Only the grid knows what a run's cells hold, so the text is cut here
    /// rather than reconstructed from what the canvas was given to paint.
    CopySelection {
        anchor_line: i64,
        anchor_col: usize,
        head_line: i64,
        head_col: usize,
    },
    SelectSession {
        session_id: SessionId,
    },
    NewShell {
        workspace: Option<WorkspaceId>,
    },
    NewAgent {
        provider: AgentProviderId,
        profile: Option<AgentProfileId>,
        workspace: Option<WorkspaceId>,
        /// A provider session id to re-enter instead of starting a fresh
        /// conversation (§13.5) — what a history card's Resume sends. The
        /// provider's own CLI does the re-entering; nothing here replays a
        /// transcript.
        #[serde(default)]
        resume: Option<String>,
        /// Hand the agent a prompt to start from rather than an empty shell (§16.8).
        #[serde(default)]
        prompt: Option<String>,
        /// The session this was handed off from, recorded as a graph edge so
        /// the rail nests the new run under the one it continues (ADR-010).
        #[serde(default)]
        parent: Option<SessionId>,
        /// Start the provider in its own read-only mode, which is what an
        /// automatic pull-request review runs in (§16.9).
        #[serde(default)]
        read_only: bool,
    },
    /// Ask the harness lieutenant a question; the accepted job and its output
    /// are published on dedicated runtime events.
    AskLieutenant {
        project: ProjectId,
        question: String,
        #[serde(default)]
        resume_from: Option<String>,
    },
    /// Seed a job viewer with the lines already written before it was opened.
    ReadJobLog {
        job_id: domain::JobId,
    },
    Resize {
        size: PtySize,
    },
    /// Persist one opaque GUI preference (§15.2).
    ///
    /// The daemon owns it: this writes and the value comes back on the next
    /// snapshot, rather than the WebView keeping a second copy that a second
    /// window would not see.
    SetAppState {
        key: String,
        value: String,
    },
    /// Clear Forge-owned state and managed worktrees, retaining repository files.
    FactoryReset,
    RefreshSnapshot,
    Reconnect,
    CloseSession {
        session_id: SessionId,
    },
    /// Stop a running session without removing its row (§7.3).
    KillSession {
        session_id: SessionId,
    },
    /// Bring an exited or orphaned session back with a fresh PTY (§7.3).
    RestartSession {
        session_id: SessionId,
    },
    /// Set the user title, or clear it with `null` so the terminal-reported one
    /// takes over again (§7.3).
    RenameSession {
        session_id: SessionId,
        title: Option<String>,
    },
    /// Re-measure a checkout's dirty/ahead/behind.
    RefreshWorkspaceStatus {
        workspace: WorkspaceId,
    },
    /// Create a managed git worktree (§14.3).
    CreateWorktree {
        project: ProjectId,
        branch: String,
        #[serde(default)]
        base: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Fetch remote refs for a project. Outcome arrives as `RemoteRefsUpdated`.
    FetchRemote {
        project: ProjectId,
        #[serde(default)]
        remote: Option<String>,
    },
    /// Open a project directory in a code editor.
    OpenInEditor {
        editor: String,
        path: String,
    },
    /// Reveal a path in the desktop file manager.
    OpenInFileManager {
        path: String,
    },
    /// Open a validated http(s) URL in the system browser.
    OpenUrl {
        url: String,
    },

    // ---------------------------------------------------------- projects ---
    //
    // Every one of these acks and then broadcasts, so none of them blocks the
    // channel on git: the row redraws from the event the daemon sends, not
    // from a return value. That is why they belong here and not on the
    // workbench worker, which exists for the reads that answer inline.
    /// Register a directory as a project (§10.2).
    AddProject {
        path: String,
        /// Drop it straight into a group, which is what the group's own
        /// "Add project…" means; `None` lands it in General.
        #[serde(default)]
        group: Option<domain::ProjectGroupId>,
    },
    /// Re-detect a project's git root, branches and worktrees.
    RefreshProject {
        project: ProjectId,
    },
    /// Move a project between groups, or to General with `null`.
    MoveProject {
        project: ProjectId,
        #[serde(default)]
        group: Option<domain::ProjectGroupId>,
    },
    /// Set a project's icon, or clear it with `null` so the rail falls back to
    /// the initials it derives from the name.
    SetProjectIcon {
        project: ProjectId,
        #[serde(default)]
        icon: Option<String>,
    },
    CreateProjectGroup {
        name: String,
    },
    RenameProjectGroup {
        group: domain::ProjectGroupId,
        name: String,
    },
    /// Remove a group. Project directories are untouched.
    RemoveProjectGroup {
        group: domain::ProjectGroupId,
    },
    /// Remove a project (§10.2). No policy ever deletes a branch.
    RemoveProject {
        project: ProjectId,
        policy: ProjectRemovalPolicy,
    },
    /// Set or clear a checkout's human label; `null` falls back to the branch.
    RenameWorkspace {
        workspace: WorkspaceId,
        #[serde(default)]
        display_name: Option<String>,
    },
    /// Remove a managed worktree (§14.4). Never deletes a branch.
    ///
    /// Without `force` the daemon refuses while the tree is dirty or sessions
    /// are running and says which — that message is meant to be shown before
    /// the gesture is repeated with `force`.
    RemoveWorktree {
        workspace: WorkspaceId,
        #[serde(default)]
        force: bool,
    },

    // ------------------------------------------------------------ agents ---
    /// Re-probe the agent CLIs; `null` refreshes every provider (§13.1).
    RefreshDetection {
        #[serde(default)]
        provider: Option<AgentProviderId>,
    },
    /// Create or replace a launch profile (§13.4).
    SaveAgentProfile {
        profile: domain::AgentProfile,
    },
    RemoveAgentProfile {
        profile: AgentProfileId,
    },
    /// Replace a project's whole file-sharing rule set (§14.2).
    SetProjectShares {
        project: ProjectId,
        rules: Vec<domain::ShareRule>,
    },
    /// Drop one rule, saying what happens to the files it already wrote.
    RemoveShareRule {
        project: ProjectId,
        rule: domain::ShareRuleId,
        #[serde(default)]
        cleanup: domain::ShareCleanup,
    },
    /// Pin the executable used for a provider, or clear the override.
    SetProviderExecutable {
        provider: AgentProviderId,
        #[serde(default)]
        path: Option<String>,
    },

    // ----------------------------------------------------------- harness ---
    /// Bind a feature to the orchestrator session running it (§harness).
    LinkHarnessSession {
        project: ProjectId,
        feature: u32,
        session_id: SessionId,
    },
    /// Kill a running job, or drop a queued one.
    CancelJob {
        job_id: domain::JobId,
    },
    /// Attach a second, small terminal to watch a harness session without
    /// leaving the feature tab (§5.8).
    ///
    /// One at a time: attaching replaces whatever the preview held, which is
    /// what makes clicking through a feature's sessions cheap.
    AttachHarnessPreview {
        session_id: SessionId,
        #[serde(default)]
        size: Option<PtySize>,
    },
    DetachHarnessPreview,
    /// Re-size the preview's PTY to what its canvas can paint.
    ///
    /// Dropped while nothing is attached: the pane sends this from a
    /// `ResizeObserver`, which can fire before the attach it belongs to.
    ResizeHarnessPreview {
        size: PtySize,
    },
    /// A key press for the preview terminal.
    InputPreview {
        key: KeyPress,
    },

    /// Ask the daemon to stop. `kill_sessions` takes the running PTYs with it.
    StopDaemon {
        #[serde(default)]
        kill_sessions: bool,
    },
}
