//! Dedicated runtime thread: blocking `client::Client`, event pump, 16 ms
//! `cells_only` coalesce.
//!
//! One thread drains one command channel, and that channel carries keystrokes,
//! so nothing on it may block on a socket: a synchronous network write queued
//! behind a key press would freeze typing (AGENTS.md).
//!
//! Two event streams leave this thread and they are deliberately separate.
//! `runtime:state` carries the shell — projects, sessions, providers — and goes
//! out only when one of them actually changed. `runtime:cells` carries terminal
//! output and goes out up to 62 times a second. Sending the shell snapshot on
//! every frame, as this bridge did while the canvas was a placeholder, put the
//! whole session tree through `serde_json` on the delta rung.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use client::{CellGrid, Client, DaemonEvent, EventOutcome, Store};
use domain::{
    AgentProfileId, AgentProviderId, JobId, MouseMode, ProjectId, PtySize, SessionId, TerminalId,
    Timestamp, WorkspaceId,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter as _};

use super::cells::{self, Damage};
use super::commands::RuntimeCommand;
use super::input;
use super::snapshot::{
    ConnectedPayload, DaemonInfoDto, DisconnectedPayload, HostStatus, Latest, ShellSnapshot,
    StatePayload,
};
use super::workbench::{self, WorkbenchCommand, WorkbenchSender};
use crate::daemon::locator::{connect_or_spawn, Locator};

const LIVENESS_TICK: Duration = Duration::from_secs(1);
const CELL_SEND_FLOOR: Duration = Duration::from_millis(16);

/// The floor while the viewport is scrolled back into history.
///
/// `cells::frame` sends a **full** frame whenever `scroll_offset > 0` — every
/// row, every time — because a damage list is expressed in live-viewport rows
/// and means nothing against a window of the scrollback. At the 16ms floor
/// that is up to 62 full frames a second for output nobody is watching: the
/// person is reading something further up, and the live rows arriving below
/// them are not on screen at all.
///
/// 33ms halves that. It is not a latency regression, because latency is
/// measured against the keystroke that produces output, and a scrolled
/// viewport is by definition not showing what a keystroke would produce
/// (`docs/performance.md`).
const CELL_SEND_FLOOR_SCROLLED: Duration = Duration::from_millis(33);

/// The floor in force for the viewport as it currently stands.
const fn cell_send_floor(scroll_offset: u64) -> Duration {
    if scroll_offset > 0 {
        CELL_SEND_FLOOR_SCROLLED
    } else {
        CELL_SEND_FLOOR
    }
}

/// Geometry used until the WebView has measured its own cell box and asked for
/// the size it can actually paint.
const DEFAULT_SIZE: PtySize = PtySize {
    cols: 100,
    rows: 32,
    pixel_width: 800,
    pixel_height: 576,
};

/// The harness preview is a *window onto* a session, not the place it is
/// driven from, so it is sized for reading the last dozen lines rather than
/// for working (§5.8). The feature tab overrides it once it has measured.
const PREVIEW_SIZE: PtySize = PtySize {
    cols: 100,
    rows: 12,
    pixel_width: 800,
    pixel_height: 216,
};

enum Selected {
    Event(Box<DaemonEvent>),
    Command(RuntimeCommand),
    Closed,
}

/// Text cut from the grid for the clipboard.
#[derive(Clone, Debug, Serialize)]
struct ClipboardPayload {
    text: String,
}

#[derive(Clone, Debug, Serialize)]
struct LieutenantJobPayload {
    project: ProjectId,
    job: Box<domain::Job>,
}

#[derive(Clone, Debug, Serialize)]
struct LieutenantFailedPayload {
    project: ProjectId,
    error: String,
}

#[derive(Clone, Debug, Serialize)]
struct JobLogPayload {
    job_id: JobId,
    lines: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct JobOutputPayload {
    job_id: JobId,
    from_line: u64,
    lines: Vec<String>,
}

pub struct Runtime {
    commands: flume::Sender<RuntimeCommand>,
    latest: Arc<Mutex<Latest>>,
    /// The worker bound to the *current* connection, or `None` while
    /// reconnecting. Replaced rather than reused, so a command can never be
    /// written to a dead socket.
    workbench: Arc<Mutex<Option<WorkbenchSender>>>,
}

impl Runtime {
    pub fn start(app: AppHandle) -> Self {
        let (commands_tx, commands_rx) = flume::bounded(256);
        let latest = Arc::new(Mutex::new(Latest::default()));
        let workbench = Arc::new(Mutex::new(None));
        let status = Arc::clone(&latest);
        let worker = Arc::clone(&workbench);
        thread::Builder::new()
            .name("forge-tauri-runtime".to_string())
            .spawn(move || runtime_loop(app, commands_rx, status, worker))
            .expect("failed to start Tauri runtime bridge");
        Self {
            commands: commands_tx,
            latest,
            workbench,
        }
    }

    pub fn send(&self, command: RuntimeCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                flume::TrySendError::Full(_) => "runtime command queue is full".to_string(),
                flume::TrySendError::Disconnected(_) => {
                    "runtime command queue is disconnected".to_string()
                }
            })
    }

    /// Queue a workbench read. Dropped while disconnected: the panel that
    /// asked learns from `runtime:disconnected`, not from a second channel.
    pub fn send_workbench(&self, command: WorkbenchCommand) {
        if let Some(sender) = lock(&self.workbench).as_ref() {
            let _ = sender.try_send(command);
        }
    }

    pub fn host_status(&self) -> HostStatus {
        lock(&self.latest).status.clone()
    }

    pub fn snapshot(&self) -> Option<ConnectedPayload> {
        let latest = lock(&self.latest);
        if latest.status.connected {
            latest.payload.clone()
        } else {
            None
        }
    }
}

/// What the shell is attached to, and where its viewport sits.
struct Attached {
    session: SessionId,
    terminal: TerminalId,
    /// Lines the viewport has been pulled up into history; `0` is live output.
    scroll_offset: u64,
}

/// The second terminal a feature tab watches a harness session through (§5.8).
///
/// One at a time. Attaching replaces whatever the preview held — which is what
/// makes clicking through a feature's sessions cheap — and it is deliberately
/// *not* the main attachment: leaving the feature tab must not move the
/// session the shell is on.
struct Preview {
    session: SessionId,
    terminal: TerminalId,
    size: PtySize,
}

/// Emits the two event streams and keeps the `connect` command's cached
/// snapshot in step with them.
struct Emitter<'a> {
    app: &'a AppHandle,
    latest: &'a Mutex<Latest>,
    daemon: DaemonInfoDto,
}

impl Emitter<'_> {
    /// Publish the shell: projects, workspaces, sessions, providers.
    ///
    /// The snapshot is built once and never copied: the event borrows it, and
    /// what is left is moved into the cache the `connect` command answers
    /// from. Emitting before remembering is what makes that possible, and the
    /// window it opens is harmless — a `connect` that lands between the two
    /// gets the previous snapshot and the event it is about to receive
    /// replaces it.
    fn shell(&self, store: &Store, at: &Attached) {
        let snapshot = ShellSnapshot::from_store(store);
        let _ = self.app.emit(
            "runtime:state",
            StatePayload {
                store: &snapshot,
                active_session: at.session,
                active_terminal: at.terminal,
            },
        );
        remember_connected(
            self.latest,
            ConnectedPayload {
                daemon: self.daemon.clone(),
                session_count: snapshot.sessions.len(),
                store: snapshot,
                active_session: Some(at.session),
                active_terminal: Some(at.terminal),
            },
        );
    }

    /// Publish one terminal frame.
    fn cells(&self, store: &mut Store, at: &Attached, damage: &Damage, echo_id: u64) {
        let bell = store
            .terminals
            .get_mut(&at.terminal)
            .is_some_and(CellGrid::take_bell);
        let Some(grid) = store.terminal(&at.terminal) else {
            return;
        };
        let _ = self.app.emit(
            "runtime:cells",
            cells::frame(at.terminal, grid, at.scroll_offset, damage, bell, echo_id),
        );
    }

    /// Publish one preview frame.
    ///
    /// Its own event, not `runtime:cells`: the main canvas must not repaint
    /// because a watched harness session printed a line, and the preview must
    /// not repaint because the user is typing.
    fn preview_cells(&self, store: &mut Store, preview: &Preview, damage: &Damage) {
        // The preview never rings: a bell belongs to the session the user is
        // working in, and taking it here would swallow it for the tab.
        let Some(grid) = store.terminal(&preview.terminal) else {
            return;
        };
        let _ = self.app.emit(
            "runtime:preview_cells",
            cells::frame(preview.terminal, grid, 0, damage, false, 0),
        );
    }
}

fn runtime_loop(
    app: AppHandle,
    commands: flume::Receiver<RuntimeCommand>,
    latest: Arc<Mutex<Latest>>,
    workbench: Arc<Mutex<Option<WorkbenchSender>>>,
) {
    let locator = Locator::from_env();
    let mut preferred_session = None;
    let mut size = DEFAULT_SIZE;

    loop {
        let _ = app.emit("runtime:connecting", ());
        {
            let mut guard = lock(&latest);
            guard.status = HostStatus {
                connected: false,
                ..HostStatus::default()
            };
        }

        // Shared with the workbench worker. `Client` was built for this:
        // `Shared.write` is a `Mutex<UnixStream>` documented as serialized
        // across request writers, and every waiter correlates by `request_id`.
        let client = match connect_or_spawn(&locator).map(Arc::new) {
            Ok(client) => client,
            Err(error) => {
                emit_disconnected(&app, &latest, error);
                thread::sleep(Duration::from_millis(300));
                continue;
            }
        };

        let (mut store, session, terminal) = match bootstrap(&client, preferred_session, size) {
            Ok(state) => state,
            Err(error) => {
                emit_disconnected(&app, &latest, error);
                thread::sleep(Duration::from_millis(300));
                continue;
            }
        };
        let mut at = Attached {
            session,
            terminal,
            scroll_offset: 0,
        };
        // Bound to this connection like the workbench worker: a reconnect
        // starts with no preview rather than one pointing at a dead terminal.
        let mut preview: Option<Preview> = None;
        // A worker per connection: dropping the previous sender ends the one
        // bound to the client that just died.
        *lock(&workbench) = Some(workbench::start(app.clone(), Arc::clone(&client)));
        preferred_session = Some(at.session);
        let emitter = Emitter {
            app: &app,
            latest: &latest,
            daemon: DaemonInfoDto::from(client.daemon_info()),
        };
        let snapshot = ShellSnapshot::from_store(&store);
        let payload = ConnectedPayload {
            session_count: snapshot.sessions.len(),
            store: snapshot,
            daemon: emitter.daemon.clone(),
            active_session: Some(at.session),
            active_terminal: Some(at.terminal),
        };
        let _ = app.emit("runtime:connected", &payload);
        remember_connected(&latest, payload);
        emitter.cells(&mut store, &at, &Damage::Full, 0);

        let events = client.events();
        let mut pending_command = None;
        // Output held back by the frame floor, with everything it has to
        // repaint once the floor lifts.
        let mut held: Option<(Instant, Damage)> = None;
        let mut last_cells_sent: Option<Instant> = None;
        // The newest keystroke whose bytes reached the daemon and whose echo
        // has not gone back out yet.
        let mut pending_echo: u64 = 0;
        // Close on a live session only kills (§7.3); the row is removed once
        // the process is terminal. Without this set the rail keeps a ghost.
        let mut pending_close: HashSet<SessionId> = HashSet::new();
        let mut reconnect = false;

        loop {
            loop {
                let next = match pending_command.take() {
                    Some(command) => Ok(command),
                    None => commands.try_recv(),
                };
                let command = match next {
                    Ok(command) => command,
                    Err(flume::TryRecvError::Empty) => break,
                    Err(flume::TryRecvError::Disconnected) => return,
                };
                match run_command(
                    command,
                    &client,
                    &mut store,
                    &mut at,
                    &mut preview,
                    &mut size,
                    &mut pending_echo,
                ) {
                    Ok(effect) => {
                        if let Some(id) = effect.forget_pending_close {
                            pending_close.remove(&id);
                        }
                        if let Some(id) = effect.pending_close {
                            pending_close.insert(id);
                        }
                        if effect.shell {
                            preferred_session = Some(at.session);
                            emitter.shell(&store, &at);
                        }
                        if let Some(damage) = effect.damage {
                            held = None;
                            last_cells_sent = Some(Instant::now());
                            emitter.cells(
                                &mut store,
                                &at,
                                &damage,
                                std::mem::take(&mut pending_echo),
                            );
                        }
                        if let Some(text) = effect.clipboard {
                            let _ = app.emit("runtime:clipboard", ClipboardPayload { text });
                        }
                        if let Some((project, job)) = effect.lieutenant_job {
                            let _ = app.emit(
                                "runtime:lieutenant_job",
                                LieutenantJobPayload { project, job },
                            );
                        }
                        if let Some((project, error)) = effect.lieutenant_failed {
                            let _ = app.emit(
                                "runtime:lieutenant_failed",
                                LieutenantFailedPayload { project, error },
                            );
                        }
                        if let Some((job_id, lines)) = effect.job_log {
                            let _ = app.emit("runtime:job_log", JobLogPayload { job_id, lines });
                        }
                        if let Some(damage) = effect.preview_damage {
                            if let Some(open) = preview.as_ref() {
                                emitter.preview_cells(&mut store, open, &damage);
                            }
                        }
                        if effect.preview_detached {
                            let _ = app.emit("runtime:preview_detached", ());
                        }
                        if let Some((workspace, reason)) = effect.worktree_blocked {
                            let _ = app.emit(
                                "runtime:worktree_blocked",
                                WorktreeBlockedPayload { workspace, reason },
                            );
                        }
                        if let Some((project, branch, reason)) = effect.worktree_create_failed {
                            let _ = app.emit(
                                "runtime:worktree_create_failed",
                                WorktreeCreateFailedPayload {
                                    project,
                                    branch,
                                    reason,
                                },
                            );
                        }
                        if let Some((profile, error)) = effect.profile_save {
                            let _ = app.emit(
                                "runtime:profile_save",
                                AgentProfileSavePayload { profile, error },
                            );
                        }
                    }
                    // A refusal leaves the connection alone: the shell says
                    // what happened and stays where it is.
                    Err(CommandError::Refused(reason)) => {
                        tracing::info!(reason, "command refused");
                        let _ = app.emit("runtime:notice", NoticePayload { reason });
                    }
                    Err(CommandError::Disconnected(reason)) => {
                        emit_disconnected(&app, &latest, reason);
                        reconnect = true;
                        break;
                    }
                }
            }
            if reconnect {
                break;
            }

            // Output held back by the floor goes out here, before the thread
            // blocks: a burst that stops inside the floor must not sit unsent
            // until some unrelated event wakes the loop.
            if let Some((since, damage)) = held.take() {
                if since.elapsed() >= cell_send_floor(at.scroll_offset) {
                    last_cells_sent = Some(Instant::now());
                    emitter.cells(&mut store, &at, &damage, std::mem::take(&mut pending_echo));
                } else {
                    held = Some((since, damage));
                }
            }

            let wait = match &held {
                Some((since, _)) => {
                    cell_send_floor(at.scroll_offset).saturating_sub(since.elapsed())
                }
                None => LIVENESS_TICK,
            };

            let event = match events.try_recv() {
                Ok(event) => Some(event),
                Err(flume::TryRecvError::Disconnected) => {
                    reconnect = true;
                    None
                }
                Err(flume::TryRecvError::Empty) => match flume::Selector::new()
                    .recv(&events, |result| match result {
                        Ok(event) => Selected::Event(Box::new(event)),
                        Err(_) => Selected::Closed,
                    })
                    .recv(&commands, |result| match result {
                        Ok(command) => Selected::Command(command),
                        Err(_) => Selected::Closed,
                    })
                    .wait_timeout(wait)
                {
                    Ok(Selected::Event(event)) => Some(*event),
                    Ok(Selected::Command(command)) => {
                        pending_command = Some(command);
                        None
                    }
                    Ok(Selected::Closed) => {
                        reconnect = true;
                        None
                    }
                    Err(_) => {
                        if !client.is_connected() {
                            reconnect = true;
                        }
                        None
                    }
                },
            };

            if let Some(event) = event {
                let mut batch = Batch::default();
                batch.absorb(&event, &store, &at, preview.as_ref());
                emit_job_event(&app, &event);
                batch.apply(
                    &event,
                    &mut store,
                    &client,
                    size,
                    at.terminal,
                    preview.as_ref(),
                );
                while let Ok(more) = events.try_recv() {
                    batch.absorb(&more, &store, &at, preview.as_ref());
                    emit_job_event(&app, &more);
                    batch.apply(
                        &more,
                        &mut store,
                        &client,
                        size,
                        at.terminal,
                        preview.as_ref(),
                    );
                }

                // A previewed session that exits leaves a terminal the daemon
                // has already dropped; holding the attachment would leak it
                // until the tab closed.
                if batch.preview_session_removed {
                    preview = None;
                    let _ = app.emit("runtime:preview_detached", ());
                }

                if batch.active_session_removed {
                    let Some(next_session) = successor_session(&store, batch.departing) else {
                        break;
                    };
                    match switch_session(&client, &mut store, at.terminal, next_session, size) {
                        Ok(next_terminal) => {
                            at.session = next_session;
                            at.terminal = next_terminal;
                            at.scroll_offset = 0;
                            preferred_session = Some(next_session);
                            batch.damage = Some(Damage::Full);
                        }
                        Err(error) => {
                            tracing::warn!(%error, "failed to leave a removed session");
                            break;
                        }
                    }
                }

                flush_pending_closes(&mut pending_close, &store, &client);

                if batch.shell {
                    emitter.shell(&store, &at);
                }
                // Not held back by the cell floor: the preview is 12 rows of a
                // job's output, so its frames are small and rare next to the
                // main canvas's, and delaying them would make a watched
                // session look stalled.
                if let Some(damage) = batch.preview_damage {
                    if let Some(open) = preview.as_ref() {
                        emitter.preview_cells(&mut store, open, &damage);
                    }
                }
                if let Some(damage) = batch.damage {
                    // A cells-only burst inside the floor is held and merged;
                    // anything the shell also cares about goes out at once, so
                    // a session appearing is never delayed by terminal output.
                    let floor = cell_send_floor(at.scroll_offset);
                    let within_floor = last_cells_sent.is_some_and(|sent| sent.elapsed() < floor);
                    if !batch.shell && within_floor {
                        held = Some(match held.take() {
                            Some((since, held_damage)) => (since, held_damage.merge(damage)),
                            None => (Instant::now(), damage),
                        });
                    } else {
                        let damage = match held.take() {
                            Some((_, held_damage)) => held_damage.merge(damage),
                            None => damage,
                        };
                        last_cells_sent = Some(Instant::now());
                        emitter.cells(&mut store, &at, &damage, std::mem::take(&mut pending_echo));
                    }
                }
            }
            if reconnect {
                break;
            }
        }

        // Retire the worker before the client goes: a queued read against a
        // dead socket is an error the panel would have to interpret, and it
        // already knows the connection dropped.
        *lock(&workbench) = None;
        if preview.take().is_some() {
            let _ = app.emit("runtime:preview_detached", ());
        }
        emit_disconnected(
            &app,
            &latest,
            "daemon connection lost; reconnecting".to_string(),
        );
        thread::sleep(Duration::from_millis(150));
    }
}

/// Why a command did not happen.
///
/// The distinction is the whole point: clicking a session that has already
/// exited must say so and leave the shell where it is. Treating it as a
/// transport failure — which is what a bare `String` error did — tore down the
/// daemon connection and reconnected, so the click looked like it did nothing.
#[derive(Debug)]
enum CommandError {
    /// The command could not be carried out. The connection is fine.
    Refused(String),
    /// The connection is gone; the loop reconnects.
    Disconnected(String),
}

impl CommandError {
    /// Classify a `ClientError`.
    ///
    /// A structured error from the daemon is an answer — it arrived, so the
    /// socket is alive. Only the transport variants mean the connection went.
    fn from_client(error: client::ClientError) -> Self {
        match error {
            client::ClientError::Protocol(_) | client::ClientError::UnexpectedResponse { .. } => {
                Self::Refused(error.to_string())
            }
            _ => Self::Disconnected(error.to_string()),
        }
    }

    fn refused(reason: impl Into<String>) -> Self {
        Self::Refused(reason.into())
    }

    fn reason(&self) -> &str {
        match self {
            Self::Refused(reason) | Self::Disconnected(reason) => reason,
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// A refusal the WebView can show, without touching the connection pill.
#[derive(Clone, Debug, Serialize)]
struct NoticePayload {
    reason: String,
}

/// A safe worktree removal needs a second, explicit forced confirmation.
#[derive(Clone, Debug, Serialize)]
struct WorktreeBlockedPayload {
    workspace: WorkspaceId,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct WorktreeCreateFailedPayload {
    project: ProjectId,
    branch: String,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct AgentProfileSavePayload {
    profile: AgentProfileId,
    error: Option<String>,
}

fn emit_job_event(app: &AppHandle, event: &DaemonEvent) {
    match event {
        DaemonEvent::JobUpdated(job) => {
            let _ = app.emit("runtime:job_updated", job.as_ref());
        }
        DaemonEvent::JobOutput {
            job_id,
            from_line,
            lines,
        } => {
            let _ = app.emit(
                "runtime:job_output",
                JobOutputPayload {
                    job_id: *job_id,
                    from_line: *from_line,
                    lines: lines.clone(),
                },
            );
        }
        // Provisioning acks when it starts, so this is where the GUI learns
        // what a worktree actually got (§14.2). It is not shell state — the
        // actions are a read, like a diff — so it rides its own event rather
        // than republishing the snapshot.
        DaemonEvent::SharesApplied {
            workspace_id,
            trigger,
            actions,
            error,
        } => {
            let _ = app.emit(
                "runtime:shares_applied",
                SharesAppliedPayload {
                    workspace: *workspace_id,
                    trigger: *trigger,
                    actions: actions.clone(),
                    error: error.clone(),
                },
            );
        }
        // Juva acks when the draft starts (its endpoint opens a socket), so
        // the text arrives here rather than as a response. Not shell state:
        // a draft is a read, like a diff, and rides its own event.
        DaemonEvent::JuvaDraftReady {
            workspace_id,
            draft,
            fell_back,
        } => {
            let _ = app.emit(
                "runtime:juva_draft",
                JuvaDraftPayload {
                    workspace: *workspace_id,
                    draft: draft.clone(),
                    fell_back: *fell_back,
                },
            );
        }
        _ => {}
    }
}

#[derive(Clone, Debug, Serialize)]
struct JuvaDraftPayload {
    workspace: WorkspaceId,
    draft: domain::JuvaDraft,
    /// The endpoint was configured but the local draft is what came back.
    fell_back: bool,
}

#[derive(Clone, Debug, Serialize)]
struct SharesAppliedPayload {
    workspace: WorkspaceId,
    trigger: domain::ShareTrigger,
    actions: Vec<domain::ShareAction>,
    error: Option<String>,
}

/// What one drained command asks the loop to publish.
#[derive(Default)]
struct Effect {
    /// The shell lists changed.
    shell: bool,
    /// The viewport has to repaint.
    damage: Option<Damage>,
    /// Text to hand the WebView for the clipboard.
    clipboard: Option<String>,
    /// A question was accepted as a headless job.
    lieutenant_job: Option<(ProjectId, Box<domain::Job>)>,
    /// The question could not be accepted, but the daemon connection survived.
    lieutenant_failed: Option<(ProjectId, String)>,
    /// Existing output read for a job that was opened after it started.
    job_log: Option<(JobId, Vec<String>)>,
    /// The preview terminal has to repaint.
    preview_damage: Option<Damage>,
    /// The preview was let go; the tab clears its canvas.
    preview_detached: bool,
    /// An unforced worktree removal needs a second confirmation.
    worktree_blocked: Option<(WorkspaceId, String)>,
    /// A create refusal belongs in the dialog that initiated it.
    worktree_create_failed: Option<(ProjectId, String, String)>,
    /// Profile validation belongs beside the values the user can correct.
    profile_save: Option<(AgentProfileId, Option<String>)>,
    /// Kill a live session, then `CloseSession` once it is terminal.
    pending_close: Option<SessionId>,
    /// A restart (or a close of an already-dead session) drops a pending kill.
    forget_pending_close: Option<SessionId>,
}

/// What `create_session` needs to start an agent rather than a shell.
///
/// A struct rather than a tuple because it has grown past the point where the
/// call site reads: `Some((provider, profile, resume, prompt, parent))` says
/// nothing about which `Option<String>` is which.
struct AgentLaunch {
    provider: AgentProviderId,
    profile: Option<AgentProfileId>,
    resume: Option<String>,
    prompt: Option<String>,
    /// Launch in the provider's own read-only mode, and tag the session as a
    /// review (§16.9). The daemon refuses a provider that declares no such
    /// mode rather than starting one that could write.
    read_only: bool,
    /// The session this one was handed off from, so the graph records the edge
    /// and the rail nests it under the session it came from (ADR-010).
    parent: Option<SessionId>,
}

impl Effect {
    fn nothing() -> Self {
        Self::default()
    }

    fn shell() -> Self {
        Self {
            shell: true,
            damage: Some(Damage::Full),
            ..Self::default()
        }
    }

    fn repaint() -> Self {
        Self {
            shell: false,
            damage: Some(Damage::Full),
            ..Self::default()
        }
    }
}

/// Sessions the user asked to close that still have to die first.
///
/// Close on a live session only kills (§7.3). The daemon refuses `CloseSession`
/// until the process is terminal, so the host remembers the id and removes the
/// row on exit — otherwise the rail keeps a ghost check-and-dot.
#[must_use]
fn due_closes(pending: &HashSet<SessionId>, store: &Store) -> Vec<SessionId> {
    pending
        .iter()
        .copied()
        .filter(|id| {
            store
                .sessions
                .iter()
                .find(|session| session.id == *id)
                .is_none_or(|session| !session.state.is_active())
        })
        .collect()
}

fn flush_pending_closes(pending: &mut HashSet<SessionId>, store: &Store, client: &Client) {
    for id in due_closes(pending, store) {
        pending.remove(&id);
        if !store.sessions.iter().any(|session| session.id == id) {
            continue;
        }
        if let Err(error) = client.close_session(id) {
            tracing::warn!(%error, "failed to close session after kill");
            pending.insert(id);
        }
    }
}

/// Run one command. `Err` means the connection is gone and the loop reconnects.
fn run_command(
    command: RuntimeCommand,
    client: &Client,
    store: &mut Store,
    at: &mut Attached,
    preview: &mut Option<Preview>,
    size: &mut PtySize,
    pending_echo: &mut u64,
) -> Result<Effect, CommandError> {
    match command {
        RuntimeCommand::Input { key, id } => {
            let modes = store.terminal(&at.terminal).map(|grid| grid.modes);
            let Some(bytes) = input::encode(&key, &modes.unwrap_or_default()) else {
                return Ok(Effect::nothing());
            };
            write_input(client, at, bytes, id, pending_echo)
        }
        RuntimeCommand::InputText { text, id } => {
            write_input(client, at, input::encode_text(&text), id, pending_echo)
        }
        RuntimeCommand::Paste { text, id } => {
            let modes = store
                .terminal(&at.terminal)
                .map(|grid| grid.modes)
                .unwrap_or_default();
            let bytes = input::encode_paste(&text, &modes);
            write_input(client, at, bytes, id, pending_echo)
        }
        // The pane only sends these while a program asked to read the mouse,
        // and the encoder refuses the events the *active* mode does not report
        // — so an event that gets this far and encodes to nothing is simply
        // one 1000-mode does not want, not an error.
        RuntimeCommand::Mouse {
            button,
            kind,
            col,
            row,
            ctrl,
            alt,
            shift,
        } => {
            let modes = store
                .terminal(&at.terminal)
                .map(|grid| grid.modes)
                .unwrap_or_default();
            let mods = client::Modifiers { ctrl, alt, shift };
            let Some(bytes) = input::encode_mouse_event(&button, &kind, col, row, mods, &modes)
            else {
                return Ok(Effect::nothing());
            };
            client
                .write_terminal_input(at.terminal, bytes)
                .map_err(CommandError::from_client)?;
            // No echo id and no scroll snap: a mouse report is not typing, and
            // yanking the viewport to the live output under a program that is
            // painting its own scrollback would fight it.
            Ok(Effect::nothing())
        }
        RuntimeCommand::Scroll { lines } => Ok(scroll(client, store, at, lines)),
        RuntimeCommand::ScrollToBottom => {
            if at.scroll_offset == 0 {
                return Ok(Effect::nothing());
            }
            at.scroll_offset = 0;
            Ok(Effect::repaint())
        }
        RuntimeCommand::Repaint => Ok(Effect::repaint()),
        RuntimeCommand::CopySelection {
            anchor_line,
            anchor_col,
            head_line,
            head_col,
        } => {
            let text = store.terminal(&at.terminal).map(|grid| {
                cells::selection_text(
                    grid,
                    at.scroll_offset,
                    (anchor_line, anchor_col),
                    (head_line, head_col),
                )
            });
            Ok(Effect {
                clipboard: text,
                ..Effect::nothing()
            })
        }
        RuntimeCommand::SelectSession { session_id } => {
            if session_id == at.session {
                return Ok(Effect::nothing());
            }
            let terminal = switch_session(client, store, at.terminal, session_id, *size)?;
            at.session = session_id;
            at.terminal = terminal;
            at.scroll_offset = 0;
            Ok(Effect::shell())
        }
        RuntimeCommand::NewShell { workspace } => {
            let (session, terminal) =
                create_session(client, store, at.terminal, *size, workspace, None)?;
            at.session = session;
            at.terminal = terminal;
            at.scroll_offset = 0;
            Ok(Effect::shell())
        }
        RuntimeCommand::NewAgent {
            provider,
            profile,
            workspace,
            resume,
            prompt,
            parent,
            read_only,
        } => {
            let (session, terminal) = create_session(
                client,
                store,
                at.terminal,
                *size,
                workspace,
                Some(AgentLaunch {
                    provider,
                    profile,
                    resume,
                    prompt,
                    parent,
                    read_only,
                }),
            )?;
            at.session = session;
            at.terminal = terminal;
            at.scroll_offset = 0;
            Ok(Effect::shell())
        }
        RuntimeCommand::AskLieutenant {
            project,
            question,
            resume_from,
        } => match client.ask_harness(project, question, resume_from) {
            Ok(job) => Ok(Effect {
                lieutenant_job: Some((project, Box::new(job))),
                ..Effect::nothing()
            }),
            Err(error) => match CommandError::from_client(error) {
                CommandError::Refused(reason) => Ok(Effect {
                    lieutenant_failed: Some((project, reason)),
                    ..Effect::nothing()
                }),
                error @ CommandError::Disconnected(_) => Err(error),
            },
        },
        RuntimeCommand::ReadJobLog { job_id } => match client.read_job_log(job_id, 0) {
            Ok((lines, _)) => Ok(Effect {
                job_log: Some((job_id, lines)),
                ..Effect::nothing()
            }),
            Err(error) => Err(CommandError::from_client(error)),
        },
        RuntimeCommand::Resize { size: requested } => {
            let requested = requested.sanitized();
            if requested == *size {
                return Ok(Effect::nothing());
            }
            *size = requested;
            resize_and_reattach(client, store, at.terminal, *size)?;
            // A resize changes how much history the viewport shows; staying
            // scrolled through it would land on rows the new geometry never
            // laid out.
            at.scroll_offset = 0;
            Ok(Effect::repaint())
        }
        RuntimeCommand::SetAppState { key, value } => {
            client
                .set_app_state(&key, &value)
                .map_err(CommandError::from_client)?;
            // SetAppState has no broadcast; publish the acknowledged value locally.
            if let Some((_, stored)) = store.app_state.iter_mut().find(|(name, _)| name == &key) {
                *stored = value;
            } else {
                store.app_state.push((key, value));
            }
            Ok(Effect {
                shell: true,
                ..Effect::nothing()
            })
        }
        RuntimeCommand::FactoryReset => {
            client.factory_reset().map_err(CommandError::from_client)?;
            // `FactoryReset` is broadcast to every client. Its event path drops
            // this attachment and bootstraps the fresh state.
            Ok(Effect::nothing())
        }
        RuntimeCommand::RefreshSnapshot => {
            client
                .refresh_store(store)
                .map_err(CommandError::from_client)?;
            Ok(Effect::shell())
        }
        RuntimeCommand::Reconnect => Err(CommandError::Disconnected(
            "reconnect requested".to_string(),
        )),
        RuntimeCommand::KillSession { session_id } => {
            client
                .kill_session(session_id)
                .map_err(CommandError::from_client)?;
            // The daemon broadcasts the state change; the row updates from the
            // event like any other.
            Ok(Effect::nothing())
        }
        RuntimeCommand::RestartSession { session_id } => {
            client
                .restart_session(session_id)
                .map_err(CommandError::from_client)?;
            // A restart mints a fresh terminal, and the shell follows it there:
            // restarting the session you are looking at and staying attached to
            // the dead one is not what the gesture meant.
            match switch_session(client, store, at.terminal, session_id, *size) {
                Ok(terminal) => {
                    at.session = session_id;
                    at.terminal = terminal;
                    at.scroll_offset = 0;
                    Ok(Effect {
                        forget_pending_close: Some(session_id),
                        ..Effect::shell()
                    })
                }
                // The new PTY is not in the replica yet; the `SessionUpdated`
                // that carries it will bring the row back.
                Err(CommandError::Refused(_)) => Ok(Effect {
                    forget_pending_close: Some(session_id),
                    ..Effect::shell()
                }),
                Err(error) => Err(error),
            }
        }
        RuntimeCommand::RenameSession { session_id, title } => {
            client
                .rename_session(session_id, title)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RefreshWorkspaceStatus { workspace } => {
            client
                .refresh_workspace_status(workspace)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::CloseSession { session_id } => {
            let live = store
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .is_some_and(|session| session.state.is_active());
            if live {
                if let Err(error) = client.kill_session(session_id) {
                    tracing::warn!(%error, "failed to kill session before closing");
                }
                // Kill is async: CloseSession is refused until the process is
                // terminal, so remember the id and remove the row on exit.
                Ok(Effect {
                    pending_close: Some(session_id),
                    ..Effect::nothing()
                })
            } else {
                if let Err(error) = client.close_session(session_id) {
                    tracing::warn!(%error, "failed to close session");
                }
                Ok(Effect {
                    forget_pending_close: Some(session_id),
                    ..Effect::nothing()
                })
            }
        }
        RuntimeCommand::CreateWorktree {
            project,
            branch,
            base,
            name,
        } => create_worktree_effect(
            project,
            branch.clone(),
            client.create_worktree(project, &branch, base, name),
        ),
        RuntimeCommand::FetchRemote { project, remote } => {
            match client.fetch_remote(project, remote) {
                Ok(()) => Ok(Effect::nothing()),
                Err(error) => {
                    tracing::warn!(%error, "failed to start a fetch");
                    Err(CommandError::from_client(error))
                }
            }
        }
        RuntimeCommand::OpenInEditor { editor, path } => {
            let Some(editor) = crate::open::Editor::parse(&editor) else {
                return Ok(Effect::nothing());
            };
            if let Err(error) = crate::open::open_editor(editor, Path::new(&path)) {
                tracing::warn!(%error, path, "failed to open in editor");
            }
            Ok(Effect::nothing())
        }
        RuntimeCommand::OpenInFileManager { path } => {
            if let Err(error) = crate::open::open_file_manager(Path::new(&path)) {
                tracing::warn!(%error, path, "failed to open file manager");
            }
            Ok(Effect::nothing())
        }
        RuntimeCommand::OpenUrl { url } => {
            if let Err(error) = crate::open::open_url(&url) {
                tracing::warn!(%error, url, "failed to open URL");
            }
            Ok(Effect::nothing())
        }

        // ------------------------------------------------------ projects ---
        //
        // All of these ack and broadcast, so the row that changed redraws from
        // the event rather than from anything published here.
        RuntimeCommand::AddProject { path, group } => {
            let path = Path::new(&path);
            let result = match group {
                Some(group) => client.add_project_to_group(path, group),
                None => client.add_project(path),
            };
            result.map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RefreshProject { project } => {
            client
                .refresh_project(project)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::MoveProject { project, group } => {
            client
                .move_project(project, group)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::SetProjectIcon { project, icon } => {
            client
                .set_project_icon(project, icon)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::CreateProjectGroup { name } => {
            client
                .create_project_group(&name)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RenameProjectGroup { group, name } => {
            client
                .rename_project_group(group, &name)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RemoveProjectGroup { group } => {
            client
                .remove_project_group(group)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RemoveProject { project, policy } => {
            client
                .remove_project(project, policy.into())
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RenameWorkspace {
            workspace,
            display_name,
        } => {
            client
                .rename_workspace(workspace, display_name)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RemoveWorktree { workspace, force } => {
            remove_worktree_effect(workspace, force, client.remove_worktree(workspace, force))
        }

        // -------------------------------------------------------- agents ---
        RuntimeCommand::RefreshDetection { provider } => {
            client
                .refresh_detection(provider)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::SaveAgentProfile { profile } => {
            let profile_id = profile.id;
            save_agent_profile_effect(profile_id, client.save_agent_profile(profile))
        }
        RuntimeCommand::RemoveAgentProfile { profile } => {
            client
                .remove_agent_profile(profile)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::SetProjectShares { project, rules } => {
            client
                .set_project_shares(project, rules)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RemoveShareRule {
            project,
            rule,
            cleanup,
        } => {
            client
                .remove_share_rule(project, rule, cleanup)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::SetProviderExecutable { provider, path } => {
            client
                .set_provider_executable(&provider, path.map(std::path::PathBuf::from))
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }

        // ------------------------------------------------------- harness ---
        RuntimeCommand::LinkHarnessSession {
            project,
            feature,
            session_id,
        } => {
            client
                .link_harness_session(project, feature, session_id)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::CancelJob { job_id } => {
            client
                .cancel_job(job_id)
                .map_err(CommandError::from_client)?;
            // The daemon publishes the job's new state; nothing to echo.
            Ok(Effect::nothing())
        }
        RuntimeCommand::AttachHarnessPreview { session_id, size } => {
            let requested = size.unwrap_or(PREVIEW_SIZE).sanitized();
            attach_preview(client, store, preview, at.terminal, session_id, requested)
        }
        RuntimeCommand::DetachHarnessPreview => Ok(detach_preview(client, preview, at.terminal)),
        RuntimeCommand::ResizeHarnessPreview { size: requested } => {
            let requested = requested.sanitized();
            let Some(open) = preview.as_mut() else {
                return Ok(Effect::nothing());
            };
            if open.size == requested {
                return Ok(Effect::nothing());
            }
            open.size = requested;
            // Re-attach rather than `ResizeTerminal`: the daemon adopts an
            // attach's size only while the terminal is *unshared*, so a
            // session somebody else is also watching keeps the geometry that
            // viewer negotiated instead of being pulled down to 12 rows. The
            // snapshot that comes back is the new viewport either way, which
            // is what the full repaint below paints.
            let snapshot = client
                .attach_terminal(open.terminal, requested)
                .map_err(CommandError::from_client)?;
            store.attach_terminal(open.terminal, &snapshot);
            Ok(Effect {
                preview_damage: Some(Damage::Full),
                ..Effect::nothing()
            })
        }
        // Typed into the preview, which is a real terminal: a feature waiting
        // on a prompt can be answered without leaving the tab. Never bracketed
        // and never echoed — the preview has no latency sample to close.
        RuntimeCommand::InputPreview { key } => {
            let Some(open) = preview.as_ref() else {
                return Ok(Effect::nothing());
            };
            let modes = store.terminal(&open.terminal).map(|grid| grid.modes);
            let Some(bytes) = input::encode(&key, &modes.unwrap_or_default()) else {
                return Ok(Effect::nothing());
            };
            client
                .write_terminal_input(open.terminal, bytes)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }

        // The daemon going away is the connection going away, so the loop
        // reconnecting afterwards is correct: it finds nothing, says so, and
        // keeps retrying until the user starts one again.
        RuntimeCommand::StopDaemon { kill_sessions } => {
            client
                .stop_daemon(kill_sessions)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
    }
}

fn remove_worktree_effect(
    workspace: WorkspaceId,
    force: bool,
    result: Result<(), client::ClientError>,
) -> Result<Effect, CommandError> {
    match result {
        Ok(()) => Ok(Effect::nothing()),
        Err(client::ClientError::Protocol(error))
            if !force && error.code == client::ErrorCode::PreconditionFailed =>
        {
            Ok(Effect {
                worktree_blocked: Some((workspace, error.message)),
                ..Effect::default()
            })
        }
        Err(error) => Err(CommandError::from_client(error)),
    }
}

fn create_worktree_effect(
    project: ProjectId,
    branch: String,
    result: Result<(), client::ClientError>,
) -> Result<Effect, CommandError> {
    match result {
        Ok(()) => Ok(Effect::nothing()),
        Err(client::ClientError::Protocol(error)) => Ok(Effect {
            worktree_create_failed: Some((project, branch, error.message)),
            ..Effect::default()
        }),
        Err(error) => match CommandError::from_client(error) {
            CommandError::Refused(reason) => Ok(Effect {
                worktree_create_failed: Some((project, branch, reason)),
                ..Effect::default()
            }),
            error => Err(error),
        },
    }
}

fn save_agent_profile_effect(
    profile: AgentProfileId,
    result: Result<(), client::ClientError>,
) -> Result<Effect, CommandError> {
    match result {
        Ok(()) => Ok(Effect {
            profile_save: Some((profile, None)),
            ..Effect::default()
        }),
        Err(client::ClientError::Protocol(error)) => Ok(Effect {
            profile_save: Some((profile, Some(error.message))),
            ..Effect::default()
        }),
        Err(error) => match CommandError::from_client(error) {
            CommandError::Refused(reason) => Ok(Effect {
                profile_save: Some((profile, Some(reason))),
                ..Effect::default()
            }),
            error => Err(error),
        },
    }
}

/// Point the preview at `session_id`, letting go of whatever it held.
///
/// Attaching to the session the main canvas is already on is refused rather
/// than done twice: the daemon would send its deltas under one terminal id and
/// both surfaces would paint them, but detaching the preview later would then
/// detach the terminal the user is working in.
fn attach_preview(
    client: &Client,
    store: &mut Store,
    preview: &mut Option<Preview>,
    main: TerminalId,
    session_id: SessionId,
    size: PtySize,
) -> Result<Effect, CommandError> {
    let Some(terminal) = store
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.terminal_id)
    else {
        return Err(CommandError::refused(
            "that session has no terminal to preview",
        ));
    };
    if terminal == main {
        return Err(CommandError::refused(
            "that session is already open in the terminal pane",
        ));
    }

    if let Some(open) = preview.as_ref() {
        if open.terminal == terminal {
            return Ok(Effect::nothing());
        }
    }
    // Let the old one go first, so two previews are never attached at once.
    let detached = detach_preview(client, preview, main);

    let snapshot = client
        .attach_terminal(terminal, size)
        .map_err(CommandError::from_client)?;
    store.attach_terminal(terminal, &snapshot);
    *preview = Some(Preview {
        session: session_id,
        terminal,
        size,
    });
    Ok(Effect {
        preview_damage: Some(Damage::Full),
        // The detach that just happened is not news to the tab: the frame
        // below replaces its canvas anyway.
        preview_detached: false,
        ..detached
    })
}

/// Let the preview go, if it holds anything.
///
/// The terminal is only detached when the main canvas is not also on it —
/// otherwise closing a feature tab would blank the terminal the user is in.
fn detach_preview(client: &Client, preview: &mut Option<Preview>, main: TerminalId) -> Effect {
    let Some(open) = preview.take() else {
        return Effect::nothing();
    };
    if open.terminal != main {
        if let Err(error) = client.detach_terminal(open.terminal) {
            tracing::warn!(%error, "failed to detach the harness preview");
        }
    }
    Effect {
        preview_detached: true,
        ..Effect::nothing()
    }
}

/// Write encoded bytes to the attached terminal.
///
/// Typing snaps the viewport back to the live output: input that lands
/// somewhere the user cannot see is the worst outcome of a scrolled viewport.
fn write_input(
    client: &Client,
    at: &mut Attached,
    bytes: Vec<u8>,
    id: u64,
    pending_echo: &mut u64,
) -> Result<Effect, CommandError> {
    if bytes.is_empty() {
        return Ok(Effect::nothing());
    }
    client
        .write_terminal_input(at.terminal, bytes)
        .map_err(CommandError::from_client)?;
    *pending_echo = (*pending_echo).max(id);
    if at.scroll_offset != 0 {
        at.scroll_offset = 0;
        return Ok(Effect::repaint());
    }
    Ok(Effect::nothing())
}

/// Move the viewport through history, fetching the rows it is about to paint.
///
/// Refused on the alternate screen and while a program is reading the mouse:
/// a full-screen TUI scrolls itself, and stealing the wheel from it would
/// scroll our replica while the program under it stayed put.
fn scroll(client: &Client, store: &mut Store, at: &mut Attached, lines: i64) -> Effect {
    let Some(grid) = store.terminal(&at.terminal) else {
        return Effect::nothing();
    };
    if grid.modes.alt_screen || grid.modes.mouse_mode != MouseMode::Off {
        return Effect::nothing();
    }
    let limit = grid.scrollback_len as i64;
    let next = (at.scroll_offset as i64 + lines).clamp(0, limit) as u64;
    if next == at.scroll_offset {
        return Effect::nothing();
    }
    at.scroll_offset = next;
    request_scrollback(client, store, at);
    Effect::repaint()
}

/// Ask the daemon for the history the viewport is about to paint, if the cache
/// does not already hold it.
///
/// The request starts half a page earlier than strictly needed, so scrolling
/// steadily in one direction keeps hitting the cache instead of stalling on a
/// round trip at every page boundary.
fn request_scrollback(client: &Client, store: &mut Store, at: &Attached) {
    let Some(grid) = store.terminal(&at.terminal) else {
        return;
    };
    if cells::viewport_is_cached(grid, at.scroll_offset) {
        return;
    }
    let Some(oldest) = cells::oldest_needed_line(grid, at.scroll_offset) else {
        return;
    };
    let from_line = (oldest - i64::from(cells::SCROLLBACK_PAGE) / 2).max(0);
    match client.fetch_scrollback(at.terminal, from_line, cells::SCROLLBACK_PAGE) {
        Ok(block) => store.merge_scrollback(&at.terminal, &block),
        Err(error) => tracing::warn!(%error, "failed to fetch scrollback"),
    }
}

/// One drained batch of daemon events, and what it asks the loop to publish.
#[derive(Default)]
struct Batch {
    shell: bool,
    damage: Option<Damage>,
    /// Rows the *preview* has to repaint, tracked apart from `damage` so a
    /// watched session printing a line never repaints the main canvas.
    preview_damage: Option<Damage>,
    active_session_removed: bool,
    /// Where the session that just went was, read while the store still had
    /// the row: `successor_session` needs its checkout to stay in it.
    departing: Option<Departing>,
    /// The previewed session went away; the tab clears its canvas.
    preview_session_removed: bool,
}

/// The active session as the store last described it, kept past its removal.
///
/// The project is resolved here rather than from `workspace` later, because
/// removing a worktree broadcasts `SessionRemoved` and `WorkspaceRemoved` in
/// one burst: by the time the successor is chosen the checkout's own row can
/// be gone too.
#[derive(Clone, Copy)]
struct Departing {
    workspace: WorkspaceId,
    project: Option<ProjectId>,
    /// Its place in the strip, which is `created_at` then id (`tab_key`).
    key: (Timestamp, SessionId),
}

impl Batch {
    /// Read what an event means for the shell and the viewport, *before* it is
    /// applied: a delta names the rows it damages, and the store does not keep
    /// them.
    fn absorb(
        &mut self,
        event: &DaemonEvent,
        store: &Store,
        at: &Attached,
        preview: Option<&Preview>,
    ) {
        if matches!(event, DaemonEvent::FactoryReset) {
            self.active_session_removed = true;
            self.preview_session_removed = preview.is_some();
        }
        if let DaemonEvent::SessionRemoved { session_id } = event {
            if *session_id == at.session {
                self.active_session_removed = true;
                self.departing = store
                    .sessions
                    .iter()
                    .find(|session| session.id == *session_id)
                    .map(|session| Departing {
                        workspace: session.workspace_id,
                        project: project_of(store, session.workspace_id),
                        key: tab_key(session),
                    });
            }
            if preview.is_some_and(|open| open.session == *session_id) {
                self.preview_session_removed = true;
            }
        }
        if let Some(open) = preview {
            let preview_damage = match event {
                DaemonEvent::TerminalDelta { terminal_id, delta }
                    if *terminal_id == open.terminal =>
                {
                    Some(delta_damage(delta))
                }
                DaemonEvent::TerminalResync { terminal_id, .. }
                    if *terminal_id == open.terminal =>
                {
                    Some(Damage::Full)
                }
                _ => None,
            };
            if let Some(damage) = preview_damage {
                self.preview_damage = Some(match self.preview_damage.take() {
                    Some(existing) => existing.merge(damage),
                    None => damage,
                });
            }
        }
        let damage = match event {
            DaemonEvent::TerminalDelta { terminal_id, delta } if *terminal_id == at.terminal => {
                Some(delta_damage(delta))
            }
            DaemonEvent::TerminalResync { terminal_id, .. } if *terminal_id == at.terminal => {
                Some(Damage::Full)
            }
            _ => None,
        };
        match damage {
            Some(damage) => {
                self.damage = Some(match self.damage.take() {
                    Some(existing) => existing.merge(damage),
                    None => damage,
                });
            }
            None if changes_shell(event, store) => self.shell = true,
            None => {}
        }
    }

    /// Apply the event to the replica, re-attaching when the sequence gapped.
    ///
    /// `watched` is every terminal a surface is painting — the main canvas and
    /// the harness preview — because a resync is only worth the round trip for
    /// a terminal somebody is looking at.
    fn apply(
        &mut self,
        event: &DaemonEvent,
        store: &mut Store,
        client: &Client,
        size: PtySize,
        active_terminal: TerminalId,
        preview: Option<&Preview>,
    ) {
        if let EventOutcome::NeedsResync { terminal_id } = store.apply_event(event) {
            if terminal_id == active_terminal {
                if let Ok(snapshot) = client.attach_terminal(terminal_id, size) {
                    store.attach_terminal(terminal_id, &snapshot);
                    self.damage = Some(Damage::Full);
                }
            } else if let Some(open) = preview.filter(|open| open.terminal == terminal_id) {
                if let Ok(snapshot) = client.attach_terminal(terminal_id, open.size) {
                    store.attach_terminal(terminal_id, &snapshot);
                    self.preview_damage = Some(Damage::Full);
                }
            }
        }
    }
}

/// Whether an event changes anything [`ShellSnapshot`] carries.
///
/// A blacklist rather than a whitelist because `DaemonEvent` is
/// `#[non_exhaustive]`: an event this build has never heard of is assumed to
/// matter, which costs one snapshot rather than losing one.
///
/// Publishing the shell is not cheap — it clones every domain list, builds the
/// launchables and the attention map, and puts the result through `serde_json`
/// on its way to the WebView — and it has no frame floor in front of it, so
/// anything that reaches it on the delta rung is a defect
/// (`docs/performance.md`, "compute on change, not on frame").
fn changes_shell(event: &DaemonEvent, store: &Store) -> bool {
    match event {
        // Provisioning results are a read with their own event, not shell
        // state: republishing the whole snapshot per applied rule would put
        // `from_store` on a path a `pnpm install` can drive.
        DaemonEvent::SharesApplied { .. } => false,
        // A draft is a read with its own event, for the same reason.
        DaemonEvent::JuvaDraftReady { .. } => false,
        // Terminal output is not shell state. The *attached* terminal's frames
        // never get here — they became `damage` above — but the preview's do,
        // and publishing the whole session tree for each of them put
        // `ShellSnapshot::from_store` on the delta rung: a watched harness
        // session printing steadily republished the shell up to 62 times a
        // second, which is exactly what `preview_cells` having its own event
        // exists to prevent.
        DaemonEvent::TerminalDelta { .. } | DaemonEvent::TerminalResync { .. } => false,
        // Job output has its own event path. Treating it as shell state would
        // serialize and send the whole snapshot for every streamed batch.
        DaemonEvent::JobUpdated(_) | DaemonEvent::JobOutput { .. } => false,
        // Both of these flip a badge in `session_attention`, and both are
        // *edges*: the set already holds the terminal after the first note, so
        // every one after it changes nothing a client could see. The daemon
        // coalesces activity to one note per second per terminal
        // (`terminal::ACTIVITY_NOTE`), so without this a handful of background
        // agents published a full snapshot every second apiece for a flag that
        // was already set.
        //
        // Checked before the event is applied, which is the order `absorb`
        // already runs in.
        DaemonEvent::TerminalActivity { terminal_id } => !store.has_unread(terminal_id),
        DaemonEvent::TerminalBell { terminal_id } => !store.wants_attention(terminal_id),
        _ => true,
    }
}

/// What one delta damages.
///
/// A scroll moves every row, and the rows the delta names are only the ones
/// that *also* changed content — so a scrolled delta is a full repaint however
/// few rows it lists.
fn delta_damage(delta: &domain::TerminalDelta) -> Damage {
    if delta.scrolled_lines > 0 {
        Damage::Full
    } else {
        Damage::Rows(delta.rows.iter().map(|(index, _)| *index).collect())
    }
}

fn bootstrap(
    client: &Client,
    preferred_session: Option<SessionId>,
    size: PtySize,
) -> Result<(Store, SessionId, TerminalId), String> {
    let mut store = client.load_store().map_err(|error| error.to_string())?;
    if store.workspaces.is_empty() {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        client
            .add_project(&cwd)
            .map_err(|error| error.to_string())?;
        store = client.load_store().map_err(|error| error.to_string())?;
    }

    let session_id = find_live_session(&store, preferred_session).or_else(|| {
        let workspace_id = store.workspaces.first()?.id;
        client.create_shell_session(workspace_id).ok()?;
        store = client.load_store().ok()?;
        find_live_session(&store, None)
    });
    let session_id =
        session_id.ok_or_else(|| "daemon did not create a live session".to_string())?;
    let terminal_id = attach_session(client, &mut store, session_id, size)
        .map_err(|error| error.reason().to_owned())?;
    Ok((store, session_id, terminal_id))
}

/// A session's place in the tab strip: creation order, ties broken by id.
///
/// The same key the window sorts the strip by, so "the tab before this one"
/// means the same thing on both sides of the IPC. The persisted order the user
/// may have dragged into lives in the window's `app_state` and is not readable
/// from here; it seeds from this one.
fn tab_key(session: &domain::Session) -> (Timestamp, SessionId) {
    (session.created_at, session.id)
}

fn project_of(store: &Store, workspace: WorkspaceId) -> Option<ProjectId> {
    store
        .workspaces
        .iter()
        .find(|item| item.id == workspace)
        .map(|item| item.project_id)
}

/// Which session takes the closed one's place on screen.
///
/// Ordered by how far the window has to travel: the departing session's own
/// checkout first — the tab before it, then the one after — then any other
/// checkout of the same project, and only then the newest live session
/// anywhere. The strip shows one checkout at a time, so answering a closed tab
/// with a global "newest" walks the window into a repository the person is not
/// looking at, which is the whole reason this is not `find_live_session`.
///
/// `None` for the departing session is a `FactoryReset`, where there is no
/// checkout left to prefer; a departing session whose project is `None` is one
/// whose checkout was never in the replica, which is the same answer.
fn successor_session(store: &Store, departing: Option<Departing>) -> Option<SessionId> {
    let Some(gone) = departing else {
        return find_live_session(store, None);
    };
    let live = || {
        store
            .sessions
            .iter()
            .filter(|session| session.terminal_id.is_some())
    };
    let in_checkout = || live().filter(|session| session.workspace_id == gone.workspace);
    let neighbour = in_checkout()
        .filter(|session| tab_key(session) < gone.key)
        .max_by_key(|session| tab_key(session))
        .or_else(|| {
            in_checkout()
                .filter(|session| tab_key(session) > gone.key)
                .min_by_key(|session| tab_key(session))
        });
    if let Some(session) = neighbour {
        return Some(session.id);
    }
    gone.project
        .and_then(|project| {
            live()
                .filter(|session| project_of(store, session.workspace_id) == Some(project))
                .max_by_key(|session| tab_key(session))
        })
        .map(|session| session.id)
        .or_else(|| find_live_session(store, None))
}

fn find_live_session(store: &Store, preferred: Option<SessionId>) -> Option<SessionId> {
    preferred
        .and_then(|id| {
            store
                .sessions
                .iter()
                .find(|session| session.id == id && session.terminal_id.is_some())
                .map(|session| session.id)
        })
        .or_else(|| {
            store
                .sessions
                .iter()
                .filter(|session| session.terminal_id.is_some())
                .max_by_key(|session| session.created_at)
                .map(|session| session.id)
        })
}

/// Attach to a session's terminal.
///
/// A session with no `terminal_id` is one that already exited: the daemon drops
/// the runtime id when the PTY goes. That is a refusal, not a broken socket —
/// the row is still in the tree, and clicking it must say so rather than take
/// the connection down.
fn attach_session(
    client: &Client,
    store: &mut Store,
    session_id: SessionId,
    size: PtySize,
) -> Result<TerminalId, CommandError> {
    let terminal_id = store
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.terminal_id)
        .ok_or_else(|| CommandError::refused("that session has exited; restart it to reopen it"))?;
    let snapshot = client
        .attach_terminal(terminal_id, size)
        .map_err(CommandError::from_client)?;
    store.attach_terminal(terminal_id, &snapshot);
    Ok(terminal_id)
}

/// Move to another session.
///
/// The new terminal is attached *before* the old one is let go. The other order
/// reads more naturally and is wrong: a switch that fails half-way would leave
/// the shell attached to nothing, with the session it was on still on screen
/// and no longer receiving output.
fn switch_session(
    client: &Client,
    store: &mut Store,
    old_terminal: TerminalId,
    session_id: SessionId,
    size: PtySize,
) -> Result<TerminalId, CommandError> {
    let terminal = attach_session(client, store, session_id, size)?;
    if terminal != old_terminal {
        let _ = client.detach_terminal(old_terminal);
        store.detach_terminal(&old_terminal);
    }
    if let Some(workspace) = store
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .map(|session| session.workspace_id)
    {
        let _ = client.refresh_workspace_status(workspace);
    }
    Ok(terminal)
}

/// The agent arm carries the whole launch as one value rather than parallel
/// `Option`s: a profile, a resume id or a read-only flag without a provider is
/// not a state this can be asked for.
fn create_session(
    client: &Client,
    store: &mut Store,
    old_terminal: TerminalId,
    size: PtySize,
    workspace: Option<WorkspaceId>,
    agent: Option<AgentLaunch>,
) -> Result<(SessionId, TerminalId), CommandError> {
    let workspace_id = workspace
        .filter(|id| store.workspaces.iter().any(|item| item.id == *id))
        .or_else(|| {
            store
                .sessions
                .iter()
                .find(|session| session.terminal_id == Some(old_terminal))
                .map(|session| session.workspace_id)
        })
        .or_else(|| store.workspaces.first().map(|workspace| workspace.id))
        .ok_or_else(|| CommandError::refused("no checkout is available to start it in"))?;
    let (session_id, terminal_id) = match agent {
        Some(agent) => client
            .create_agent_session_with_role(
                workspace_id,
                agent.provider,
                agent.profile,
                // A parent only makes sense inside the same graph; the daemon
                // validates depth and rejects a stranger.
                agent.parent,
                // A read-only launch is a review and says so in the rail; the
                // role is the only thing that outlives the flag.
                if agent.read_only {
                    domain::SessionRole::Reviewer
                } else {
                    domain::SessionRole::Generic
                },
                agent.resume,
                agent.prompt,
                agent.read_only,
            )
            .map_err(CommandError::from_client)?,
        None => client
            .create_shell_session(workspace_id)
            .map_err(CommandError::from_client)?,
    };
    let _ = client.detach_terminal(old_terminal);
    store.detach_terminal(&old_terminal);
    let snapshot = client
        .attach_terminal(terminal_id, size)
        .map_err(CommandError::from_client)?;
    store.attach_terminal(terminal_id, &snapshot);
    Ok((session_id, terminal_id))
}

fn resize_and_reattach(
    client: &Client,
    store: &mut Store,
    terminal_id: TerminalId,
    size: PtySize,
) -> Result<(), CommandError> {
    client
        .resize_terminal(terminal_id, size)
        .map_err(CommandError::from_client)?;
    let snapshot = client
        .attach_terminal(terminal_id, size)
        .map_err(CommandError::from_client)?;
    store.attach_terminal(terminal_id, &snapshot);
    Ok(())
}

/// Cache the payload the `connect` command answers from.
///
/// Takes it by value: this is the largest structure the bridge builds, and
/// taking a reference meant cloning it here on every shell publish.
fn remember_connected(latest: &Mutex<Latest>, payload: ConnectedPayload) {
    let mut guard = lock(latest);
    guard.status = HostStatus {
        client_ready: true,
        connected: true,
        session_count: payload.session_count,
        daemon_version: Some(payload.daemon.daemon_version.clone()),
        instance_id: Some(payload.daemon.instance_id.clone()),
        reason: None,
    };
    guard.payload = Some(payload);
}

fn emit_disconnected(app: &AppHandle, latest: &Mutex<Latest>, reason: String) {
    {
        let mut guard = lock(latest);
        guard.status = HostStatus {
            client_ready: true,
            connected: false,
            session_count: guard.payload.as_ref().map_or(0, |p| p.session_count),
            daemon_version: None,
            instance_id: None,
            reason: Some(reason.clone()),
        };
    }
    let _ = app.emit("runtime:disconnected", DisconnectedPayload { reason });
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Cursor, TermModes, TerminalDelta};
    use std::collections::HashSet;

    fn attached(terminal: TerminalId, session: SessionId, scroll_offset: u64) -> Attached {
        Attached {
            session,
            terminal,
            scroll_offset,
        }
    }

    fn delta(terminal_id: TerminalId, rows: Vec<u16>, scrolled_lines: u32) -> DaemonEvent {
        DaemonEvent::TerminalDelta {
            terminal_id,
            delta: TerminalDelta {
                seq: 1,
                rows: rows
                    .into_iter()
                    .map(|index| (index, domain::Row::blank(4)))
                    .collect(),
                scrolled_lines,
                cursor: Cursor::default(),
                modes: TermModes::default(),
            },
        }
    }

    #[test]
    fn a_delta_for_the_attached_terminal_damages_only_its_rows() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(at.terminal, vec![3, 1], 0), &store, &at, None);
        assert_eq!(batch.damage, Some(Damage::Rows(vec![3, 1])));
        assert!(!batch.shell, "terminal output is not shell state");
    }

    /// A scroll moved every row, and the rows the delta names are only the ones
    /// that also changed content — repainting just those would tear the frame.
    #[test]
    fn a_delta_that_scrolled_damages_the_whole_viewport() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(at.terminal, vec![0], 2), &store, &at, None);
        assert_eq!(batch.damage, Some(Damage::Full));
    }

    /// The only terminal that is not the attached one and still sends deltas is
    /// the harness preview, and its output is not shell state either.
    /// Publishing the session tree for each of its frames is what put
    /// `ShellSnapshot::from_store` on the delta rung.
    #[test]
    fn a_delta_for_another_terminal_is_not_shell_state() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(TerminalId::new(), vec![0], 0), &store, &at, None);
        assert_eq!(batch.damage, None);
        assert!(!batch.shell);
    }

    #[test]
    fn a_preview_delta_repaints_the_preview_and_nothing_else() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let open = Preview {
            session: SessionId::new(),
            terminal: TerminalId::new(),
            size: PREVIEW_SIZE,
        };
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(open.terminal, vec![2], 0), &store, &at, Some(&open));
        assert_eq!(batch.preview_damage, Some(Damage::Rows(vec![2])));
        assert_eq!(batch.damage, None);
        assert!(
            !batch.shell,
            "a watched session printing a line must not republish the shell"
        );
    }

    /// The first note flips the unread badge, so it is news. The daemon sends
    /// one per second per busy terminal, and the rest say what the store
    /// already knows.
    #[test]
    fn only_the_first_activity_note_for_a_terminal_publishes_the_shell() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let elsewhere = TerminalId::new();
        let mut store = Store::new();
        let event = DaemonEvent::TerminalActivity {
            terminal_id: elsewhere,
        };

        let mut first = Batch::default();
        first.absorb(&event, &store, &at, None);
        assert!(first.shell, "the badge went from off to on");

        let _ = store.apply_event(&event);
        let mut second = Batch::default();
        second.absorb(&event, &store, &at, None);
        assert!(!second.shell, "the badge was already on");
    }

    #[test]
    fn a_repeated_bell_from_a_terminal_already_asking_publishes_nothing() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let elsewhere = TerminalId::new();
        let mut store = Store::new();
        let event = DaemonEvent::TerminalBell {
            terminal_id: elsewhere,
        };

        let mut first = Batch::default();
        first.absorb(&event, &store, &at, None);
        assert!(first.shell);

        let _ = store.apply_event(&event);
        let mut second = Batch::default();
        second.absorb(&event, &store, &at, None);
        assert!(!second.shell);
    }

    #[test]
    fn job_output_has_its_own_stream_and_does_not_send_a_shell_snapshot() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(
            &DaemonEvent::JobOutput {
                job_id: JobId::new(),
                from_line: 0,
                lines: vec!["assistant  the answer".to_string()],
            },
            &store,
            &at,
            None,
        );
        assert_eq!(batch.damage, None);
        assert!(!batch.shell);
    }

    #[test]
    fn two_deltas_in_one_batch_merge_their_rows() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(at.terminal, vec![1], 0), &store, &at, None);
        batch.absorb(&delta(at.terminal, vec![2], 0), &store, &at, None);
        assert_eq!(batch.damage, Some(Damage::Rows(vec![1, 2])));
    }

    #[test]
    fn a_resync_for_the_attached_terminal_widens_to_full() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(at.terminal, vec![1], 0), &store, &at, None);
        batch.absorb(
            &DaemonEvent::TerminalResync {
                terminal_id: at.terminal,
                snapshot: domain::TerminalSnapshot {
                    seq: 2,
                    size: DEFAULT_SIZE,
                    visible: Vec::new(),
                    scrollback_tail: Vec::new(),
                    scrollback_len: 0,
                    cursor: Cursor::default(),
                    modes: TermModes::default(),
                    title: None,
                },
            },
            &store,
            &at,
            None,
        );
        assert_eq!(batch.damage, Some(Damage::Full));
    }

    #[test]
    fn losing_the_active_session_is_noticed_before_the_store_forgets_it() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(
            &DaemonEvent::SessionRemoved {
                session_id: at.session,
            },
            &store,
            &at,
            None,
        );
        assert!(batch.active_session_removed);
        assert!(batch.shell);
    }

    fn checkout(project: ProjectId) -> domain::Workspace {
        domain::Workspace {
            id: WorkspaceId::new(),
            project_id: project,
            kind: domain::WorkspaceKind::Main,
            path: std::path::PathBuf::from("/tmp/forge-test"),
            branch: None,
            display_name: None,
            managed_by_app: false,
            created_at: Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        }
    }

    /// `at` is a whole second apart per tab so the strip order under test is
    /// the timestamps and never the sub-millisecond luck of two v7 ids.
    fn tab(workspace: WorkspaceId, at: i64) -> domain::Session {
        let mut session = sample_session(domain::SessionState::Running);
        session.workspace_id = workspace;
        session.terminal_id = Some(TerminalId::new());
        session.created_at = Timestamp::from_unix_secs(at).expect("epoch second");
        session
    }

    fn store_of(workspaces: &[domain::Workspace], sessions: &[domain::Session]) -> Store {
        let mut store = Store::new();
        store.workspaces = workspaces.to_vec();
        store.sessions = sessions.to_vec();
        store
    }

    fn departing(store: &Store, session: &domain::Session) -> Option<Departing> {
        Some(Departing {
            workspace: session.workspace_id,
            project: project_of(store, session.workspace_id),
            key: tab_key(session),
        })
    }

    /// The bug this ordering exists for: the newest live session anywhere is
    /// usually in the project the person just left.
    #[test]
    fn closing_a_tab_lands_on_the_one_before_it_in_the_same_checkout() {
        let here = checkout(ProjectId::new());
        let elsewhere = checkout(ProjectId::new());
        let first = tab(here.id, 100);
        let closing = tab(here.id, 200);
        let newest_anywhere = tab(elsewhere.id, 900);
        // Without the closing session: the loop applies its removal to the
        // store before it asks who takes over.
        let store = store_of(&[here, elsewhere], &[first.clone(), newest_anywhere]);

        assert_eq!(
            successor_session(&store, departing(&store, &closing)),
            Some(first.id)
        );
    }

    #[test]
    fn closing_the_first_tab_lands_on_the_next_one_in_the_same_checkout() {
        let here = checkout(ProjectId::new());
        let elsewhere = checkout(ProjectId::new());
        let closing = tab(here.id, 100);
        let after = tab(here.id, 200);
        let store = store_of(
            &[here, elsewhere.clone()],
            &[after.clone(), tab(elsewhere.id, 900)],
        );

        assert_eq!(
            successor_session(&store, departing(&store, &closing)),
            Some(after.id)
        );
    }

    /// A session with no terminal has already exited; the strip does not show
    /// it, so it cannot be what a close lands on.
    #[test]
    fn an_exited_session_is_not_a_successor() {
        let here = checkout(ProjectId::new());
        let mut exited = tab(here.id, 100);
        exited.terminal_id = None;
        let after = tab(here.id, 300);
        let closing = tab(here.id, 200);
        let store = store_of(&[here], &[exited, after.clone()]);

        assert_eq!(
            successor_session(&store, departing(&store, &closing)),
            Some(after.id)
        );
    }

    /// The strip is empty now, but the rail is still on this project: another
    /// worktree of it is nearer than another repository.
    #[test]
    fn the_last_tab_of_a_checkout_falls_back_to_its_own_project() {
        let project = ProjectId::new();
        let here = checkout(project);
        let sibling = checkout(project);
        let elsewhere = checkout(ProjectId::new());
        let closing = tab(here.id, 200);
        let in_sibling = tab(sibling.id, 100);
        let store = store_of(
            &[here, sibling, elsewhere.clone()],
            &[in_sibling.clone(), tab(elsewhere.id, 900)],
        );

        assert_eq!(
            successor_session(&store, departing(&store, &closing)),
            Some(in_sibling.id)
        );
    }

    /// Removing a worktree broadcasts `SessionRemoved` and `WorkspaceRemoved`
    /// back to back, and the loop drains both before it picks a successor: the
    /// departing checkout's own row is gone by then, so a project resolved at
    /// that point is `None` and the window falls through to another repository.
    #[test]
    fn removing_a_worktree_stays_inside_its_project() {
        let project = ProjectId::new();
        let here = checkout(project);
        let sibling = checkout(project);
        let elsewhere = checkout(ProjectId::new());
        let closing = tab(here.id, 200);
        let in_sibling = tab(sibling.id, 100);
        let newest_anywhere = tab(elsewhere.id, 900);
        let before = store_of(
            &[here, sibling.clone(), elsewhere.clone()],
            &[closing.clone(), in_sibling.clone(), newest_anywhere.clone()],
        );
        let at = attached(TerminalId::new(), closing.id, 0);
        let mut batch = Batch::default();
        batch.absorb(
            &DaemonEvent::SessionRemoved {
                session_id: closing.id,
            },
            &before,
            &at,
            None,
        );

        let after = store_of(
            &[sibling, elsewhere],
            &[in_sibling.clone(), newest_anywhere],
        );
        assert_eq!(
            successor_session(&after, batch.departing),
            Some(in_sibling.id)
        );
    }

    #[test]
    fn a_project_with_nothing_left_open_falls_back_to_the_newest_anywhere() {
        let here = checkout(ProjectId::new());
        let elsewhere = checkout(ProjectId::new());
        let closing = tab(here.id, 200);
        let older = tab(elsewhere.id, 100);
        let newest = tab(elsewhere.id, 900);
        let store = store_of(&[here, elsewhere], &[older, newest.clone()]);

        assert_eq!(
            successor_session(&store, departing(&store, &closing)),
            Some(newest.id)
        );
    }

    /// `FactoryReset` removes the active session without naming it, so there
    /// is no checkout to prefer and the old global answer is the right one.
    #[test]
    fn with_no_departing_checkout_the_newest_session_anywhere_wins() {
        let here = checkout(ProjectId::new());
        let newest = tab(here.id, 900);
        let store = store_of(
            std::slice::from_ref(&here),
            &[tab(here.id, 100), newest.clone()],
        );

        assert_eq!(successor_session(&store, None), Some(newest.id));
        assert_eq!(successor_session(&Store::new(), None), None);
    }

    #[test]
    fn the_departing_checkout_is_read_before_the_store_forgets_it() {
        let here = checkout(ProjectId::new());
        let closing = tab(here.id, 200);
        let store = store_of(std::slice::from_ref(&here), std::slice::from_ref(&closing));
        let at = attached(TerminalId::new(), closing.id, 0);
        let mut batch = Batch::default();

        batch.absorb(
            &DaemonEvent::SessionRemoved {
                session_id: closing.id,
            },
            &store,
            &at,
            None,
        );

        assert_eq!(batch.departing.map(|gone| gone.workspace), Some(here.id));
    }

    #[test]
    fn another_sessions_removal_leaves_the_viewport_alone() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(
            &DaemonEvent::SessionRemoved {
                session_id: SessionId::new(),
            },
            &store,
            &at,
            None,
        );
        assert!(!batch.active_session_removed);
        assert!(batch.shell);
    }

    /// A structured error from the daemon *arrived*, so the socket is alive.
    /// Classifying it as a disconnect is what tore the connection down when a
    /// click landed on a session that had already exited.
    #[test]
    fn a_refusal_from_the_daemon_is_not_a_disconnect() {
        let refused =
            CommandError::from_client(client::ClientError::UnexpectedResponse { expected: "Ack" });
        assert!(matches!(refused, CommandError::Refused(_)));
    }

    #[test]
    fn a_dead_socket_is_a_disconnect() {
        for error in [
            client::ClientError::Disconnected,
            client::ClientError::Timeout,
        ] {
            assert!(
                matches!(
                    CommandError::from_client(error),
                    CommandError::Disconnected(_)
                ),
                "a transport failure must reconnect"
            );
        }
    }

    #[test]
    fn a_refusal_carries_a_reason_a_person_can_read() {
        let error = CommandError::refused("that session has exited");
        assert_eq!(error.to_string(), "that session has exited");
    }

    #[test]
    fn an_unforced_worktree_precondition_opens_the_force_confirmation() {
        let workspace = WorkspaceId::new();
        let error = client::ProtocolError::precondition_failed(
            "cannot remove: running_sessions=false, dirty=true, merge_or_rebase=false",
        );
        let effect =
            remove_worktree_effect(workspace, false, Err(client::ClientError::Protocol(error)))
                .expect("a precondition is a safe second step, not a generic refusal");

        assert_eq!(
            effect.worktree_blocked,
            Some((
                workspace,
                "cannot remove: running_sessions=false, dirty=true, merge_or_rebase=false"
                    .to_string()
            ))
        );
    }

    #[test]
    fn a_forced_worktree_refusal_stays_a_generic_refusal() {
        let error = client::ProtocolError::precondition_failed("still blocked");
        let result = remove_worktree_effect(
            WorkspaceId::new(),
            true,
            Err(client::ClientError::Protocol(error)),
        );

        assert!(matches!(result, Err(CommandError::Refused(_))));
    }

    #[test]
    fn a_worktree_create_refusal_returns_to_its_dialog() {
        let project = ProjectId::new();
        let error = client::ProtocolError::invalid_request("branch is already checked out");
        let effect = create_worktree_effect(
            project,
            "feature".to_string(),
            Err(client::ClientError::Protocol(error)),
        )
        .expect("a domain refusal leaves the connection alive");

        assert_eq!(
            effect.worktree_create_failed,
            Some((
                project,
                "feature".to_string(),
                "branch is already checked out".to_string()
            ))
        );
    }

    #[test]
    fn a_profile_refusal_returns_to_its_editor() {
        let profile = AgentProfileId::new();
        let error = client::ProtocolError::conflict("profile name already exists");
        let effect = save_agent_profile_effect(profile, Err(client::ClientError::Protocol(error)))
            .expect("a profile validation error leaves the connection alive");

        assert_eq!(
            effect.profile_save,
            Some((profile, Some("profile name already exists".to_string())))
        );
    }

    #[test]
    fn a_full_runtime_queue_refuses_another_command() {
        let (commands, receiver) = flume::bounded(1);
        let runtime = Runtime {
            commands,
            latest: Arc::new(Mutex::new(Latest::default())),
            workbench: Arc::new(Mutex::new(None)),
        };
        runtime.send(RuntimeCommand::Reconnect).unwrap();

        assert_eq!(
            runtime.send(RuntimeCommand::Reconnect),
            Err("runtime command queue is full".to_string())
        );
        drop(receiver);
    }

    #[test]
    fn a_shell_effect_also_repaints_because_the_terminal_changed_under_it() {
        let effect = Effect::shell();
        assert!(effect.shell);
        assert_eq!(effect.damage, Some(Damage::Full));
        assert!(Effect::nothing().damage.is_none());
    }

    fn sample_session(state: domain::SessionState) -> domain::Session {
        let id = SessionId::new();
        domain::Session {
            id,
            workspace_id: WorkspaceId::new(),
            kind: domain::SessionKind::Shell,
            role: domain::SessionRole::Generic,
            parent_session_id: None,
            root_session_id: id,
            terminal_id: None,
            agent_provider_id: None,
            agent_profile_id: None,
            title: domain::SessionTitle::default(),
            state,
            created_at: domain::Timestamp::now(),
            launch_command: None,
            last_activity_at: domain::Timestamp::now(),
            ended_at: None,
            base_commit: None,
        }
    }

    #[test]
    fn a_live_session_is_not_due_to_close_until_it_dies() {
        let live = sample_session(domain::SessionState::Running);
        let dead = sample_session(domain::SessionState::Exited {
            code: Some(0),
            signal: None,
        });
        let mut store = Store::new();
        let _ = store.apply_event(&DaemonEvent::SessionCreated(live.clone()));
        let _ = store.apply_event(&DaemonEvent::SessionCreated(dead.clone()));
        let pending = HashSet::from([live.id, dead.id]);

        let due = due_closes(&pending, &store);
        assert_eq!(due, vec![dead.id]);
    }

    #[test]
    fn a_pending_close_for_a_session_already_gone_is_due() {
        let vanished = SessionId::new();
        let store = Store::new();
        let pending = HashSet::from([vanished]);
        assert_eq!(due_closes(&pending, &store), vec![vanished]);
    }
}
