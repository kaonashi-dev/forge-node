//! The only owner of `client::Client`: a dedicated thread, a bounded command
//! queue in, and shell `AppHandle::update` batches out. Terminal deltas never
//! take that path: they are applied in order to `SharedTerminal` and wake the
//! compositor, which redraws the terminal without rebuilding the shell.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use client::{Client, Store};
use domain::{
    AgentActivity, Session, SessionId, SessionKind, SessionRole, SessionState, SessionTitle,
    TerminalId, Timestamp, WorkspaceId,
};
use flume::{Receiver, Selector, Sender};
use protocol::{DaemonEvent, Request};

use crate::terminal::SharedTerminal;
use crate::terminal::fixture::{self, Feed};

pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RETRY: Duration = Duration::from_millis(400);
const CONNECT_ATTEMPTS: usize = 8;
const FIXTURE_CONNECT: Duration = Duration::from_millis(1);
const FIXTURE_TICK: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub enum Mode {
    Live {
        socket: PathBuf,
        daemon: Option<PathBuf>,
        spawn: bool,
    },
    Fixture {
        snapshot: Arc<Store>,
        reconnect_after: Option<Duration>,
    },
}

#[derive(Clone)]
pub struct Boot {
    pub mode: Mode,
    pub metrics: Arc<Metrics>,
    pub terminal: Arc<SharedTerminal>,
}

#[derive(Debug)]
pub enum Command {
    Activate {
        generation: u64,
        session: Option<SessionId>,
        terminal: Option<TerminalId>,
        size: Option<(u16, u16)>,
    },
    ResizeAndAttach {
        generation: u64,
        terminal: TerminalId,
        size: domain::PtySize,
    },
    Input {
        generation: u64,
        terminal: TerminalId,
        bytes: Vec<u8>,
    },
    FetchScrollback {
        generation: u64,
        terminal: TerminalId,
        from_line: i64,
        count: u32,
    },
    CreateShellSession {
        workspace_id: WorkspaceId,
    },
    RenameSession {
        session_id: SessionId,
        title: String,
    },
    OpenEditor {
        workspace_id: WorkspaceId,
        path: String,
    },
    EditorView {
        generation: u64,
        session_id: SessionId,
        first_line: u32,
        line_count: u32,
    },
    EditorInput {
        generation: u64,
        session_id: SessionId,
        events: Vec<domain::EditorInputEvent>,
    },
    Read {
        generation: u64,
        scope: crate::workbench::Scope,
        read: crate::workbench::Read,
    },
    /// A preference shared with every client, in the daemon's `app_state`.
    SetAppState {
        key: String,
        value: String,
    },
    AddProject(std::path::PathBuf),
    Usage {
        generation: u64,
        window_days: u16,
    },
    Paths {
        generation: u64,
    },
    KillSession(SessionId),
    CloseSession(SessionId),
    RebaseStep {
        scope: crate::workbench::Scope,
        step: crate::workbench::RebaseStep,
    },
    RunAction {
        workspace_id: WorkspaceId,
        action_id: domain::ProjectActionId,
    },
    Fetch {
        project_id: domain::ProjectId,
    },
    OpenPullRequest {
        workspace_id: WorkspaceId,
        title: String,
        body: String,
        /// Commit everything uncommitted with this message first.
        commit: Option<String>,
    },
    RefreshPullRequests,
    DraftCommit {
        workspace_id: WorkspaceId,
    },
    Commit {
        workspace_id: WorkspaceId,
        message: String,
    },
    CreateAgent {
        workspace_id: WorkspaceId,
        provider_id: domain::AgentProviderId,
        profile_id: Option<domain::AgentProfileId>,
        initial_prompt: Option<String>,
        /// In the provider's own review mode: a launch that may not write.
        read_only: bool,
        /// The provider's own id of a conversation to re-enter.
        resume: Option<String>,
    },
    RestartSession(SessionId),
    /// Deletes a recorded run's transcript from disk, then re-reads the
    /// snapshot: history reaches the client only on a snapshot.
    DeleteTranscript {
        session_id: String,
        provider: String,
        profile_id: Option<domain::AgentProfileId>,
    },
    CreateWorktree {
        project_id: domain::ProjectId,
        branch: String,
    },
    RemoveWorktree {
        workspace_id: WorkspaceId,
    },
    /// The daemon exits; the worker then reconnects and finds nothing.
    StopDaemon {
        kill_sessions: bool,
    },
    FactoryReset,
    RefreshAgentDetection,
    SetProviderExecutable {
        provider_id: domain::AgentProviderId,
        path: Option<std::path::PathBuf>,
    },
    SaveAgentProfile(Box<domain::AgentProfile>),
    RemoveAgentProfile(domain::AgentProfileId),
    RefreshProject(domain::ProjectId),
    RefreshWorkspaceStatus(WorkspaceId),
    RenameWorkspace {
        workspace_id: WorkspaceId,
        display_name: Option<String>,
    },
    SetProjectIcon {
        project_id: domain::ProjectId,
        icon: Option<String>,
    },
    RemoveProject {
        project_id: domain::ProjectId,
        policy: protocol::RemoveProjectPolicy,
    },
    /// A project's whole rule set, which replaces the one it had.
    SetWorktreeIgnores {
        project_id: domain::ProjectId,
        rules: Vec<domain::WorktreeIgnore>,
    },
    CreatePath {
        workspace_id: WorkspaceId,
        path: String,
        directory: bool,
    },
    RenamePath {
        workspace_id: WorkspaceId,
        from: String,
        to: String,
    },
    CopyPath {
        workspace_id: WorkspaceId,
        from: String,
        to: String,
    },
    DeletePath {
        workspace_id: WorkspaceId,
        path: String,
    },
    RefreshSnapshot,
    ForceReconnect,
    Shutdown,
    EditorFind {
        session_id: SessionId,
        command: domain::EditorFindCommand,
    },
    SetEditorAutosave {
        session_id: SessionId,
        autosave: bool,
    },
    /// The two sides of a refused save, answered as `UiEvent::EditorConflict`.
    EditorConflict {
        generation: u64,
        session_id: SessionId,
    },
    ReloadEditorBuffer(SessionId),
    OverwriteEditorBuffer(SessionId),
}

#[derive(Debug)]
pub enum UiEvent {
    Connecting {
        generation: u64,
        detail: String,
    },
    Connected {
        generation: u64,
        detail: String,
        editor_surface: protocol::EditorSurface,
    },
    Disconnected {
        generation: u64,
        reason: String,
    },
    Snapshot {
        generation: u64,
        store: Box<Store>,
        took_us: u64,
    },
    Daemon {
        generation: u64,
        event: Box<DaemonEvent>,
    },
    SessionOpened {
        generation: u64,
        session_id: SessionId,
    },
    Diff {
        generation: u64,
        scope: crate::workbench::Scope,
        result: Result<Box<domain::WorkspaceDiff>, String>,
    },
    Directory {
        generation: u64,
        scope: crate::workbench::Scope,
        path: String,
        result: Result<domain::DirectoryListing, String>,
    },
    File {
        generation: u64,
        scope: crate::workbench::Scope,
        result: Result<Box<domain::FileContents>, String>,
    },
    Search {
        generation: u64,
        scope: crate::workbench::Scope,
        query: String,
        result: Result<Vec<String>, String>,
    },
    Usage {
        generation: u64,
        window_days: u16,
        result: Result<Box<domain::UsageAnalytics>, String>,
    },
    Paths {
        generation: u64,
        result: Result<Box<(protocol::ForgePaths, std::path::PathBuf)>, String>,
    },
    Rebase {
        generation: u64,
        scope: crate::workbench::Scope,
        result: Result<Box<domain::RebaseState>, String>,
    },
    CommandResult {
        generation: u64,
        label: &'static str,
        ok: bool,
        message: String,
        took_us: u64,
    },
    /// `(disk, mine)`, the two sides of a refused editor save.
    EditorConflict {
        generation: u64,
        session_id: SessionId,
        result: Result<Box<(String, String)>, String>,
    },
}

pub type EventSink = Arc<dyn Fn(Vec<UiEvent>) + Send + Sync>;
pub type WakeSink = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Default)]
pub struct Metrics {
    ui_thread: OnceLock<ThreadId>,
    pub socket_ops: AtomicU64,
    pub ui_socket_ops: AtomicU64,
    pub subprocess_ops: AtomicU64,
    pub ui_subprocess_ops: AtomicU64,
    pub commands_sent: AtomicU64,
    pub commands_completed: AtomicU64,
    pub reconnects: AtomicU64,
    pub stale_events: AtomicU64,
    pub terminal_deltas: AtomicU64,
    pub last_snapshot_us: Mutex<Option<u64>>,
    pub last_command_us: Mutex<Option<u64>>,
    pub last_socket_thread: Mutex<Option<String>>,
}

impl Metrics {
    pub fn note_ui_thread(&self) {
        let _ = self.ui_thread.set(thread::current().id());
    }

    pub fn note_socket_op(&self) {
        self.socket_ops.fetch_add(1, Ordering::Relaxed);
        if self.is_ui_thread() {
            self.ui_socket_ops.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut thread) = self.last_socket_thread.lock() {
            *thread = Some(format!("{:?}", thread::current().id()));
        }
    }

    pub fn note_subprocess_op(&self) {
        self.subprocess_ops.fetch_add(1, Ordering::Relaxed);
        if self.is_ui_thread() {
            self.ui_subprocess_ops.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn is_ui_thread(&self) -> bool {
        self.ui_thread.get() == Some(&thread::current().id())
    }

    pub fn command_us(&self) -> Option<u64> {
        self.last_command_us.lock().ok().and_then(|value| *value)
    }

    pub fn socket_thread(&self) -> String {
        self.last_socket_thread
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .unwrap_or_else(|| "none".to_string())
    }
}

pub struct ConnectionHandle {
    tx: Sender<Command>,
    stop: Arc<AtomicBool>,
    pub metrics: Arc<Metrics>,
}

impl ConnectionHandle {
    #[cfg(test)]
    pub fn test_handle(metrics: Arc<Metrics>) -> (Self, Receiver<Command>) {
        let (tx, rx) = flume::bounded(16);
        (
            ConnectionHandle {
                tx,
                stop: Arc::new(AtomicBool::new(false)),
                metrics,
            },
            rx,
        )
    }

    pub fn create_shell_session(&self, workspace_id: WorkspaceId) -> Result<(), String> {
        self.send(Command::CreateShellSession { workspace_id })
    }

    pub fn rename_session(&self, session_id: SessionId, title: String) -> Result<(), String> {
        self.send(Command::RenameSession { session_id, title })
    }

    pub fn open_editor(&self, workspace_id: WorkspaceId, path: String) -> Result<(), String> {
        self.send(Command::OpenEditor { workspace_id, path })
    }

    pub fn editor_view(
        &self,
        generation: u64,
        session_id: SessionId,
        first_line: u32,
        line_count: u32,
    ) -> Result<(), String> {
        self.send(Command::EditorView {
            generation,
            session_id,
            first_line,
            line_count,
        })
    }

    pub fn editor_input(
        &self,
        generation: u64,
        session_id: SessionId,
        events: Vec<domain::EditorInputEvent>,
    ) -> Result<(), String> {
        self.send(Command::EditorInput {
            generation,
            session_id,
            events,
        })
    }

    pub fn read(
        &self,
        generation: u64,
        scope: crate::workbench::Scope,
        read: crate::workbench::Read,
    ) -> Result<(), String> {
        self.send(Command::Read {
            generation,
            scope,
            read,
        })
    }

    pub fn create_agent(
        &self,
        workspace_id: WorkspaceId,
        provider_id: domain::AgentProviderId,
        profile_id: Option<domain::AgentProfileId>,
        initial_prompt: Option<String>,
        read_only: bool,
    ) -> Result<(), String> {
        self.send(Command::CreateAgent {
            workspace_id,
            provider_id,
            profile_id,
            initial_prompt,
            read_only,
            resume: None,
        })
    }

    /// Re-enters a recorded conversation: the ordinary agent launch, told
    /// which one, under the account that recorded it.
    pub fn resume_agent(
        &self,
        workspace_id: WorkspaceId,
        provider_id: domain::AgentProviderId,
        profile_id: Option<domain::AgentProfileId>,
        resume: String,
    ) -> Result<(), String> {
        self.send(Command::CreateAgent {
            workspace_id,
            provider_id,
            profile_id,
            initial_prompt: None,
            read_only: false,
            resume: Some(resume),
        })
    }

    pub fn delete_transcript(
        &self,
        session_id: String,
        provider: String,
        profile_id: Option<domain::AgentProfileId>,
    ) -> Result<(), String> {
        self.send(Command::DeleteTranscript {
            session_id,
            provider,
            profile_id,
        })
    }

    pub fn restart_session(&self, session_id: SessionId) -> Result<(), String> {
        self.send(Command::RestartSession(session_id))
    }

    pub fn draft_commit(&self, workspace_id: WorkspaceId) -> Result<(), String> {
        self.send(Command::DraftCommit { workspace_id })
    }

    pub fn commit(&self, workspace_id: WorkspaceId, message: String) -> Result<(), String> {
        self.send(Command::Commit {
            workspace_id,
            message,
        })
    }

    pub fn fetch(&self, project_id: domain::ProjectId) -> Result<(), String> {
        self.send(Command::Fetch { project_id })
    }

    pub fn open_pull_request(
        &self,
        workspace_id: WorkspaceId,
        title: String,
        body: String,
        commit: Option<String>,
    ) -> Result<(), String> {
        self.send(Command::OpenPullRequest {
            workspace_id,
            title,
            body,
            commit,
        })
    }

    pub fn refresh_pull_requests(&self) -> Result<(), String> {
        self.send(Command::RefreshPullRequests)
    }

    pub fn run_action(
        &self,
        workspace_id: WorkspaceId,
        action_id: domain::ProjectActionId,
    ) -> Result<(), String> {
        self.send(Command::RunAction {
            workspace_id,
            action_id,
        })
    }

    pub fn rebase_step(
        &self,
        scope: crate::workbench::Scope,
        step: crate::workbench::RebaseStep,
    ) -> Result<(), String> {
        self.send(Command::RebaseStep { scope, step })
    }

    pub fn kill_session(&self, session_id: SessionId) -> Result<(), String> {
        self.send(Command::KillSession(session_id))
    }

    pub fn close_session(&self, session_id: SessionId) -> Result<(), String> {
        self.send(Command::CloseSession(session_id))
    }

    pub fn usage(&self, generation: u64, window_days: u16) -> Result<(), String> {
        self.send(Command::Usage {
            generation,
            window_days,
        })
    }

    pub fn paths(&self, generation: u64) -> Result<(), String> {
        self.send(Command::Paths { generation })
    }

    pub fn add_project(&self, path: std::path::PathBuf) -> Result<(), String> {
        self.send(Command::AddProject(path))
    }

    pub fn create_worktree(
        &self,
        project_id: domain::ProjectId,
        branch: String,
    ) -> Result<(), String> {
        self.send(Command::CreateWorktree { project_id, branch })
    }

    pub fn remove_worktree(&self, workspace_id: WorkspaceId) -> Result<(), String> {
        self.send(Command::RemoveWorktree { workspace_id })
    }

    pub fn refresh_project(&self, project_id: domain::ProjectId) -> Result<(), String> {
        self.send(Command::RefreshProject(project_id))
    }

    pub fn refresh_workspace_status(&self, workspace_id: WorkspaceId) -> Result<(), String> {
        self.send(Command::RefreshWorkspaceStatus(workspace_id))
    }

    pub fn rename_workspace(
        &self,
        workspace_id: WorkspaceId,
        display_name: Option<String>,
    ) -> Result<(), String> {
        self.send(Command::RenameWorkspace {
            workspace_id,
            display_name,
        })
    }

    pub fn set_project_icon(
        &self,
        project_id: domain::ProjectId,
        icon: Option<String>,
    ) -> Result<(), String> {
        self.send(Command::SetProjectIcon { project_id, icon })
    }

    pub fn remove_project(
        &self,
        project_id: domain::ProjectId,
        policy: protocol::RemoveProjectPolicy,
    ) -> Result<(), String> {
        self.send(Command::RemoveProject { project_id, policy })
    }

    pub fn set_worktree_ignores(
        &self,
        project_id: domain::ProjectId,
        rules: Vec<domain::WorktreeIgnore>,
    ) -> Result<(), String> {
        self.send(Command::SetWorktreeIgnores { project_id, rules })
    }

    pub fn create_path(
        &self,
        workspace_id: WorkspaceId,
        path: String,
        directory: bool,
    ) -> Result<(), String> {
        self.send(Command::CreatePath {
            workspace_id,
            path,
            directory,
        })
    }

    pub fn rename_path(
        &self,
        workspace_id: WorkspaceId,
        from: String,
        to: String,
    ) -> Result<(), String> {
        self.send(Command::RenamePath {
            workspace_id,
            from,
            to,
        })
    }

    pub fn copy_path(
        &self,
        workspace_id: WorkspaceId,
        from: String,
        to: String,
    ) -> Result<(), String> {
        self.send(Command::CopyPath {
            workspace_id,
            from,
            to,
        })
    }

    pub fn delete_path(&self, workspace_id: WorkspaceId, path: String) -> Result<(), String> {
        self.send(Command::DeletePath { workspace_id, path })
    }

    pub fn set_app_state(&self, key: &str, value: &str) -> Result<(), String> {
        self.send(Command::SetAppState {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    pub fn stop_daemon(&self, kill_sessions: bool) -> Result<(), String> {
        self.send(Command::StopDaemon { kill_sessions })
    }

    pub fn factory_reset(&self) -> Result<(), String> {
        self.send(Command::FactoryReset)
    }

    pub fn refresh_agent_detection(&self) -> Result<(), String> {
        self.send(Command::RefreshAgentDetection)
    }

    /// `None` clears the override.
    pub fn set_provider_executable(
        &self,
        provider_id: domain::AgentProviderId,
        path: Option<std::path::PathBuf>,
    ) -> Result<(), String> {
        self.send(Command::SetProviderExecutable { provider_id, path })
    }

    pub fn save_agent_profile(&self, profile: domain::AgentProfile) -> Result<(), String> {
        self.send(Command::SaveAgentProfile(Box::new(profile)))
    }

    pub fn remove_agent_profile(&self, profile_id: domain::AgentProfileId) -> Result<(), String> {
        self.send(Command::RemoveAgentProfile(profile_id))
    }

    pub fn refresh_snapshot(&self) -> Result<(), String> {
        self.send(Command::RefreshSnapshot)
    }

    pub fn force_reconnect(&self) -> Result<(), String> {
        self.send(Command::ForceReconnect)
    }

    pub fn editor_find(
        &self,
        session_id: SessionId,
        command: domain::EditorFindCommand,
    ) -> Result<(), String> {
        self.send(Command::EditorFind {
            session_id,
            command,
        })
    }

    pub fn set_editor_autosave(&self, session_id: SessionId, autosave: bool) -> Result<(), String> {
        self.send(Command::SetEditorAutosave {
            session_id,
            autosave,
        })
    }

    pub fn editor_conflict(&self, generation: u64, session_id: SessionId) -> Result<(), String> {
        self.send(Command::EditorConflict {
            generation,
            session_id,
        })
    }

    pub fn reload_editor_buffer(&self, session_id: SessionId) -> Result<(), String> {
        self.send(Command::ReloadEditorBuffer(session_id))
    }

    pub fn overwrite_editor_buffer(&self, session_id: SessionId) -> Result<(), String> {
        self.send(Command::OverwriteEditorBuffer(session_id))
    }

    pub fn activate(
        &self,
        generation: u64,
        session: Option<SessionId>,
        terminal: Option<TerminalId>,
        size: Option<(u16, u16)>,
    ) -> Result<(), String> {
        self.send(Command::Activate {
            generation,
            session,
            terminal,
            size,
        })
    }

    pub fn resize_and_attach(
        &self,
        generation: u64,
        terminal: TerminalId,
        size: domain::PtySize,
    ) -> Result<(), String> {
        self.send(Command::ResizeAndAttach {
            generation,
            terminal,
            size,
        })
    }

    pub fn input(
        &self,
        generation: u64,
        terminal: TerminalId,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.send(Command::Input {
            generation,
            terminal,
            bytes,
        })
    }

    pub fn fetch_scrollback(
        &self,
        generation: u64,
        terminal: TerminalId,
        from_line: i64,
        count: u32,
    ) -> Result<(), String> {
        self.send(Command::FetchScrollback {
            generation,
            terminal,
            from_line,
            count,
        })
    }

    fn send(&self, command: Command) -> Result<(), String> {
        self.metrics.commands_sent.fetch_add(1, Ordering::Relaxed);
        self.tx
            .try_send(command)
            .map_err(|error| format!("command queue: {error}"))
    }
}

impl Drop for ConnectionHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.tx.try_send(Command::Shutdown);
    }
}

pub fn spawn(boot: Arc<Boot>, sink: EventSink, wake: WakeSink) -> ConnectionHandle {
    let (tx, rx) = flume::bounded(1024);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let metrics = Arc::clone(&boot.metrics);
    let builder = thread::Builder::new().name("forge-rune-client".to_string());
    if let Err(error) = builder.spawn(move || worker(boot, sink, wake, rx, worker_stop)) {
        eprintln!("forge-ui: could not start client worker: {error}");
    }
    ConnectionHandle { tx, stop, metrics }
}

fn worker(
    boot: Arc<Boot>,
    sink: EventSink,
    wake: WakeSink,
    rx: Receiver<Command>,
    stop: Arc<AtomicBool>,
) {
    let mut generation = 0u64;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        generation = generation.saturating_add(1);
        boot.metrics.reconnects.fetch_add(1, Ordering::Relaxed);
        let outcome = match &boot.mode {
            Mode::Fixture {
                snapshot,
                reconnect_after,
            } => run_fixture(FixtureCtx {
                generation,
                snapshot,
                reconnect_after: *reconnect_after,
                sink: &sink,
                wake: &wake,
                rx: &rx,
                stop: &stop,
                metrics: &boot.metrics,
                terminal: &boot.terminal,
            }),
            Mode::Live {
                socket,
                daemon,
                spawn,
            } => run_live(LiveCtx {
                generation,
                socket,
                daemon: daemon.as_deref(),
                spawn: *spawn,
                sink: &sink,
                wake: &wake,
                rx: &rx,
                stop: &stop,
                metrics: &boot.metrics,
                terminal: &boot.terminal,
            }),
        };
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Err(reason) = outcome {
            sink(vec![UiEvent::Disconnected { generation, reason }]);
            boot.terminal.clear(generation, None);
        }
        if matches!(boot.mode, Mode::Fixture { .. }) {
            continue;
        }
        wait_offline(generation, &sink, &rx, &stop, &boot.metrics);
    }
}

fn wait_offline(
    generation: u64,
    sink: &EventSink,
    rx: &Receiver<Command>,
    stop: &AtomicBool,
    metrics: &Metrics,
) {
    let deadline = Instant::now() + RETRY;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(Command::Shutdown) => {
                stop.store(true, Ordering::Release);
                break;
            }
            Ok(command) => {
                let label = command_label(&command);
                metrics.commands_completed.fetch_add(1, Ordering::Relaxed);
                sink(vec![UiEvent::CommandResult {
                    generation,
                    label,
                    ok: false,
                    message: "not connected; command rejected".to_string(),
                    took_us: 0,
                }]);
            }
            Err(flume::RecvTimeoutError::Timeout) => break,
            Err(flume::RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::Release);
                break;
            }
        }
    }
}

struct FixtureCtx<'a> {
    generation: u64,
    snapshot: &'a Store,
    reconnect_after: Option<Duration>,
    sink: &'a EventSink,
    wake: &'a WakeSink,
    rx: &'a Receiver<Command>,
    stop: &'a AtomicBool,
    metrics: &'a Metrics,
    terminal: &'a SharedTerminal,
}

fn run_fixture(ctx: FixtureCtx<'_>) -> Result<(), String> {
    let FixtureCtx {
        generation,
        snapshot,
        reconnect_after,
        sink,
        wake,
        rx,
        stop,
        metrics,
        terminal,
    } = ctx;
    sink(vec![UiEvent::Connecting {
        generation,
        detail: "fixture: simulated socket".to_string(),
    }]);
    thread::sleep(FIXTURE_CONNECT);
    sink(vec![UiEvent::Connected {
        generation,
        detail: format!("fixture daemon, protocol {}", protocol::PROTOCOL_VERSION),
        editor_surface: protocol::EditorSurface::Cells,
    }]);

    let mut link = FixtureLink {
        store: snapshot.clone(),
        feed: Feed::new(false, false),
        echo: String::new(),
    };
    emit_snapshot(generation, &link.store, 0, sink, metrics);

    let started = Instant::now();
    let mut next_tick = started + FIXTURE_TICK;
    let disconnect_at = reconnect_after.map(|after| started + after);
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(at) = disconnect_at
            && Instant::now() >= at
        {
            return Err("fixture: simulated connection loss".to_string());
        }
        let now = Instant::now();
        if now >= next_tick {
            fixture_tick(generation, &mut link, terminal, metrics);
            wake();
            next_tick = now + FIXTURE_TICK;
        }
        let wait = next_tick
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        match rx.recv_timeout(wait) {
            Ok(Command::Shutdown) => {
                stop.store(true, Ordering::Release);
                return Ok(());
            }
            Ok(Command::ForceReconnect) => {
                return Err("fixture: reconnect requested".to_string());
            }
            Ok(Command::RefreshSnapshot) => {
                emit_snapshot(generation, &link.store, 0, sink, metrics);
            }
            Ok(command) => handle_fixture_command(
                generation, &mut link, command, sink, wake, metrics, terminal,
            ),
            Err(flume::RecvTimeoutError::Timeout) => {}
            Err(flume::RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::Release);
                return Ok(());
            }
        }
    }
}

struct FixtureLink {
    store: Store,
    feed: Feed,
    echo: String,
}

fn fixture_tick(
    generation: u64,
    link: &mut FixtureLink,
    terminal: &SharedTerminal,
    metrics: &Metrics,
) {
    let delta = {
        let slot = terminal.lock();
        let Some(grid) = slot.grid.as_ref() else {
            return;
        };
        let Some(terminal_id) = slot.terminal_id else {
            return;
        };
        (terminal_id, link.feed.delta(grid))
    };
    let (terminal_id, delta) = delta;
    let _ = terminal.apply_delta(generation, terminal_id, &delta);
    metrics.terminal_deltas.fetch_add(1, Ordering::Relaxed);
}

fn handle_fixture_command(
    generation: u64,
    link: &mut FixtureLink,
    command: Command,
    sink: &EventSink,
    wake: &WakeSink,
    metrics: &Metrics,
    terminal: &SharedTerminal,
) {
    let started = Instant::now();
    match command {
        Command::Activate {
            generation: command_generation,
            session,
            terminal: terminal_id,
            size,
        } => {
            if command_generation < generation {
                return;
            }
            if let Some(terminal_id) = terminal_id {
                let size = size.unwrap_or((80, 24));
                let already = {
                    let slot = terminal.lock();
                    slot.generation == generation
                        && slot.terminal_id == Some(terminal_id)
                        && slot
                            .size
                            .is_some_and(|pty| pty.cols == size.0 && pty.rows == size.1)
                };
                if already {
                    return;
                }
                let title = session
                    .and_then(|session| {
                        link.store
                            .sessions
                            .iter()
                            .find(|row| row.id == session)
                            .map(|row| row.title.resolve("fixture shell").to_string())
                    })
                    .unwrap_or_else(|| "fixture shell".to_string());
                let grid = fixture::titled_grid(size.0, size.1, &title);
                let pty = grid.size;
                if let Some(session) = session {
                    terminal.publish_grid(generation, session, terminal_id, grid, pty);
                }
                wake();
            } else {
                terminal.clear(generation, session);
                wake();
            }
        }
        Command::ResizeAndAttach {
            terminal: terminal_id,
            size,
            ..
        } => {
            let size = size.sanitized();
            let session = {
                let slot = terminal.lock();
                slot.session_id
            };
            if let Some(session) = session {
                let grid = fixture::titled_grid(size.cols, size.rows, "fixture shell");
                terminal.publish_grid(generation, session, terminal_id, grid, size);
                wake();
            }
            metrics.commands_completed.fetch_add(1, Ordering::Relaxed);
            sink(vec![UiEvent::CommandResult {
                generation,
                label: "ResizeTerminal",
                ok: true,
                message: format!("{}x{}", size.cols, size.rows),
                took_us: started.elapsed().as_micros() as u64,
            }]);
        }
        Command::Input {
            terminal: terminal_id,
            bytes,
            ..
        } => {
            let delta = {
                let slot = terminal.lock();
                let Some(grid) = slot.grid.as_ref() else {
                    return;
                };
                if slot.terminal_id != Some(terminal_id) {
                    return;
                }
                for byte in bytes {
                    match byte {
                        b'\r' | b'\n' => link.echo.clear(),
                        0x20..=0x7e if link.echo.len() < 120 => {
                            link.echo.push(byte as char);
                        }
                        _ => {}
                    }
                }
                fixture::echo_delta(grid, &link.echo, grid.last_seq + 1)
            };
            let _ = terminal.apply_delta(generation, terminal_id, &delta);
            metrics.terminal_deltas.fetch_add(1, Ordering::Relaxed);
            wake();
        }
        Command::FetchScrollback {
            terminal: terminal_id,
            from_line,
            count,
            ..
        } => {
            let block = {
                let slot = terminal.lock();
                let Some(grid) = slot.grid.as_ref() else {
                    return;
                };
                fixture::scrollback_rows(
                    grid.visible.first().map_or(0, |row| row.cells.len()),
                    from_line,
                    count,
                    grid.scrollback_generation,
                )
            };
            let _ = terminal.merge_scrollback(generation, terminal_id, &block);
            wake();
        }
        Command::CreateShellSession { workspace_id } => {
            let (label, result) = (
                "CreateShellSession",
                fixture_create(&mut link.store, workspace_id),
            );
            fixture_result(generation, label, result, sink, metrics, started);
        }
        Command::RenameSession { session_id, title } => {
            let (label, result) = (
                "RenameSession",
                fixture_rename(&mut link.store, session_id, &title),
            );
            fixture_result(generation, label, result, sink, metrics, started);
        }
        Command::OpenEditor { .. }
        | Command::EditorView { .. }
        | Command::EditorInput { .. }
        | Command::Read { .. }
        | Command::SetAppState { .. }
        | Command::AddProject(_)
        | Command::Usage { .. }
        | Command::Paths { .. }
        | Command::KillSession(_)
        | Command::RestartSession(_)
        | Command::DeleteTranscript { .. }
        | Command::CloseSession(_)
        | Command::RebaseStep { .. }
        | Command::RunAction { .. }
        | Command::Fetch { .. }
        | Command::OpenPullRequest { .. }
        | Command::RefreshPullRequests
        | Command::DraftCommit { .. }
        | Command::Commit { .. }
        | Command::CreateAgent { .. }
        | Command::CreateWorktree { .. }
        | Command::RemoveWorktree { .. }
        | Command::StopDaemon { .. }
        | Command::FactoryReset
        | Command::RefreshAgentDetection
        | Command::SetProviderExecutable { .. }
        | Command::SaveAgentProfile(_)
        | Command::RemoveAgentProfile(_)
        | Command::RefreshProject(_)
        | Command::RefreshWorkspaceStatus(_)
        | Command::RenameWorkspace { .. }
        | Command::SetProjectIcon { .. }
        | Command::RemoveProject { .. }
        | Command::SetWorktreeIgnores { .. }
        | Command::CreatePath { .. }
        | Command::RenamePath { .. }
        | Command::CopyPath { .. }
        | Command::DeletePath { .. } => {}
        Command::RefreshSnapshot | Command::ForceReconnect | Command::Shutdown => {}
        Command::EditorFind { .. }
        | Command::SetEditorAutosave { .. }
        | Command::EditorConflict { .. }
        | Command::ReloadEditorBuffer(_)
        | Command::OverwriteEditorBuffer(_) => {}
    }
}

fn fixture_result(
    generation: u64,
    label: &'static str,
    result: Result<(String, Option<DaemonEvent>), String>,
    sink: &EventSink,
    metrics: &Metrics,
    started: Instant,
) {
    metrics.commands_completed.fetch_add(1, Ordering::Relaxed);
    match result {
        Ok((message, event)) => {
            sink(vec![UiEvent::CommandResult {
                generation,
                label,
                ok: true,
                message,
                took_us: started.elapsed().as_micros() as u64,
            }]);
            if let Some(event) = event {
                sink(vec![UiEvent::Daemon {
                    generation,
                    event: Box::new(event),
                }]);
            }
        }
        Err(message) => sink(vec![UiEvent::CommandResult {
            generation,
            label,
            ok: false,
            message,
            took_us: started.elapsed().as_micros() as u64,
        }]),
    }
}

fn fixture_create(
    store: &mut Store,
    workspace_id: WorkspaceId,
) -> Result<(String, Option<DaemonEvent>), String> {
    let workspace = store
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .ok_or_else(|| format!("fixture: no workspace {workspace_id}"))?;
    let now = Timestamp::now();
    let id = SessionId::new();
    let session = Session {
        id,
        workspace_id,
        kind: SessionKind::Shell,
        role: SessionRole::Generic,
        parent_session_id: None,
        root_session_id: id,
        terminal_id: Some(TerminalId::new()),
        editor: None,
        agent_provider_id: None,
        agent_profile_id: None,
        title: SessionTitle {
            user: Some("fixture shell".to_string()),
            terminal: None,
        },
        state: SessionState::Running,
        created_at: now,
        launch_command: None,
        last_activity_at: now,
        ended_at: None,
        base_commit: None,
        activity: AgentActivity::unknown(),
        action: None,
    };
    store.sessions.push(session.clone());
    Ok((
        format!("created {} in {}", short_id(&id), workspace.label()),
        Some(DaemonEvent::SessionCreated(session)),
    ))
}

fn fixture_rename(
    store: &mut Store,
    session_id: SessionId,
    title: &str,
) -> Result<(String, Option<DaemonEvent>), String> {
    let session = store
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| format!("fixture: no session {session_id}"))?;
    session.title.user = Some(title.to_string());
    Ok((
        format!("renamed {} to “{title}”", short_id(&session_id)),
        Some(DaemonEvent::SessionUpdated(session.clone())),
    ))
}

struct LiveCtx<'a> {
    generation: u64,
    socket: &'a std::path::Path,
    daemon: Option<&'a std::path::Path>,
    spawn: bool,
    sink: &'a EventSink,
    wake: &'a WakeSink,
    rx: &'a Receiver<Command>,
    stop: &'a AtomicBool,
    metrics: &'a Metrics,
    terminal: &'a SharedTerminal,
}

struct LiveLink {
    client: Client,
    events: Receiver<DaemonEvent>,
}

fn run_live(ctx: LiveCtx<'_>) -> Result<(), String> {
    let LiveCtx {
        generation,
        socket,
        daemon,
        spawn,
        sink,
        wake,
        rx,
        stop,
        metrics,
        terminal,
    } = ctx;
    sink(vec![UiEvent::Connecting {
        generation,
        detail: format!("socket {}", socket.display()),
    }]);
    let link = connect_live(socket, daemon, spawn, metrics)?;
    let info = link.client.daemon_info();
    sink(vec![UiEvent::Connected {
        generation,
        detail: format!(
            "forge-daemon v{}, protocol {}, {}",
            info.daemon_version,
            info.protocol_version,
            short_id_str(&info.instance_id)
        ),
        editor_surface: info.editor_surface,
    }]);

    let (store, took_us) = bootstrap(&link.client, metrics)?;
    emit_snapshot(generation, &store, took_us, sink, metrics);

    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let events = link.events.clone();
        let selected = Selector::new()
            .recv(&events, |message| Msg::Event(message.ok().map(Box::new)))
            .recv(rx, |message| Msg::Command(message.ok()))
            .wait_timeout(Duration::from_millis(250));
        match selected {
            Ok(Msg::Event(Some(event))) => match event.as_ref() {
                DaemonEvent::TerminalDelta { terminal_id, delta } => {
                    metrics.terminal_deltas.fetch_add(1, Ordering::Relaxed);
                    if terminal.apply_delta(generation, *terminal_id, delta)
                        == client::DeltaOutcome::NeedsResync
                    {
                        let _ =
                            resync_live(generation, &link.client, terminal, *terminal_id, metrics);
                        wake();
                    } else {
                        wake();
                    }
                }
                DaemonEvent::TerminalResync {
                    terminal_id,
                    snapshot,
                } => {
                    terminal.resync(generation, *terminal_id, snapshot);
                    wake();
                }
                _ => sink(vec![UiEvent::Daemon { generation, event }]),
            },
            Ok(Msg::Event(None)) => {
                return Err("daemon closed the connection".to_string());
            }
            Ok(Msg::Command(Some(Command::Shutdown))) => {
                stop.store(true, Ordering::Release);
                return Ok(());
            }
            Ok(Msg::Command(Some(Command::ForceReconnect))) => {
                return Err("reconnect requested".to_string());
            }
            Ok(Msg::Command(Some(command))) => handle_live_command(
                generation,
                &link.client,
                command,
                sink,
                wake,
                metrics,
                terminal,
            ),
            Ok(Msg::Command(None)) => {
                stop.store(true, Ordering::Release);
                return Ok(());
            }
            Err(_) => {
                if !link.client.is_connected() {
                    return Err("daemon disconnected".to_string());
                }
            }
        }
    }
}

enum Msg {
    Event(Option<Box<DaemonEvent>>),
    Command(Option<Command>),
}

fn connect_live(
    socket: &std::path::Path,
    daemon: Option<&std::path::Path>,
    spawn: bool,
    metrics: &Metrics,
) -> Result<LiveLink, String> {
    let mut spawned = false;
    let mut last = String::new();
    for attempt in 1..=CONNECT_ATTEMPTS {
        metrics.note_socket_op();
        match Client::connect(socket, CLIENT_VERSION) {
            Ok(client) => {
                let events = client.events();
                return Ok(LiveLink { client, events });
            }
            Err(error) => {
                last = error.to_string();
                if attempt == CONNECT_ATTEMPTS {
                    break;
                }
                if !spawned
                    && spawn
                    && daemon_absent(&error)
                    && let Some(bin) = daemon
                {
                    spawn_daemon(bin, metrics)?;
                    spawned = true;
                }
                thread::sleep(RETRY);
            }
        }
    }
    Err(format!("could not connect to {}: {last}", socket.display()))
}

fn daemon_absent(error: &client::ClientError) -> bool {
    matches!(
        error,
        client::ClientError::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
    )
}

fn spawn_daemon(bin: &std::path::Path, metrics: &Metrics) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command as Process, Stdio};
    metrics.note_subprocess_op();
    Process::new(bin)
        .arg("run")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to start {}: {error}", bin.display()))
}

fn handle_live_command(
    generation: u64,
    client: &Client,
    command: Command,
    sink: &EventSink,
    wake: &WakeSink,
    metrics: &Metrics,
    terminal: &SharedTerminal,
) {
    let started = Instant::now();
    match command {
        Command::Activate {
            generation: command_generation,
            session,
            terminal: terminal_id,
            size,
        } => {
            if command_generation < generation {
                return;
            }
            metrics.note_socket_op();
            let old = {
                let slot = terminal.lock();
                slot.terminal_id
            };
            if old != terminal_id
                && let Some(old) = old
            {
                metrics.note_socket_op();
                let _ = client.detach_terminal(old);
            }
            match (session, terminal_id) {
                (Some(session), Some(terminal_id)) => {
                    let pty = pty_size(size);
                    metrics.note_socket_op();
                    match client.attach_terminal(terminal_id, pty) {
                        Ok(snapshot) => {
                            terminal.publish_snapshot(generation, session, terminal_id, &snapshot);
                            wake();
                        }
                        Err(error) => sink(vec![UiEvent::CommandResult {
                            generation,
                            label: "AttachTerminal",
                            ok: false,
                            message: error.to_string(),
                            took_us: started.elapsed().as_micros() as u64,
                        }]),
                    }
                }
                (session, None) => {
                    terminal.clear(generation, session);
                    wake();
                }
                (None, Some(_)) => {}
            }
        }
        Command::ResizeAndAttach {
            generation: command_generation,
            terminal: terminal_id,
            size,
        } => {
            if command_generation < generation {
                return;
            }
            metrics.note_socket_op();
            let result = client
                .resize_terminal(terminal_id, size)
                .and_then(|()| client.attach_terminal(terminal_id, size));
            match result {
                Ok(snapshot) => {
                    let session = {
                        let slot = terminal.lock();
                        slot.session_id
                    };
                    if let Some(session) = session {
                        terminal.publish_snapshot(generation, session, terminal_id, &snapshot);
                    }
                    wake();
                    metrics.commands_completed.fetch_add(1, Ordering::Relaxed);
                    sink(vec![UiEvent::CommandResult {
                        generation,
                        label: "ResizeTerminal",
                        ok: true,
                        message: format!("{}x{}", size.cols, size.rows),
                        took_us: started.elapsed().as_micros() as u64,
                    }]);
                }
                Err(error) => {
                    metrics.commands_completed.fetch_add(1, Ordering::Relaxed);
                    sink(vec![UiEvent::CommandResult {
                        generation,
                        label: "ResizeTerminal",
                        ok: false,
                        message: error.to_string(),
                        took_us: started.elapsed().as_micros() as u64,
                    }]);
                }
            }
        }
        Command::Input {
            generation: command_generation,
            terminal: terminal_id,
            bytes,
        } => {
            if command_generation < generation {
                return;
            }
            metrics.note_socket_op();
            if let Err(error) = client.write_terminal_input(terminal_id, bytes) {
                sink(vec![UiEvent::CommandResult {
                    generation,
                    label: "WriteTerminalInput",
                    ok: false,
                    message: error.to_string(),
                    took_us: started.elapsed().as_micros() as u64,
                }]);
            }
        }
        Command::FetchScrollback {
            generation: command_generation,
            terminal: terminal_id,
            from_line,
            count,
        } => {
            if command_generation < generation {
                return;
            }
            metrics.note_socket_op();
            match client.fetch_scrollback(terminal_id, from_line, count) {
                Ok(block) => {
                    terminal.merge_scrollback(generation, terminal_id, &block);
                    wake();
                }
                Err(error) => eprintln!("forge-ui: FetchScrollback: {error}"),
            }
        }
        Command::CreateShellSession { workspace_id } => {
            metrics.note_socket_op();
            let result = client
                .create_shell_session(workspace_id)
                .map(|(session_id, terminal_id)| {
                    format!(
                        "created {} on {}",
                        short_id(&session_id),
                        short_id(&terminal_id)
                    )
                })
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "CreateShellSession",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::RenameSession { session_id, title } => {
            metrics.note_socket_op();
            let result = client
                .rename_session(session_id, Some(title.clone()))
                .map(|()| format!("renamed {} to “{title}”", short_id(&session_id)))
                .map_err(|error| error.to_string());
            live_result(generation, "RenameSession", result, sink, metrics, started);
        }
        Command::OpenEditor { workspace_id, path } => {
            metrics.note_socket_op();
            let result = client
                .create_editor_session(workspace_id, &path, None, false, false)
                .map(|(session_id, _)| {
                    sink(vec![UiEvent::SessionOpened {
                        generation,
                        session_id,
                    }]);
                    format!("opened {path} in {}", short_id(&session_id))
                })
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "CreateEditorSession",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::EditorView {
            generation: command_generation,
            session_id,
            first_line,
            line_count,
        } => {
            if command_generation != generation {
                return;
            }
            metrics.note_socket_op();
            if let Err(error) = client.set_editor_view(session_id, first_line, line_count) {
                eprintln!("forge-ui: SetEditorView: {error}");
            }
        }
        Command::EditorInput {
            generation: command_generation,
            session_id,
            events,
        } => {
            // Input from an older connection is dropped, never replayed: the
            // editor it was typed against may have moved on.
            if command_generation != generation {
                return;
            }
            metrics.note_socket_op();
            if let Err(error) = client.send_editor_input(session_id, events) {
                eprintln!("forge-ui: SendEditorInput: {error}");
            }
        }
        Command::Read {
            generation: command_generation,
            scope,
            read,
        } => {
            let workspace_id = scope.workspace;
            if command_generation != generation {
                return;
            }
            metrics.note_socket_op();
            let event = match read {
                crate::workbench::Read::Diff => UiEvent::Diff {
                    generation,
                    scope,
                    result: client
                        .workspace_diff(workspace_id, None)
                        .map(Box::new)
                        .map_err(|error| error.to_string()),
                },
                crate::workbench::Read::Directory(path) => UiEvent::Directory {
                    generation,
                    scope,
                    result: client
                        .list_directory(workspace_id, path.clone())
                        .map_err(|error| error.to_string()),
                    path,
                },
                crate::workbench::Read::Search(query) => UiEvent::Search {
                    generation,
                    scope,
                    result: client
                        .search_files(
                            workspace_id,
                            query.clone(),
                            domain::SearchKind::Name,
                            Some(crate::palette::FILE_LIMIT),
                        )
                        .map(|results| results.matches.into_iter().map(|hit| hit.path).collect())
                        .map_err(|error| error.to_string()),
                    query,
                },
                crate::workbench::Read::Rebase => UiEvent::Rebase {
                    generation,
                    scope,
                    result: client
                        .rebase_state(workspace_id)
                        .map(Box::new)
                        .map_err(|error| error.to_string()),
                },
                crate::workbench::Read::File(path) => UiEvent::File {
                    generation,
                    scope,
                    result: client
                        .read_file(workspace_id, path)
                        .map(Box::new)
                        .map_err(|error| error.to_string()),
                },
            };
            sink(vec![event]);
        }
        Command::CreateAgent {
            workspace_id,
            provider_id,
            profile_id,
            initial_prompt,
            read_only,
            resume,
        } => {
            metrics.note_socket_op();
            // A provider with no review mode is refused by the daemon, never
            // downgraded to a launch that could write.
            let result = client
                .create_agent_session_with_role(
                    workspace_id,
                    provider_id.clone(),
                    profile_id,
                    None,
                    domain::SessionRole::Generic,
                    resume,
                    initial_prompt,
                    read_only,
                )
                .map(|(session_id, _)| {
                    sink(vec![UiEvent::SessionOpened {
                        generation,
                        session_id,
                    }]);
                    format!("started {provider_id} in {}", short_id(&session_id))
                })
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "CreateAgentSession",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::DraftCommit { workspace_id } => {
            metrics.note_socket_op();
            // The draft arrives later as `JuvaDraftReady`; this only starts it.
            let result = client
                .draft_with_juva(workspace_id, domain::JuvaKind::CommitMessage)
                .map(|()| "drafting".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "DraftWithJuva", result, sink, metrics, started);
        }
        Command::Commit {
            workspace_id,
            message,
        } => {
            metrics.note_socket_op();
            let result = client
                .create_commit(workspace_id, &message)
                .map(|()| "committed".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "CreateCommit", result, sink, metrics, started);
        }
        // The three below ack when the work starts; the outcome is an event.
        Command::Fetch { project_id } => {
            metrics.note_socket_op();
            let result = client
                .fetch_remote(project_id, None)
                .map(|()| "fetching".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "FetchRemote", result, sink, metrics, started);
        }
        Command::OpenPullRequest {
            workspace_id,
            title,
            body,
            commit,
        } => {
            metrics.note_socket_op();
            let result = match commit {
                Some(message) => client.commit_and_open_pull_request(
                    workspace_id,
                    domain::CommitPlan {
                        branch: None,
                        message,
                    },
                    &title,
                    &body,
                ),
                None => client.create_pull_request(workspace_id, &title, &body, None),
            }
            .map(|()| "pushing".to_string())
            .map_err(|error| error.to_string());
            live_result(
                generation,
                "CreatePullRequest",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::RefreshPullRequests => {
            metrics.note_socket_op();
            let result = client
                .refresh_pull_requests()
                .map(|()| "refreshing".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "RefreshPullRequests",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::RunAction {
            workspace_id,
            action_id,
        } => {
            metrics.note_socket_op();
            let result = client
                .run_action(workspace_id, action_id)
                .map(|launch| {
                    // Started, queued behind a dependency, or already live: each
                    // names the session to show.
                    let (session_id, said) = match launch {
                        client::ActionLaunch::Started { session_id, .. } => (session_id, "started"),
                        client::ActionLaunch::Queued { session_id } => (session_id, "queued"),
                        client::ActionLaunch::AlreadyRunning { session_id } => {
                            (session_id, "already running")
                        }
                    };
                    sink(vec![UiEvent::SessionOpened {
                        generation,
                        session_id,
                    }]);
                    said.to_string()
                })
                .map_err(|error| error.to_string());
            live_result(generation, "RunAction", result, sink, metrics, started);
        }
        Command::RebaseStep { scope, step } => {
            use crate::workbench::RebaseStep;
            let workspace_id = scope.workspace;
            metrics.note_socket_op();
            let label = match step {
                RebaseStep::Resolve(_) => "MarkConflictResolved",
                RebaseStep::Continue => "ContinueRebase",
                RebaseStep::Abort => "AbortRebase",
            };
            // Each step answers with the state git is in now (abort reads it
            // again), so the panel never keeps a count of its own.
            let state = match step {
                RebaseStep::Resolve(paths) => client.mark_conflict_resolved(workspace_id, paths),
                RebaseStep::Continue => client.continue_rebase(workspace_id),
                RebaseStep::Abort => client
                    .abort_rebase(workspace_id)
                    .and_then(|()| client.rebase_state(workspace_id)),
            }
            .map_err(|error| error.to_string());
            let result = state
                .as_ref()
                .map(|_| "done".to_string())
                .map_err(Clone::clone);
            sink(vec![UiEvent::Rebase {
                generation,
                scope,
                result: state.map(Box::new),
            }]);
            live_result(generation, label, result, sink, metrics, started);
        }
        Command::DeleteTranscript {
            session_id,
            provider,
            profile_id,
        } => {
            metrics.note_socket_op();
            let result = client
                .delete_external_session(session_id, provider, profile_id)
                .map(|()| "deleted transcript".to_string())
                .map_err(|error| error.to_string());
            if result.is_ok()
                && let Ok((store, took_us)) = load_snapshot(client, metrics)
            {
                emit_snapshot(generation, &store, took_us, sink, metrics);
            }
            live_result(
                generation,
                "DeleteExternalSession",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::RestartSession(session_id) => {
            metrics.note_socket_op();
            let result = client
                .restart_session(session_id)
                .map(|()| "restarting".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "RestartSession", result, sink, metrics, started);
        }
        Command::KillSession(session_id) => {
            metrics.note_socket_op();
            let result = client
                .kill_session(session_id)
                .map(|()| "stopping".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "KillSession", result, sink, metrics, started);
        }
        Command::CloseSession(session_id) => {
            metrics.note_socket_op();
            let result = client
                .close_session(session_id)
                .map(|()| "closed".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "CloseSession", result, sink, metrics, started);
        }
        Command::Usage {
            generation: command_generation,
            window_days,
        } => {
            if command_generation != generation {
                return;
            }
            metrics.note_socket_op();
            let result = client
                .usage_analytics(Some(window_days))
                .map(Box::new)
                .map_err(|error| error.to_string());
            sink(vec![UiEvent::Usage {
                generation,
                window_days,
                result,
            }]);
        }
        Command::Paths {
            generation: command_generation,
        } => {
            if command_generation != generation {
                return;
            }
            metrics.note_socket_op();
            let result = client
                .paths()
                .map(Box::new)
                .map_err(|error| error.to_string());
            sink(vec![UiEvent::Paths { generation, result }]);
        }
        Command::RefreshProject(project_id) => {
            metrics.note_socket_op();
            let result = client
                .refresh_project(project_id)
                .map(|()| "refreshing project".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "RefreshProject", result, sink, metrics, started);
        }
        Command::RefreshWorkspaceStatus(workspace_id) => {
            metrics.note_socket_op();
            let result = client
                .refresh_workspace_status(workspace_id)
                .map(|()| "refreshing status".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "RefreshWorkspaceStatus",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::RenameWorkspace {
            workspace_id,
            display_name,
        } => {
            metrics.note_socket_op();
            let result = client
                .rename_workspace(workspace_id, display_name)
                .map(|()| "renamed checkout".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "RenameWorkspace",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::SetProjectIcon { project_id, icon } => {
            metrics.note_socket_op();
            let result = client
                .set_project_icon(project_id, icon)
                .map(|()| "set icon".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "SetProjectIcon", result, sink, metrics, started);
        }
        Command::RemoveProject { project_id, policy } => {
            metrics.note_socket_op();
            let result = client
                .remove_project(project_id, policy)
                .map(|()| "removed project".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "RemoveProject", result, sink, metrics, started);
        }
        Command::SetWorktreeIgnores { project_id, rules } => {
            metrics.note_socket_op();
            let result = client
                .set_worktree_ignores(project_id, rules)
                .map(|()| "ignoring worktrees".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "SetWorktreeIgnores",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::CreatePath {
            workspace_id,
            path,
            directory,
        } => {
            metrics.note_socket_op();
            let result = client
                .create_path(workspace_id, path.clone(), directory)
                .map(|()| format!("created {path}"))
                .map_err(|error| error.to_string());
            live_result(generation, "CreatePath", result, sink, metrics, started);
        }
        Command::RenamePath {
            workspace_id,
            from,
            to,
        } => {
            metrics.note_socket_op();
            let result = client
                .rename_path(workspace_id, from, to.clone())
                .map(|()| format!("moved to {to}"))
                .map_err(|error| error.to_string());
            live_result(generation, "RenamePath", result, sink, metrics, started);
        }
        Command::CopyPath {
            workspace_id,
            from,
            to,
        } => {
            metrics.note_socket_op();
            let result = client
                .copy_path(workspace_id, from, to.clone())
                .map(|()| format!("copied to {to}"))
                .map_err(|error| error.to_string());
            live_result(generation, "CopyPath", result, sink, metrics, started);
        }
        Command::DeletePath { workspace_id, path } => {
            metrics.note_socket_op();
            let result = client
                .delete_path(workspace_id, path.clone())
                .map(|()| format!("deleted {path}"))
                .map_err(|error| error.to_string());
            live_result(generation, "DeletePath", result, sink, metrics, started);
        }
        Command::AddProject(path) => {
            metrics.note_socket_op();
            let result = client
                .add_project(&path)
                .map(|()| format!("added {}", path.display()))
                .map_err(|error| error.to_string());
            // The new project's recorded runs reach the client only on a
            // snapshot, so History would miss them until a reconnect.
            if result.is_ok()
                && let Ok((store, took_us)) = load_snapshot(client, metrics)
            {
                emit_snapshot(generation, &store, took_us, sink, metrics);
            }
            live_result(generation, "AddProject", result, sink, metrics, started);
        }
        Command::CreateWorktree { project_id, branch } => {
            metrics.note_socket_op();
            let result = client
                .create_worktree(project_id, &branch, None, None)
                .map(|_| format!("creating worktree {branch}"))
                .map_err(|error| error.to_string());
            live_result(generation, "CreateWorktree", result, sink, metrics, started);
        }
        Command::RemoveWorktree { workspace_id } => {
            metrics.note_socket_op();
            // Never forced from here: a worktree with uncommitted work is the
            // daemon's to refuse, not this button's to discard.
            let result = client
                .remove_worktree(workspace_id, false)
                .map(|_| "removed worktree".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "RemoveWorktree", result, sink, metrics, started);
        }
        Command::StopDaemon { kill_sessions } => {
            metrics.note_socket_op();
            let sessions = if kill_sessions {
                client::StopSessions::Kill
            } else {
                client::StopSessions::Keep
            };
            let result = client
                .stop_daemon(sessions)
                .map(|()| "stopping the daemon".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "StopDaemon", result, sink, metrics, started);
        }
        Command::FactoryReset => {
            metrics.note_socket_op();
            let result = client
                .factory_reset()
                .map(|()| "reset".to_string())
                .map_err(|error| error.to_string());
            live_result(generation, "FactoryReset", result, sink, metrics, started);
        }
        Command::RefreshAgentDetection => {
            metrics.note_socket_op();
            let result = client
                .refresh_detection(None)
                .map(|()| "rescanning".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "RefreshAgentDetection",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::SetProviderExecutable { provider_id, path } => {
            metrics.note_socket_op();
            let result = client
                .set_provider_executable(&provider_id, path)
                .map(|()| format!("{provider_id} command saved"))
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "SetProviderExecutable",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::SaveAgentProfile(profile) => {
            metrics.note_socket_op();
            let result = client
                .save_agent_profile(*profile)
                .map(|()| "profile saved".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "SaveAgentProfile",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::RemoveAgentProfile(profile_id) => {
            metrics.note_socket_op();
            let result = client
                .remove_agent_profile(profile_id)
                .map(|()| "profile removed".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "RemoveAgentProfile",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::SetAppState { key, value } => {
            metrics.note_socket_op();
            if let Err(error) = client.set_app_state(&key, &value) {
                eprintln!("forge-ui: saving {key}: {error}");
            }
        }
        Command::RefreshSnapshot => {
            if let Ok((store, took_us)) = load_snapshot(client, metrics) {
                emit_snapshot(generation, &store, took_us, sink, metrics);
            }
        }
        Command::ForceReconnect | Command::Shutdown => {}
        Command::EditorFind {
            session_id,
            command,
        } => {
            metrics.note_socket_op();
            if let Err(error) = client.editor_find(session_id, command) {
                eprintln!("forge-ui: EditorFind: {error}");
            }
        }
        Command::SetEditorAutosave {
            session_id,
            autosave,
        } => {
            metrics.note_socket_op();
            let result = client
                .set_editor_autosave(session_id, autosave)
                .map(|()| format!("autosave {}", if autosave { "on" } else { "off" }))
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "SetEditorAutosave",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::EditorConflict {
            generation: command_generation,
            session_id,
        } => {
            if command_generation != generation {
                return;
            }
            metrics.note_socket_op();
            let result = client
                .editor_conflict(session_id)
                .map(|(_, disk, mine)| Box::new((disk, mine)))
                .map_err(|error| error.to_string());
            sink(vec![UiEvent::EditorConflict {
                generation,
                session_id,
                result,
            }]);
        }
        Command::ReloadEditorBuffer(session_id) => {
            metrics.note_socket_op();
            let result = client
                .reload_editor_buffer(session_id)
                .map(|()| "took the disk's copy".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "ReloadEditorBuffer",
                result,
                sink,
                metrics,
                started,
            );
        }
        Command::OverwriteEditorBuffer(session_id) => {
            metrics.note_socket_op();
            let result = client
                .overwrite_editor_buffer(session_id)
                .map(|()| "kept this copy".to_string())
                .map_err(|error| error.to_string());
            live_result(
                generation,
                "OverwriteEditorBuffer",
                result,
                sink,
                metrics,
                started,
            );
        }
    }
}

fn live_result(
    generation: u64,
    label: &'static str,
    result: Result<String, String>,
    sink: &EventSink,
    metrics: &Metrics,
    started: Instant,
) {
    metric_command(metrics, started);
    metrics.commands_completed.fetch_add(1, Ordering::Relaxed);
    let (ok, message) = match result {
        Ok(message) => (true, message),
        Err(error) => (false, error),
    };
    sink(vec![UiEvent::CommandResult {
        generation,
        label,
        ok,
        message,
        took_us: started.elapsed().as_micros() as u64,
    }]);
}

fn resync_live(
    generation: u64,
    client: &Client,
    terminal: &SharedTerminal,
    terminal_id: TerminalId,
    metrics: &Metrics,
) -> Result<(), String> {
    let (session, size) = {
        let slot = terminal.lock();
        (slot.session_id, slot.size)
    };
    let Some(_session) = session else {
        return Ok(());
    };
    metrics.note_socket_op();
    let snapshot = client
        .attach_terminal(terminal_id, size.unwrap_or_default())
        .map_err(|error| error.to_string())?;
    terminal.resync(generation, terminal_id, &snapshot);
    Ok(())
}

/// The first read after connecting. It adds no project and starts no
/// shell: an empty daemon shows the empty centre (New terminal, Add a
/// project) as Tauri's does, and a launch from Finder has `/` for its
/// working directory, which is no one's project.
fn bootstrap(client: &Client, metrics: &Metrics) -> Result<(Store, u64), String> {
    load_snapshot(client, metrics)
}

fn load_snapshot(client: &Client, metrics: &Metrics) -> Result<(Store, u64), String> {
    let started = Instant::now();
    metrics.note_socket_op();
    let response = client
        .request(Request::GetSnapshot)
        .map_err(|error| error.to_string())?;
    let mut store = Store::new();
    store.apply_snapshot(response);
    Ok((store, started.elapsed().as_micros() as u64))
}

fn emit_snapshot(
    generation: u64,
    store: &Store,
    took_us: u64,
    sink: &EventSink,
    metrics: &Metrics,
) {
    if let Ok(mut value) = metrics.last_snapshot_us.lock() {
        *value = Some(took_us);
    }
    sink(vec![UiEvent::Snapshot {
        generation,
        store: Box::new(store.clone()),
        took_us,
    }]);
}

fn pty_size(size: Option<(u16, u16)>) -> domain::PtySize {
    let (cols, rows) = size.unwrap_or((80, 24));
    domain::PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
    .sanitized()
}

fn metric_command(metrics: &Metrics, started: Instant) {
    if let Ok(mut value) = metrics.last_command_us.lock() {
        *value = Some(started.elapsed().as_micros() as u64);
    }
}

fn command_label(command: &Command) -> &'static str {
    match command {
        Command::Activate { .. } => "AttachTerminal",
        Command::ResizeAndAttach { .. } => "ResizeTerminal",
        Command::Input { .. } => "WriteTerminalInput",
        Command::FetchScrollback { .. } => "FetchScrollback",
        Command::CreateShellSession { .. } => "CreateShellSession",
        Command::RenameSession { .. } => "RenameSession",
        Command::OpenEditor { .. } => "CreateEditorSession",
        Command::EditorView { .. } => "SetEditorView",
        Command::EditorInput { .. } => "SendEditorInput",
        Command::Read { .. } => "WorkspaceRead",
        Command::SetAppState { .. } => "SetAppState",
        Command::AddProject(_) => "AddProject",
        Command::Usage { .. } => "GetUsageAnalytics",
        Command::Paths { .. } => "GetPaths",
        Command::KillSession(_) => "KillSession",
        Command::RestartSession(_) => "RestartSession",
        Command::DeleteTranscript { .. } => "DeleteExternalSession",
        Command::CloseSession(_) => "CloseSession",
        Command::RebaseStep { .. } => "RebaseStep",
        Command::RunAction { .. } => "RunAction",
        Command::Fetch { .. } => "FetchRemote",
        Command::OpenPullRequest { .. } => "CreatePullRequest",
        Command::RefreshPullRequests => "RefreshPullRequests",
        Command::DraftCommit { .. } => "DraftWithJuva",
        Command::Commit { .. } => "CreateCommit",
        Command::CreateAgent { .. } => "CreateAgentSession",
        Command::CreateWorktree { .. } => "CreateWorktree",
        Command::RemoveWorktree { .. } => "RemoveWorktree",
        Command::StopDaemon { .. } => "StopDaemon",
        Command::FactoryReset => "FactoryReset",
        Command::RefreshAgentDetection => "RefreshAgentDetection",
        Command::SetProviderExecutable { .. } => "SetProviderExecutable",
        Command::SaveAgentProfile(_) => "SaveAgentProfile",
        Command::RemoveAgentProfile(_) => "RemoveAgentProfile",
        Command::RefreshProject(_) => "RefreshProject",
        Command::RefreshWorkspaceStatus(_) => "RefreshWorkspaceStatus",
        Command::RenameWorkspace { .. } => "RenameWorkspace",
        Command::SetProjectIcon { .. } => "SetProjectIcon",
        Command::RemoveProject { .. } => "RemoveProject",
        Command::SetWorktreeIgnores { .. } => "SetWorktreeIgnores",
        Command::CreatePath { .. } => "CreatePath",
        Command::RenamePath { .. } => "RenamePath",
        Command::CopyPath { .. } => "CopyPath",
        Command::DeletePath { .. } => "DeletePath",
        Command::RefreshSnapshot => "GetSnapshot",
        Command::ForceReconnect => "Reconnect",
        Command::Shutdown => "Shutdown",
        Command::EditorFind { .. } => "EditorFind",
        Command::SetEditorAutosave { .. } => "SetEditorAutosave",
        Command::EditorConflict { .. } => "GetEditorConflict",
        Command::ReloadEditorBuffer(_) => "ReloadEditorBuffer",
        Command::OverwriteEditorBuffer(_) => "OverwriteEditorBuffer",
    }
}

fn short_id<T: std::fmt::Display>(id: &T) -> String {
    short_id_str(&id.to_string())
}

fn short_id_str(id: &str) -> String {
    id.chars().take(8).collect()
}

pub fn fixture_store(path: &std::path::Path) -> Result<Store, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("read fixture {}: {error}", path.display()))?;
    let response: protocol::Response = serde_json::from_slice(&bytes)
        .map_err(|error| format!("decode fixture {}: {error}", path.display()))?;
    let mut store = Store::new();
    store.apply_snapshot(response);
    if store.workspaces.is_empty() {
        return Err(format!(
            "fixture {} has no workspaces; a New shell command needs one",
            path.display()
        ));
    }
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    fn fixture() -> Store {
        fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
            .expect("fixture snapshot decodes")
    }

    fn collect(
        rx: &flume::Receiver<Vec<UiEvent>>,
        deadline: Instant,
        mut done: impl FnMut(&UiEvent) -> bool,
    ) -> Vec<UiEvent> {
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(batch) = rx.recv_timeout(left) else {
                break;
            };
            for event in batch {
                let stop = done(&event);
                seen.push(event);
                if stop {
                    return seen;
                }
            }
        }
        seen
    }

    #[test]
    fn fixture_worker_round_trips_a_rename_and_reconnects_with_a_new_generation() {
        let store = fixture();
        let session_id = store.sessions[0].id;
        let metrics = Arc::new(Metrics::default());
        metrics.note_ui_thread();
        let boot = Arc::new(Boot {
            mode: Mode::Fixture {
                snapshot: Arc::new(store),
                reconnect_after: Some(Duration::from_millis(400)),
            },
            metrics: Arc::clone(&metrics),
            terminal: SharedTerminal::new(),
        });
        let (tx, rx) = flume::unbounded();
        let sink: EventSink = Arc::new(move |events| {
            let _ = tx.send(events);
        });
        let handle = spawn(boot, sink, Arc::new(|| {}));

        let events = collect(&rx, Instant::now() + Duration::from_secs(2), |event| {
            matches!(event, UiEvent::Snapshot { generation: 1, .. })
        });
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UiEvent::Snapshot { generation: 1, .. })),
            "first snapshot missing: {events:?}"
        );

        handle
            .rename_session(session_id, "renamed by the worker".to_string())
            .expect("command queued");
        let events = collect(&rx, Instant::now() + Duration::from_secs(2), |event| {
            matches!(event, UiEvent::CommandResult { ok: true, .. })
        });
        assert!(
            events.iter().any(|event| matches!(
                event,
                UiEvent::CommandResult {
                    label: "RenameSession",
                    ok: true,
                    ..
                }
            )),
            "rename result missing: {events:?}"
        );

        let events = collect(&rx, Instant::now() + Duration::from_secs(3), |event| {
            matches!(event, UiEvent::Snapshot { generation: 2, .. })
        });
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UiEvent::Disconnected { generation: 1, .. })),
            "disconnect missing: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, UiEvent::Snapshot { generation: 2, .. })),
            "second-generation snapshot missing: {events:?}"
        );
    }

    #[test]
    fn socket_work_only_happens_on_the_worker_thread() {
        let metrics = Arc::new(Metrics::default());
        metrics.note_ui_thread();
        let boot = Arc::new(Boot {
            mode: Mode::Live {
                socket: PathBuf::from("/tmp/forge-ui-no-such-daemon.sock"),
                daemon: None,
                spawn: false,
            },
            metrics: Arc::clone(&metrics),
            terminal: SharedTerminal::new(),
        });
        let sink: EventSink = Arc::new(|_| {});
        let handle = spawn(boot, sink, Arc::new(|| {}));
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            metrics.socket_ops.load(Ordering::Relaxed) > 0,
            "the worker should have attempted a socket connect"
        );
        assert_eq!(
            metrics.ui_socket_ops.load(Ordering::Relaxed),
            0,
            "the calling (UI) thread must never open the daemon socket"
        );
        assert_ne!(
            metrics.socket_thread(),
            format!("{:?}", thread::current().id()),
            "socket work ran on the test's main thread"
        );
        drop(handle);
    }

    #[test]
    fn fixture_input_echoes_through_a_delta() {
        let store = fixture();
        let metrics = Arc::new(Metrics::default());
        metrics.note_ui_thread();
        let terminal = SharedTerminal::new();
        let boot = Arc::new(Boot {
            mode: Mode::Fixture {
                snapshot: Arc::new(store.clone()),
                reconnect_after: None,
            },
            metrics: Arc::clone(&metrics),
            terminal: Arc::clone(&terminal),
        });
        let (tx, rx) = flume::unbounded();
        let sink: EventSink = Arc::new(move |events| {
            let _ = tx.send(events);
        });
        let handle = spawn(boot, sink, Arc::new(|| {}));
        let events = collect(&rx, Instant::now() + Duration::from_secs(2), |event| {
            matches!(event, UiEvent::Snapshot { .. })
        });
        assert!(!events.is_empty());
        let session = store.sessions[0].id;
        let terminal_id = store.sessions[0].terminal_id.unwrap();
        handle
            .activate(1, Some(session), Some(terminal_id), Some((80, 24)))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while terminal.meta().terminal_id != Some(terminal_id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        handle.input(1, terminal_id, b"echo hi\r".to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut echoed = false;
        while Instant::now() < deadline {
            let slot = terminal.lock();
            if let Some(grid) = slot.grid.as_ref() {
                let last = grid.visible.last().unwrap();
                let text: String = last.cells.iter().map(|cell| cell.text.as_str()).collect();
                if text.contains("fixture>") {
                    echoed = true;
                    break;
                }
            }
            drop(slot);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(echoed, "fixture did not echo the input bytes");
        drop(handle);
    }
}
