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

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use client::{CellGrid, Client, DaemonEvent, EventOutcome, Store};
use domain::{
    AgentProfileId, AgentProviderId, MouseMode, ProjectId, PtySize, SessionId, TerminalId,
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

const EVENT_BATCH_LIMIT: usize = 64;
const EVENT_BATCH_BUDGET: Duration = Duration::from_millis(2);
const COMMAND_BATCH_LIMIT: usize = 32;

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

/// One pane's output held back by [`cell_send_floor`], merged until it lifts.
#[derive(Default)]
struct FrameFloor {
    held: Option<(Instant, Damage)>,
    last_sent: Option<Instant>,
}

impl FrameFloor {
    /// The damage to publish now, or `None` when it was held.
    ///
    /// `urgent` skips the floor: anything the shell also cares about goes out
    /// at once, so a session appearing is never delayed by terminal output.
    fn offer(&mut self, damage: Damage, floor: Duration, urgent: bool) -> Option<Damage> {
        let within_floor = self.last_sent.is_some_and(|sent| sent.elapsed() < floor);
        if !urgent && within_floor {
            self.held = Some(match self.held.take() {
                Some((since, held)) => (since, held.merge(damage)),
                None => (self.last_sent.unwrap_or_else(Instant::now), damage),
            });
            return None;
        }
        Some(self.release(damage))
    }

    /// Publish `damage` now, with whatever was held folded in.
    fn release(&mut self, damage: Damage) -> Damage {
        self.last_sent = Some(Instant::now());
        match self.held.take() {
            Some((_, held)) => held.merge(damage),
            None => damage,
        }
    }

    /// Held output whose floor has lifted.
    fn due(&mut self, floor: Duration) -> Option<Damage> {
        let (since, _) = self.held.as_ref()?;
        if since.elapsed() < floor {
            return None;
        }
        let (_, damage) = self.held.take()?;
        self.last_sent = Some(Instant::now());
        Some(damage)
    }

    /// How long the loop may block before held output falls due.
    fn wait(&self, floor: Duration) -> Option<Duration> {
        self.held
            .as_ref()
            .map(|(since, _)| floor.saturating_sub(since.elapsed()))
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
        if let RuntimeCommand::PasteTarget {
            connection_generation,
            ..
        } = &command
        {
            let latest = lock(&self.latest);
            if !latest.status.connected
                || latest
                    .payload
                    .as_ref()
                    .is_none_or(|payload| payload.connection_generation != *connection_generation)
            {
                return Err("terminal connection changed".into());
            }
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                flume::TrySendError::Full(_) => "runtime command queue is full".to_string(),
                flume::TrySendError::Disconnected(_) => {
                    "runtime command queue is disconnected".to_string()
                }
            })
    }

    /// Success acknowledges enqueueing; execution reports through workbench events.
    pub fn send_workbench(&self, command: WorkbenchCommand) -> Result<(), String> {
        workbench::enqueue(lock(&self.workbench).as_ref(), command)
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

/// A Code-region editor terminal. Not the main attachment.
///
/// Keyed by `SessionId` in the `editors` map, so the session is the key and
/// not a field: the pane addresses an editor by its session and the bridge
/// resolves the terminal from it.
struct EditorAttachment {
    terminal: TerminalId,
    size: PtySize,
}

/// The extra session terminal in a Cmd+D split. Not the focused attachment.
///
/// The original session stays `Attached` so switching tabs does not resize it
/// to nothing. This column has its own size and scroll, like an editor pane.
struct SplitAttachment {
    session: SessionId,
    terminal: TerminalId,
    size: PtySize,
    scroll_offset: u64,
    /// This pane's own echo ids: each pane's `LatencyProbe` numbers from 1, so
    /// sharing the main pane's counter would settle one pane with the other's.
    pending_echo: u64,
    floor: FrameFloor,
    /// Off screen behind Code or Settings: the replica keeps up, the WebView
    /// is not sent frames nothing would paint.
    parked: bool,
}

impl SplitAttachment {
    fn new(session: SessionId, terminal: TerminalId, size: PtySize) -> Self {
        Self {
            session,
            terminal,
            size,
            scroll_offset: 0,
            pending_echo: 0,
            floor: FrameFloor::default(),
            parked: false,
        }
    }

    fn opened(&self) -> SplitOpened {
        SplitOpened {
            session_id: self.session,
            terminal_id: self.terminal,
        }
    }
}

/// Emits the two event streams and keeps the `connect` command's cached
/// snapshot in step with them.
struct Emitter<'a> {
    connection_generation: u64,
    client: &'a Client,
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
        let snapshot = ShellSnapshot::from_store(store, Some(at.terminal));
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
                connection_generation: self.connection_generation,
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
        self.emit_cells(store, at.terminal, at.scroll_offset, damage, echo_id);
    }

    fn split_cells(&self, store: &mut Store, open: &mut SplitAttachment, damage: &Damage) {
        if open.parked {
            return;
        }
        let echo_id = std::mem::take(&mut open.pending_echo);
        self.emit_cells(store, open.terminal, open.scroll_offset, damage, echo_id);
    }

    fn emit_cells(
        &self,
        store: &mut Store,
        terminal: TerminalId,
        scroll_offset: u64,
        damage: &Damage,
        echo_id: u64,
    ) {
        request_scrollback(self.client, store, terminal, scroll_offset);
        let bell = store
            .terminals
            .get_mut(&terminal)
            .is_some_and(CellGrid::take_bell);
        let Some(grid) = store.terminal(&terminal) else {
            return;
        };
        let _ = self.app.emit(
            "runtime:cells",
            cells::frame(terminal, grid, scroll_offset, damage, bell, echo_id),
        );
    }

    fn editor_cells(&self, store: &mut Store, terminal: TerminalId, damage: &Damage) {
        let Some(grid) = store.terminal(&terminal) else {
            return;
        };
        let _ = self.app.emit(
            "runtime:editor_cells",
            cells::frame(terminal, grid, 0, damage, false, 0),
        );
    }

    /// Publish one DOM editor window.
    ///
    /// Not held back by the cell floor: the editor already coalesced this
    /// against its own emit floor and against the last window it published, so
    /// what arrives here is a frame the surface has not seen.
    fn editor_frame(&self, session_id: SessionId, frame: domain::EditorFrame) {
        let _ = self.app.emit(
            "runtime:editor_frame",
            EditorFramePayload { session_id, frame },
        );
    }
}

/// One window, addressed to the surface showing that session.
#[derive(Clone, Debug, Serialize)]
struct EditorFramePayload {
    session_id: SessionId,
    frame: domain::EditorFrame,
}

#[derive(Clone, Debug, Serialize)]
struct EditorOpened {
    session_id: SessionId,
    terminal_id: TerminalId,
    workspace: WorkspaceId,
    path: String,
}

/// The extra column's session and the terminal its frames name. Sent again
/// when that session moves to a new terminal.
#[derive(Clone, Copy, Debug, Serialize)]
struct SplitOpened {
    session_id: SessionId,
    terminal_id: TerminalId,
}

/// The host let the extra column go on its own: its session was removed or
/// promoted into the main pane.
#[derive(Clone, Copy, Debug, Serialize)]
struct SplitClosed {
    session_id: SessionId,
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
    let mut connection_generation = 0;

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
        connection_generation += 1;
        let mut at = Attached {
            session,
            terminal,
            scroll_offset: 0,
        };
        let mut split: Option<SplitAttachment> = None;
        let mut editors: HashMap<SessionId, EditorAttachment> = HashMap::new();
        // A worker per connection: dropping the previous sender ends the one
        // bound to the client that just died.
        *lock(&workbench) = Some(workbench::start(app.clone(), Arc::clone(&client)));
        preferred_session = Some(at.session);
        let emitter = Emitter {
            connection_generation,
            client: &client,
            app: &app,
            latest: &latest,
            daemon: DaemonInfoDto::from(client.daemon_info()),
        };
        let snapshot = ShellSnapshot::from_store(&store, Some(at.terminal));
        let payload = ConnectedPayload {
            connection_generation,
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
        let mut main_floor = FrameFloor::default();
        // The newest keystroke whose bytes reached the daemon and whose echo
        // has not gone back out yet.
        let mut pending_echo: u64 = 0;
        // Close on a live session only kills; the row is removed once
        // the process is terminal. Without this set the rail keeps a ghost.
        let mut pending_close: HashSet<SessionId> = HashSet::new();
        let mut reconnect = false;

        loop {
            for _ in 0..COMMAND_BATCH_LIMIT {
                let next = match pending_command.take() {
                    Some(command) => Ok(command),
                    None => commands.try_recv(),
                };
                let command = match next {
                    Ok(command) => command,
                    Err(flume::TryRecvError::Empty) => break,
                    Err(flume::TryRecvError::Disconnected) => return,
                };
                if !command_on_connection(&command, connection_generation) {
                    continue;
                }
                match run_command(
                    command,
                    &client,
                    &mut store,
                    &mut at,
                    &mut split,
                    &mut editors,
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
                            let damage = main_floor.release(damage);
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
                        if let Some((terminal, damage)) = effect.editor_damage {
                            emitter.editor_cells(&mut store, terminal, &damage);
                        }
                        if let Some(opened) = effect.editor_opened {
                            let _ = app.emit("runtime:editor_opened", opened);
                        }
                        if let Some(opened) = effect.split_opened {
                            let _ = app.emit("runtime:terminal_split", opened);
                        }
                        if let Some((terminal, damage)) = effect.split_damage {
                            if let Some(open) =
                                split.as_mut().filter(|open| open.terminal == terminal)
                            {
                                let damage = open.floor.release(damage);
                                emitter.split_cells(&mut store, open, &damage);
                            }
                        }
                        if let Some(session) = effect.editor_detached {
                            let _ = app.emit("runtime:editor_detached", session);
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
            if let Some(damage) = main_floor.due(cell_send_floor(at.scroll_offset)) {
                emitter.cells(&mut store, &at, &damage, std::mem::take(&mut pending_echo));
            }
            if let Some(open) = split.as_mut() {
                if let Some(damage) = open.floor.due(cell_send_floor(open.scroll_offset)) {
                    emitter.split_cells(&mut store, open, &damage);
                }
            }

            let wait = [
                main_floor.wait(cell_send_floor(at.scroll_offset)),
                split
                    .as_ref()
                    .and_then(|open| open.floor.wait(cell_send_floor(open.scroll_offset))),
            ]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(LIVENESS_TICK);

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
                batch.absorb_named(&event, &store, &at, split.as_ref());
                emit_side_event(&app, &event);
                emit_clipboard_event(&app, &event, &at, &editors, split.as_ref());
                batch.apply(
                    &event,
                    &mut store,
                    &client,
                    size,
                    at.terminal,
                    split.as_ref(),
                );
                let batch_end = Instant::now() + EVENT_BATCH_BUDGET;
                for _ in 1..EVENT_BATCH_LIMIT {
                    if Instant::now() >= batch_end {
                        break;
                    }
                    let Ok(more) = events.try_recv() else {
                        break;
                    };
                    batch.absorb_named(&more, &store, &at, split.as_ref());
                    emit_side_event(&app, &more);
                    emit_clipboard_event(&app, &more, &at, &editors, split.as_ref());
                    batch.apply(
                        &more,
                        &mut store,
                        &client,
                        size,
                        at.terminal,
                        split.as_ref(),
                    );
                }

                for id in &batch.editor_sessions_removed {
                    editors.remove(id);
                    let _ = app.emit("runtime:editor_detached", *id);
                }

                if batch.split_removed {
                    if let Some(open) = split.take() {
                        if open.terminal != at.terminal {
                            let _ = client.detach_terminal(open.terminal);
                            store.detach_terminal(&open.terminal);
                        }
                        let _ = app.emit(
                            "runtime:split_closed",
                            SplitClosed {
                                session_id: open.session,
                            },
                        );
                    }
                }
                if let Some(terminal) = batch.split_retarget.take() {
                    if let Some(open) = split.as_mut() {
                        if follow_split_terminal(&client, &mut store, open, at.terminal, terminal) {
                            let _ = app.emit("runtime:terminal_split", open.opened());
                            batch.split_damage = Some((open.terminal, Damage::Full));
                        }
                    }
                }

                if batch.active_session_removed {
                    let Some(next_session) = successor_session(&store, batch.departing) else {
                        break;
                    };
                    if split
                        .as_ref()
                        .is_some_and(|open| open.session == next_session)
                    {
                        if let Some(open) = split.take() {
                            // The departed session's grid; `switch_session_keeping`
                            // lets it go on the other branch.
                            if at.terminal != open.terminal {
                                let _ = client.detach_terminal(at.terminal);
                                store.detach_terminal(&at.terminal);
                            }
                            at.session = open.session;
                            at.terminal = open.terminal;
                            at.scroll_offset = open.scroll_offset;
                            size = open.size;
                            pending_echo = 0;
                            main_floor = FrameFloor::default();
                            preferred_session = Some(next_session);
                            batch.shell = true;
                            batch.damage = Some(Damage::Full);
                            // Without this the WebView keeps the column and paints
                            // the promoted session twice, both panes resizing one PTY.
                            let _ = app.emit(
                                "runtime:split_closed",
                                SplitClosed {
                                    session_id: open.session,
                                },
                            );
                        }
                    } else {
                        match switch_session_keeping(
                            &client,
                            &mut store,
                            at.terminal,
                            next_session,
                            size,
                            split.as_ref().map(|open| open.terminal),
                        ) {
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
                }

                flush_pending_closes(&mut pending_close, &store, &client);

                if batch.shell {
                    emitter.shell(&store, &at);
                }
                for (terminal, damage) in batch.editor_damage {
                    emitter.editor_cells(&mut store, terminal, &damage);
                }
                if let Some((terminal, damage)) = batch.split_damage {
                    if let Some(open) = split.as_mut().filter(|open| open.terminal == terminal) {
                        let floor = cell_send_floor(open.scroll_offset);
                        if let Some(damage) = open.floor.offer(damage, floor, batch.shell) {
                            emitter.split_cells(&mut store, open, &damage);
                        }
                    }
                }
                for (session_id, frame) in batch.editor_frames {
                    emitter.editor_frame(session_id, frame);
                }
                if let Some(damage) = batch.damage {
                    let floor = cell_send_floor(at.scroll_offset);
                    if let Some(damage) = main_floor.offer(damage, floor, batch.shell) {
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

/// Forward an OSC 52 clipboard store, but only from a terminal on screen.
///
/// The guard is the point: OSC 52 lets whatever runs in a PTY set the person's
/// clipboard, and a background agent quietly replacing it would be a real
/// hazard. A terminal this window is attached to — the focused one, or an open
/// editor pane — is one the person is looking at; everything else is refused.
fn emit_clipboard_event(
    app: &AppHandle,
    event: &DaemonEvent,
    at: &Attached,
    editors: &HashMap<SessionId, EditorAttachment>,
    split: Option<&SplitAttachment>,
) {
    let DaemonEvent::ClipboardStore { terminal_id, text } = event else {
        return;
    };
    if !clipboard_is_allowed(*terminal_id, at, editors, split) {
        return;
    }
    let _ = app.emit("runtime:clipboard", ClipboardPayload { text: text.clone() });
}

/// Whether a terminal may set the clipboard: only one this window shows.
///
/// Its own function so the rule can be tested without an `AppHandle` — it is
/// the security property here, not the emit.
fn clipboard_is_allowed(
    terminal_id: TerminalId,
    at: &Attached,
    editors: &HashMap<SessionId, EditorAttachment>,
    split: Option<&SplitAttachment>,
) -> bool {
    terminal_id == at.terminal
        || split.is_some_and(|open| open.terminal == terminal_id)
        || editors.values().any(|open| open.terminal == terminal_id)
}

fn emit_side_event(app: &AppHandle, event: &DaemonEvent) {
    match event {
        DaemonEvent::FileChanged { workspace_id, path } => {
            let _ = app.emit("workbench:file_changed", &(*workspace_id, path));
        }
        // Provisioning acks when it starts, so this is where the GUI learns
        // what a worktree actually got. It is not shell state — the
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
    /// A side editor terminal has to repaint.
    editor_damage: Option<(TerminalId, Damage)>,
    /// A new editor session was spawned; the WebView opens its Code view.
    editor_opened: Option<EditorOpened>,
    /// A new shell occupies the extra column; the WebView mounts its pane.
    split_opened: Option<SplitOpened>,
    /// The extra column's grid, keyed so it does not merge with the main pane.
    split_damage: Option<(TerminalId, Damage)>,
    /// The Code pane let an editor go; the process is still running.
    editor_detached: Option<SessionId>,
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
    /// review. The daemon refuses a provider that declares no such
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
/// Close on a live session only kills. The daemon refuses `CloseSession`
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

fn command_on_connection(command: &RuntimeCommand, generation: u64) -> bool {
    match command {
        RuntimeCommand::PasteTarget {
            connection_generation,
            ..
        } => *connection_generation == generation,
        _ => true,
    }
}

fn paste_target_matches(
    store: &Store,
    at: &Attached,
    split: Option<&SplitAttachment>,
    session: SessionId,
    terminal: TerminalId,
) -> bool {
    let on_screen = (at.session == session && at.terminal == terminal)
        || split.is_some_and(|open| open.session == session && open.terminal == terminal);
    on_screen
        && store.sessions.iter().any(|item| {
            item.id == session && item.terminal_id == Some(terminal) && item.state.is_active()
        })
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
///
/// The arguments are the whole of one connection's mutable state — the main
/// attachment, the split column, the editor attachments, the viewport size and
/// the echo watermark. Bundling them into a struct would only move the same fields
/// behind a name and make every borrow in the loop go through it.
#[allow(clippy::too_many_arguments)]
fn run_command(
    command: RuntimeCommand,
    client: &Client,
    store: &mut Store,
    at: &mut Attached,
    split: &mut Option<SplitAttachment>,
    editors: &mut HashMap<SessionId, EditorAttachment>,
    size: &mut PtySize,
    pending_echo: &mut u64,
) -> Result<Effect, CommandError> {
    match command {
        RuntimeCommand::Input {
            key,
            id,
            session_id,
        } => {
            let Some(pane) = pane(at, pending_echo, split, session_id) else {
                return Ok(Effect::nothing());
            };
            let modes = store.terminal(&pane.terminal).map(|grid| grid.modes);
            let Some(bytes) = input::encode_terminal(&key, &modes.unwrap_or_default()) else {
                return Ok(Effect::nothing());
            };
            write_input(client, store, pane, bytes, id)
        }
        RuntimeCommand::InputText {
            text,
            id,
            session_id,
        } => match pane(at, pending_echo, split, session_id) {
            Some(pane) => write_input(client, store, pane, input::encode_text(&text), id),
            None => Ok(Effect::nothing()),
        },
        RuntimeCommand::MoveCursor {
            terminal_id,
            seq,
            row,
            col,
            id,
            session_id,
        } => {
            let Some(pane) = pane(at, pending_echo, split, session_id)
                .filter(|pane| pane.terminal == terminal_id && *pane.scroll_offset == 0)
            else {
                return Ok(Effect::nothing());
            };
            let Some(grid) = store
                .terminal(&terminal_id)
                .filter(|grid| grid.last_seq == seq)
            else {
                return Ok(Effect::nothing());
            };
            let Some(bytes) = input::encode_cursor_move(grid, row, col) else {
                return Ok(Effect::nothing());
            };
            write_input(client, store, pane, bytes, id)
        }
        RuntimeCommand::Paste {
            text,
            id,
            session_id,
        } => {
            let Some(pane) = pane(at, pending_echo, split, session_id) else {
                return Ok(Effect::nothing());
            };
            let modes = store
                .terminal(&pane.terminal)
                .map(|grid| grid.modes)
                .unwrap_or_default();
            let bytes = input::encode_paste(&text, &modes);
            write_input(client, store, pane, bytes, id)
        }
        RuntimeCommand::PasteTarget {
            session_id,
            terminal_id,
            text,
            ..
        } => {
            if !paste_target_matches(store, at, split.as_ref(), session_id, terminal_id) {
                return Err(CommandError::refused(
                    "The terminal destination changed; reference was not inserted.",
                ));
            }
            let Some(grid) = store.terminal(&terminal_id) else {
                return Err(CommandError::refused(
                    "The terminal destination is no longer attached.",
                ));
            };
            let bytes = input::encode_paste(&text, &grid.modes);
            let Some(pane) = pane(at, pending_echo, split, Some(session_id)) else {
                return Err(CommandError::refused(
                    "The terminal destination changed; reference was not inserted.",
                ));
            };
            write_input(client, store, pane, bytes, 0)
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
            session_id,
        } => {
            let Some(pane) = pane(at, pending_echo, split, session_id) else {
                return Ok(Effect::nothing());
            };
            let terminal = pane.terminal;
            let modes = store
                .terminal(&terminal)
                .map(|grid| grid.modes)
                .unwrap_or_default();
            let mods = client::Modifiers { ctrl, alt, shift };
            let Some(bytes) = input::encode_mouse_event(&button, &kind, col, row, mods, &modes)
            else {
                return Ok(Effect::nothing());
            };
            client
                .write_terminal_input(terminal, bytes)
                .map_err(CommandError::from_client)?;
            // A TUI that reads the mouse can be answered with a click, so a
            // button press spends the attention mark the way typing does.
            // No echo id and no scroll snap: a mouse report is not typing, and
            // yanking the viewport to the live output under a program that is
            // painting its own scrollback would fight it.
            if mouse_answers(&button, &kind) && store.answer_attention(&terminal) {
                return Ok(pane.route(Effect::shell()));
            }
            Ok(Effect::nothing())
        }
        RuntimeCommand::Scroll { lines, session_id } => {
            Ok(match pane(at, pending_echo, split, session_id) {
                Some(pane) => {
                    let effect = scroll(client, store, pane.terminal, pane.scroll_offset, lines);
                    pane.route(effect)
                }
                None => Effect::nothing(),
            })
        }
        RuntimeCommand::ScrollToBottom { session_id } => {
            Ok(match pane(at, pending_echo, split, session_id) {
                Some(pane) if *pane.scroll_offset != 0 => {
                    *pane.scroll_offset = 0;
                    pane.route(Effect::repaint())
                }
                _ => Effect::nothing(),
            })
        }
        RuntimeCommand::Repaint { session_id } => {
            Ok(match pane(at, pending_echo, split, session_id) {
                Some(pane) => pane.route(Effect::repaint()),
                None => Effect::nothing(),
            })
        }
        RuntimeCommand::CopySelection {
            anchor_line,
            anchor_col,
            head_line,
            head_col,
            session_id,
        } => {
            let Some(pane) = pane(at, pending_echo, split, session_id) else {
                return Ok(Effect::nothing());
            };
            let text = store
                .terminal(&pane.terminal)
                .map(|grid| {
                    cells::selection_text(grid, (anchor_line, anchor_col), (head_line, head_col))
                })
                .transpose()
                .map_err(CommandError::refused)?;
            Ok(Effect {
                clipboard: text,
                ..Effect::nothing()
            })
        }
        RuntimeCommand::SelectSession { session_id } => {
            if session_id == at.session {
                return Ok(Effect::nothing());
            }
            if split
                .as_ref()
                .is_some_and(|open| open.session == session_id)
            {
                return Ok(Effect::nothing());
            }
            let terminal = switch_session_keeping(
                client,
                store,
                at.terminal,
                session_id,
                *size,
                split.as_ref().map(|open| open.terminal),
            )?;
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
        RuntimeCommand::SplitShell { workspace } => {
            if split.is_some() {
                return Ok(Effect::nothing());
            }
            let (session_id, terminal_id) = spawn_shell(client, store, at.terminal, workspace)?;
            let snapshot = client
                .attach_terminal(terminal_id, *size)
                .map_err(CommandError::from_client)?;
            store.attach_terminal(terminal_id, &snapshot);
            let open = SplitAttachment::new(session_id, terminal_id, *size);
            let opened = open.opened();
            *split = Some(open);
            Ok(Effect {
                shell: true,
                split_opened: Some(opened),
                split_damage: Some((terminal_id, Damage::Full)),
                ..Effect::nothing()
            })
        }
        RuntimeCommand::ParkSplit { session_id, parked } => {
            let Some(open) = split
                .as_mut()
                .filter(|open| open.session == session_id && open.parked != parked)
            else {
                return Ok(Effect::nothing());
            };
            open.parked = parked;
            Ok(Effect {
                split_damage: (!parked).then_some((open.terminal, Damage::Full)),
                ..Effect::nothing()
            })
        }
        RuntimeCommand::DetachSplit { session_id } => {
            let Some(open) = split.take() else {
                return Ok(Effect::nothing());
            };
            if open.session != session_id {
                *split = Some(open);
                return Ok(Effect::nothing());
            }
            if open.terminal != at.terminal {
                let _ = client.detach_terminal(open.terminal);
                store.detach_terminal(&open.terminal);
            }
            Ok(Effect::nothing())
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
        RuntimeCommand::CreateChildSession {
            parent,
            provider,
            profile,
            prompt,
            role,
            workspace_policy,
            branch_hint,
        } => {
            let (session, terminal) = client
                .create_child_session(
                    parent,
                    domain::SessionKind::Agent,
                    Some(provider),
                    profile,
                    parse_session_role(role.as_deref()),
                    parse_workspace_policy(workspace_policy.as_deref(), branch_hint),
                    prompt,
                )
                .map_err(CommandError::from_client)?;
            let _ = client.detach_terminal(at.terminal);
            store.detach_terminal(&at.terminal);
            let snapshot = client
                .attach_terminal(terminal, *size)
                .map_err(CommandError::from_client)?;
            store.attach_terminal(terminal, &snapshot);
            at.session = session;
            at.terminal = terminal;
            at.scroll_offset = 0;
            Ok(Effect::shell())
        }
        RuntimeCommand::SendContext {
            source,
            target,
            spawn_provider,
            profile,
            summary,
            instructions,
            include_transcript,
            role,
            workspace_policy,
            branch_hint,
        } => {
            let spawn = spawn_provider.map(|provider| client::SendContextSpawn {
                kind: domain::SessionKind::Agent,
                provider_id: Some(provider),
                profile_id: profile,
                role: parse_session_role(role.as_deref()),
                workspace_policy: parse_workspace_policy(workspace_policy.as_deref(), branch_hint),
            });
            match client
                .send_context(
                    source,
                    target,
                    spawn,
                    summary,
                    instructions,
                    include_transcript,
                    None,
                )
                .map_err(CommandError::from_client)?
            {
                client::SendContextResult::Delivered => Ok(Effect::nothing()),
                client::SendContextResult::Spawned {
                    session_id,
                    terminal_id,
                } => {
                    let _ = client.detach_terminal(at.terminal);
                    store.detach_terminal(&at.terminal);
                    let snapshot = client
                        .attach_terminal(terminal_id, *size)
                        .map_err(CommandError::from_client)?;
                    store.attach_terminal(terminal_id, &snapshot);
                    at.session = session_id;
                    at.terminal = terminal_id;
                    at.scroll_offset = 0;
                    Ok(Effect::shell())
                }
            }
        }
        RuntimeCommand::Resize {
            size: requested,
            session_id,
        } => {
            let requested = requested.sanitized();
            if let Some(open) = split
                .as_mut()
                .filter(|open| session_id == Some(open.session))
            {
                if requested == open.size {
                    return Ok(Effect::nothing());
                }
                resize_and_reattach(client, store, open.terminal, requested)?;
                open.size = requested;
                open.scroll_offset = 0;
                return Ok(Effect {
                    split_damage: Some((open.terminal, Damage::Full)),
                    ..Effect::nothing()
                });
            }
            if session_id.is_some() && session_id != Some(at.session) {
                return Ok(Effect::nothing());
            }
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
            if split
                .as_ref()
                .is_some_and(|open| open.session == session_id)
            {
                let split_size = split.as_ref().map(|open| open.size).unwrap_or(*size);
                match attach_session(client, store, session_id, split_size) {
                    Ok(terminal) => {
                        let opened = split.as_mut().map(|open| {
                            open.terminal = terminal;
                            open.scroll_offset = 0;
                            open.pending_echo = 0;
                            open.opened()
                        });
                        return Ok(Effect {
                            forget_pending_close: Some(session_id),
                            shell: true,
                            split_opened: opened,
                            split_damage: Some((terminal, Damage::Full)),
                            ..Effect::nothing()
                        });
                    }
                    Err(CommandError::Refused(_)) => {
                        return Ok(Effect {
                            forget_pending_close: Some(session_id),
                            ..Effect::shell()
                        });
                    }
                    Err(error) => return Err(error),
                }
            }
            match switch_session_keeping(
                client,
                store,
                at.terminal,
                session_id,
                *size,
                split.as_ref().map(|open| open.terminal),
            ) {
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
        RuntimeCommand::SetWorktreeIgnores { project, rules } => {
            client
                .set_worktree_ignores(project, rules)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::SetProviderExecutable { provider, path } => {
            client
                .set_provider_executable(&provider, path.map(std::path::PathBuf::from))
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }

        RuntimeCommand::OpenEditor {
            workspace,
            path,
            line,
            autosave,
        } => open_editor(
            client,
            store,
            at,
            editors,
            EditorOpen {
                workspace,
                path,
                line,
                autosave,
            },
        ),
        RuntimeCommand::ReopenEditor { session_id } => {
            let terminal_id = attach_editor(client, store, editors, session_id)?;
            let session = store
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .ok_or_else(|| CommandError::refused("the editor session is no longer running"))?;
            let path = session
                .editor
                .as_ref()
                .map(|state| state.path.clone())
                .ok_or_else(|| CommandError::refused("the editor buffer has not opened yet"))?;
            Ok(Effect {
                editor_opened: Some(EditorOpened {
                    session_id,
                    terminal_id,
                    workspace: session.workspace_id,
                    path,
                }),
                editor_damage: Some((terminal_id, Damage::Full)),
                shell: true,
                ..Effect::nothing()
            })
        }
        RuntimeCommand::InputEditor {
            session_id,
            key,
            id,
        } => input_editor(client, store, editors, pending_echo, session_id, key, id),
        RuntimeCommand::InputEditorText {
            session_id,
            text,
            id,
        } => input_editor_bytes(
            client,
            editors,
            pending_echo,
            session_id,
            input::encode_text(&text),
            id,
        ),
        RuntimeCommand::PasteEditor {
            session_id,
            text,
            id,
        } => {
            let Some(open) = editors.get(&session_id) else {
                return Ok(Effect::nothing());
            };
            let modes = store
                .terminal(&open.terminal)
                .map(|grid| grid.modes)
                .unwrap_or_default();
            let (bytes, clamped) = input::encode_editor_paste(&text, &modes);
            if clamped {
                tracing::warn!(
                    bytes = text.len(),
                    limit = input::MAX_EDITOR_PASTE_BYTES,
                    "editor paste clamped to the document budget"
                );
            }
            input_editor_bytes(client, editors, pending_echo, session_id, bytes, id)
        }
        // The editor sibling of `Mouse`: the pane only sends these while the
        // editor asked to read the mouse, and the encoder refuses whatever the
        // active mode does not report, so an event that encodes to nothing is
        // simply one this mode does not want.
        RuntimeCommand::MouseEditor {
            session_id,
            button,
            kind,
            col,
            row,
            ctrl,
            alt,
            shift,
        } => {
            let Some(open) = editors.get(&session_id) else {
                return Ok(Effect::nothing());
            };
            let modes = store
                .terminal(&open.terminal)
                .map(|grid| grid.modes)
                .unwrap_or_default();
            let mods = client::Modifiers { ctrl, alt, shift };
            let Some(bytes) = input::encode_mouse_event(&button, &kind, col, row, mods, &modes)
            else {
                return Ok(Effect::nothing());
            };
            client
                .write_terminal_input(open.terminal, bytes)
                .map_err(CommandError::from_client)?;
            // No echo id: a mouse report is not typing, so there is no latency
            // sample to close.
            Ok(Effect::nothing())
        }
        RuntimeCommand::ResizeEditor { session_id, size } => {
            resize_editor(client, store, editors, session_id, size)
        }
        RuntimeCommand::EditorSurfaceInput { session_id, events } => {
            client
                .send_editor_input(session_id, events)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        // A refused view is not worth reporting: the surface's next scroll
        // frame replaces it, and a saturated queue means one is already on its
        // way.
        RuntimeCommand::EditorSurfaceView {
            session_id,
            first_line,
            line_count,
        } => {
            client
                .set_editor_view(session_id, first_line, line_count)
                .map_err(CommandError::from_client)?;
            Ok(Effect::nothing())
        }
        RuntimeCommand::RepaintEditor { session_id } => {
            let terminal = attach_editor(client, store, editors, session_id)?;
            Ok(Effect {
                editor_damage: Some((terminal, Damage::Full)),
                ..Effect::nothing()
            })
        }
        RuntimeCommand::CloseEditor { session_id } => {
            Ok(detach_editor(client, editors, at.terminal, session_id))
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

fn attach_editor(
    client: &Client,
    store: &mut Store,
    editors: &mut HashMap<SessionId, EditorAttachment>,
    session_id: SessionId,
) -> Result<TerminalId, CommandError> {
    if let Some(open) = editors.get(&session_id) {
        return Ok(open.terminal);
    }
    let terminal = store
        .sessions
        .iter()
        .find(|session| {
            session.id == session_id
                && session.kind == domain::SessionKind::Editor
                && !session.state.is_terminal()
        })
        .and_then(|session| session.terminal_id)
        .ok_or_else(|| CommandError::refused("the editor session is no longer running"))?;
    let size = DEFAULT_SIZE;
    let snapshot = client
        .attach_terminal(terminal, size)
        .map_err(CommandError::from_client)?;
    store.attach_terminal(terminal, &snapshot);
    editors.insert(session_id, EditorAttachment { terminal, size });
    Ok(terminal)
}

fn open_editor(
    client: &Client,
    store: &mut Store,
    at: &Attached,
    editors: &mut HashMap<SessionId, EditorAttachment>,
    open: EditorOpen,
) -> Result<Effect, CommandError> {
    let EditorOpen {
        workspace,
        path,
        line,
        autosave,
    } = open;
    // A second open of the same file moves the caret in the session that
    // already has it: a rival editor would be a second process, a second PTY
    // and a second draft of one file. The same rule `editorReveal` follows for
    // the DOM editor, one layer down.
    if let Some((session_id, terminal_id)) = live_editor_for(store, editors, workspace, &path) {
        // A closed view detached the terminal, not the editor: the process
        // still holds the draft. Re-attach rather than open the disk state
        // under it, which would strand the draft in a process nothing shows.
        attach_editor(client, store, editors, session_id)?;
        client
            .set_editor_autosave(session_id, autosave)
            .map_err(CommandError::from_client)?;
        if let Some(line) = line {
            client
                .reveal_in_editor_session(session_id, line, None)
                .map_err(CommandError::from_client)?;
        }
        return Ok(Effect {
            editor_opened: Some(EditorOpened {
                session_id,
                terminal_id,
                workspace,
                path,
            }),
            editor_damage: Some((terminal_id, Damage::Full)),
            shell: true,
            ..Effect::nothing()
        });
    }

    let (session_id, terminal_id) = client
        .create_editor_session(workspace, &path, line, false, autosave)
        .map_err(CommandError::from_client)?;
    if terminal_id == at.terminal {
        return Err(CommandError::refused(
            "the editor session must not replace the main terminal",
        ));
    }
    let size = DEFAULT_SIZE;
    let snapshot = client
        .attach_terminal(terminal_id, size)
        .map_err(CommandError::from_client)?;
    store.attach_terminal(terminal_id, &snapshot);
    editors.insert(
        session_id,
        EditorAttachment {
            terminal: terminal_id,
            size,
        },
    );
    Ok(Effect {
        editor_opened: Some(EditorOpened {
            session_id,
            terminal_id,
            workspace,
            path,
        }),
        editor_damage: Some((terminal_id, Damage::Full)),
        shell: true,
        ..Effect::nothing()
    })
}

/// What one `OpenEditor` names. Grouped so the call is not eight positionals.
struct EditorOpen {
    workspace: WorkspaceId,
    path: String,
    line: Option<u32>,
    autosave: bool,
}

/// The editor session already holding `path` in `workspace`, if one is live.
///
/// Matched on the daemon's own `EditorState.path` — what the editor reported
/// through its control channel — and never on this window's attachment map
/// alone: a closed view detaches without stopping the editor, and the draft it
/// still holds is exactly what reopening the file must show.
fn live_editor_for(
    store: &Store,
    editors: &HashMap<SessionId, EditorAttachment>,
    workspace: WorkspaceId,
    path: &str,
) -> Option<(SessionId, TerminalId)> {
    store
        .sessions
        .iter()
        .filter(|session| {
            session.kind == domain::SessionKind::Editor
                && session.workspace_id == workspace
                && !session.state.is_terminal()
        })
        .find(|session| {
            session
                .editor
                .as_ref()
                .is_some_and(|state| state.path == path)
        })
        .and_then(|session| {
            let terminal = editors
                .get(&session.id)
                .map(|open| open.terminal)
                .or(session.terminal_id)?;
            Some((session.id, terminal))
        })
}

/// Which terminal an editor keystroke is written to, and the bytes for it.
///
/// The editor's own terminal, never the main attachment: the pane is a side
/// attachment, so typing into it must not reach the session the shell is on.
/// `None` for a session with no editor open and for a key that is not input
/// (a bare modifier, a composition placeholder).
fn editor_input_target(
    store: &Store,
    editors: &HashMap<SessionId, EditorAttachment>,
    session_id: SessionId,
    key: &super::input::KeyPress,
) -> Option<(TerminalId, Vec<u8>)> {
    let open = editors.get(&session_id)?;
    let modes = store.terminal(&open.terminal).map(|grid| grid.modes);
    let bytes = input::encode(key, &modes.unwrap_or_default())?;
    Some((open.terminal, bytes))
}

fn input_editor(
    client: &Client,
    store: &Store,
    editors: &HashMap<SessionId, EditorAttachment>,
    pending_echo: &mut u64,
    session_id: SessionId,
    key: super::input::KeyPress,
    id: u64,
) -> Result<Effect, CommandError> {
    let Some((_, bytes)) = editor_input_target(store, editors, session_id, &key) else {
        return Ok(Effect::nothing());
    };
    input_editor_bytes(client, editors, pending_echo, session_id, bytes, id)
}

fn input_editor_bytes(
    client: &Client,
    editors: &HashMap<SessionId, EditorAttachment>,
    pending_echo: &mut u64,
    session_id: SessionId,
    bytes: Vec<u8>,
    id: u64,
) -> Result<Effect, CommandError> {
    let Some(open) = editors.get(&session_id) else {
        return Ok(Effect::nothing());
    };
    if bytes.is_empty() {
        return Ok(Effect::nothing());
    }
    client
        .write_terminal_input(open.terminal, bytes)
        .map_err(CommandError::from_client)?;
    *pending_echo = (*pending_echo).max(id);
    Ok(Effect::nothing())
}

/// The terminal to re-attach at `requested`, recording the new size, or `None`
/// when nothing has to move.
///
/// A pane measures itself on every animation frame of a drag and sends what it
/// measured; re-attaching for a size the terminal already has would put a
/// round trip and a full repaint on that rung (`docs/performance.md`), so an
/// unchanged size is not a resize.
fn editor_resize_target(
    editors: &mut HashMap<SessionId, EditorAttachment>,
    session_id: SessionId,
    requested: PtySize,
) -> Option<TerminalId> {
    let open = editors.get_mut(&session_id)?;
    if open.size == requested {
        return None;
    }
    open.size = requested;
    Some(open.terminal)
}

fn resize_editor(
    client: &Client,
    store: &mut Store,
    editors: &mut HashMap<SessionId, EditorAttachment>,
    session_id: SessionId,
    requested: PtySize,
) -> Result<Effect, CommandError> {
    let requested = requested.sanitized();
    let Some(terminal) = editor_resize_target(editors, session_id, requested) else {
        return Ok(Effect::nothing());
    };
    // A re-attach would not resize: the daemon adopts an attach size only for
    // the first subscriber, and this connection is already one. Resize first,
    // like the main terminal does (`resize_and_reattach`), or the editor keeps
    // the geometry it was born with.
    resize_and_reattach(client, store, terminal, requested)?;
    Ok(Effect {
        editor_damage: Some((terminal, Damage::Full)),
        ..Effect::nothing()
    })
}

fn detach_editor(
    client: &Client,
    editors: &mut HashMap<SessionId, EditorAttachment>,
    main: TerminalId,
    session_id: SessionId,
) -> Effect {
    let Some(open) = editors.remove(&session_id) else {
        return Effect::nothing();
    };
    if open.terminal != main {
        if let Err(error) = client.detach_terminal(open.terminal) {
            tracing::warn!(%error, "failed to detach the editor terminal");
        }
    }
    Effect {
        editor_detached: Some(session_id),
        ..Effect::nothing()
    }
}

/// A terminal pane on screen, resolved from the session a command names.
struct Pane<'a> {
    terminal: TerminalId,
    scroll_offset: &'a mut u64,
    /// Each pane's `LatencyProbe` numbers from 1, so each pane settles its own.
    pending_echo: &'a mut u64,
    split: bool,
}

impl Pane<'_> {
    /// Address an effect's repaint to this pane's own damage slot.
    fn route(&self, effect: Effect) -> Effect {
        if !self.split {
            return effect;
        }
        Effect {
            split_damage: effect.damage.map(|damage| (self.terminal, damage)),
            damage: None,
            ..effect
        }
    }
}

/// The pane `session` names; `None` names the window's attachment.
///
/// A session in neither pane resolves to nothing: it is a column the host
/// already let go and the WebView has not heard about yet, and falling back
/// to the main pane would type into, or copy from, a terminal nobody aimed at.
fn pane<'a>(
    at: &'a mut Attached,
    pending_echo: &'a mut u64,
    split: &'a mut Option<SplitAttachment>,
    session: Option<SessionId>,
) -> Option<Pane<'a>> {
    if let Some(open) = split.as_mut().filter(|open| session == Some(open.session)) {
        return Some(Pane {
            terminal: open.terminal,
            scroll_offset: &mut open.scroll_offset,
            pending_echo: &mut open.pending_echo,
            split: true,
        });
    }
    if session.is_some_and(|id| id != at.session) {
        return None;
    }
    Some(Pane {
        terminal: at.terminal,
        scroll_offset: &mut at.scroll_offset,
        pending_echo,
        split: false,
    })
}

/// Write encoded bytes to a terminal this window is showing.
///
/// Typing snaps the viewport back to the live output: input that lands
/// somewhere the user cannot see is the worst outcome of a scrolled viewport.
fn write_input(
    client: &Client,
    store: &mut Store,
    pane: Pane<'_>,
    bytes: Vec<u8>,
    id: u64,
) -> Result<Effect, CommandError> {
    if bytes.is_empty() {
        return Ok(Effect::nothing());
    }
    client
        .write_terminal_input(pane.terminal, bytes)
        .map_err(CommandError::from_client)?;
    *pane.pending_echo = (*pane.pending_echo).max(id);
    // Typing is how a permission prompt gets answered, so it is what spends the
    // attention mark. `answer_attention` reports the edge, and only the edge
    // republishes the shell: a keystroke must never reach `from_store`.
    if store.answer_attention(&pane.terminal) {
        *pane.scroll_offset = 0;
        return Ok(pane.route(Effect::shell()));
    }
    if *pane.scroll_offset != 0 {
        *pane.scroll_offset = 0;
        return Ok(pane.route(Effect::repaint()));
    }
    Ok(Effect::nothing())
}

/// Move the viewport through history, fetching the rows it is about to paint.
///
/// Refused on the alternate screen and while a program is reading the mouse:
/// a full-screen TUI scrolls itself, and stealing the wheel from it would
/// scroll our replica while the program under it stayed put.
fn scroll(
    client: &Client,
    store: &mut Store,
    terminal: TerminalId,
    scroll_offset: &mut u64,
    lines: i64,
) -> Effect {
    let Some(grid) = store.terminal(&terminal) else {
        return Effect::nothing();
    };
    if grid.modes.alt_screen || grid.modes.mouse_mode != MouseMode::Off {
        return Effect::nothing();
    }
    let limit = grid.scrollback_len as i64;
    let next = (*scroll_offset as i64 + lines).clamp(0, limit) as u64;
    if next == *scroll_offset {
        return Effect::nothing();
    }
    *scroll_offset = next;
    request_scrollback(client, store, terminal, *scroll_offset);
    Effect::repaint()
}

/// Ask the daemon for the history the viewport is about to paint, if the cache
/// does not already hold it.
///
/// The request starts half a page earlier than strictly needed, so scrolling
/// steadily in one direction keeps hitting the cache instead of stalling on a
/// round trip at every page boundary.
fn request_scrollback(
    client: &Client,
    store: &mut Store,
    terminal: TerminalId,
    scroll_offset: u64,
) {
    let Some(grid) = store.terminal(&terminal) else {
        return;
    };
    let offset = scroll_offset.min(grid.scrollback_len);
    if cells::viewport_is_cached(grid, offset) {
        return;
    }
    let Some(oldest) = cells::oldest_needed_line(grid, offset) else {
        return;
    };
    let from_line = (oldest - i64::from(cells::SCROLLBACK_PAGE) / 2).max(0);
    match client.fetch_scrollback(terminal, from_line, cells::SCROLLBACK_PAGE) {
        Ok(block) => store.merge_scrollback(&terminal, &block),
        Err(error) => tracing::warn!(%error, "failed to fetch scrollback"),
    }
}

/// One drained batch of daemon events, and what it asks the loop to publish.
#[derive(Default)]
struct Batch {
    shell: bool,
    damage: Option<Damage>,
    split_damage: Option<(TerminalId, Damage)>,
    split_removed: bool,
    /// The split's session reported a terminal the column is not attached to:
    /// a restart minted one, possibly after the restart command's own attach
    /// read the replica before the new id reached it.
    split_retarget: Option<TerminalId>,
    active_session_removed: bool,
    /// Where the session that just went was, read while the store still had
    /// the row: `successor_session` needs its checkout to stay in it.
    departing: Option<Departing>,
    /// Side editor terminals that have to repaint, keyed so two editors
    /// in the Code strip do not merge their damage.
    editor_damage: HashMap<TerminalId, Damage>,
    /// The newest window each DOM editor surface published in this batch.
    ///
    /// Last wins rather than merged: a frame *is* the whole window, so an
    /// older one has nothing the newer one is missing.
    editor_frames: HashMap<SessionId, domain::EditorFrame>,
    editor_sessions_removed: Vec<SessionId>,
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
    #[cfg(test)]
    fn absorb(&mut self, event: &DaemonEvent, store: &Store, at: &Attached) {
        self.absorb_named(event, store, at, None);
    }

    /// Read what an event means for the shell and the viewport, *before* it is
    /// applied: a delta names the rows it damages, and the store does not keep
    /// them.
    fn absorb_named(
        &mut self,
        event: &DaemonEvent,
        store: &Store,
        at: &Attached,
        split: Option<&SplitAttachment>,
    ) {
        if matches!(event, DaemonEvent::FactoryReset) {
            self.active_session_removed = true;
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
            if split.is_some_and(|open| open.session == *session_id) {
                self.split_removed = true;
            }
            if store.sessions.iter().any(|session| {
                session.id == *session_id && session.kind == domain::SessionKind::Editor
            }) {
                self.editor_sessions_removed.push(*session_id);
            }
        }
        if let (DaemonEvent::SessionUpdated(session), Some(open)) = (event, split) {
            if session.id == open.session {
                if let Some(terminal) = session.terminal_id.filter(|id| *id != open.terminal) {
                    self.split_retarget = Some(terminal);
                }
            }
        }
        if let DaemonEvent::EditorFrame { session_id, frame } = event {
            // A clone here is a refcount bump: the rows are the only part that
            // scales with the window and they ship behind an `Arc`.
            self.editor_frames.insert(*session_id, frame.clone());
        }
        let split_terminal = split.map(|open| open.terminal);
        let damage = match event {
            DaemonEvent::TerminalDelta { terminal_id, delta } if *terminal_id == at.terminal => {
                Some(delta_damage(delta))
            }
            DaemonEvent::TerminalResync { terminal_id, .. } if *terminal_id == at.terminal => {
                Some(Damage::Full)
            }
            DaemonEvent::TerminalDelta { terminal_id, delta }
                if split_terminal == Some(*terminal_id) =>
            {
                let d = delta_damage(delta);
                self.split_damage = Some(match self.split_damage.take() {
                    Some((terminal, existing)) if terminal == *terminal_id => {
                        (terminal, existing.merge(d))
                    }
                    _ => (*terminal_id, d),
                });
                None
            }
            DaemonEvent::TerminalResync { terminal_id, .. }
                if split_terminal == Some(*terminal_id) =>
            {
                self.split_damage = Some((*terminal_id, Damage::Full));
                None
            }
            DaemonEvent::TerminalDelta { terminal_id, delta } => {
                let d = editor_delta_damage(delta);
                let merged = match self.editor_damage.remove(terminal_id) {
                    Some(existing) => existing.merge(d),
                    None => d,
                };
                self.editor_damage.insert(*terminal_id, merged);
                None
            }
            DaemonEvent::TerminalResync { terminal_id, .. } => {
                self.editor_damage.insert(*terminal_id, Damage::Full);
                None
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
    /// A resync is only worth the round trip for the terminal the main canvas
    /// is painting.
    fn apply(
        &mut self,
        event: &DaemonEvent,
        store: &mut Store,
        client: &Client,
        size: PtySize,
        active_terminal: TerminalId,
        split: Option<&SplitAttachment>,
    ) {
        if let EventOutcome::NeedsResync { terminal_id } = store.apply_event(event) {
            let split_size = split
                .filter(|open| open.terminal == terminal_id)
                .map(|open| open.size);
            if terminal_id == active_terminal || split_size.is_some() {
                let attach_size = split_size.unwrap_or(size);
                if let Ok(snapshot) = client.attach_terminal(terminal_id, attach_size) {
                    store.attach_terminal(terminal_id, &snapshot);
                    if split_size.is_some() {
                        self.split_damage = Some((terminal_id, Damage::Full));
                    } else {
                        self.damage = Some(Damage::Full);
                    }
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
        DaemonEvent::FileChanged { .. } => false,
        DaemonEvent::SharesApplied { .. } => false,
        // A draft is a read with its own event, for the same reason.
        DaemonEvent::JuvaDraftReady { .. } => false,
        // Terminal output is not shell state. Attached frames became `damage`
        // above; an unattached terminal's frames must not republish the
        // snapshot on the delta rung either.
        DaemonEvent::TerminalDelta { .. } | DaemonEvent::TerminalResync { .. } => false,
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

/// What one delta damages for the attached terminal.
///
/// A scroll moves every row, and the rows the delta names are only the ones
/// that *also* changed content — so a scrolled delta is a full repaint however
/// few rows it lists.
fn delta_damage(delta: &domain::TerminalDelta) -> Damage {
    if delta.scrolled_lines > 0 {
        Damage::Full
    } else {
        Damage::Rows(delta.changed_rows().map(|(index, _)| index).collect())
    }
}

/// Editor attachments do not walk terminal scrollback: the TUI rewrites the
/// live grid in place. `scrolled_lines` from the VT engine must not force a
/// blanking `paintAll` on the canvas.
fn editor_delta_damage(delta: &domain::TerminalDelta) -> Damage {
    Damage::Rows(delta.changed_rows().map(|(index, _)| index).collect())
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

/// A button press answers a prompt; a wheel notch or a drag does not.
fn mouse_answers(button: &str, kind: &str) -> bool {
    kind == "press" && matches!(button, "left" | "middle" | "right")
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
fn switch_session_keeping(
    client: &Client,
    store: &mut Store,
    old_terminal: TerminalId,
    session_id: SessionId,
    size: PtySize,
    keep: Option<TerminalId>,
) -> Result<TerminalId, CommandError> {
    let terminal = attach_session(client, store, session_id, size)?;
    if terminal != old_terminal && keep != Some(old_terminal) {
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
fn parse_session_role(raw: Option<&str>) -> domain::SessionRole {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("generic") => domain::SessionRole::Generic,
        Some("orchestrator") => domain::SessionRole::Orchestrator,
        Some("planner") => domain::SessionRole::Planner,
        Some("researcher") => domain::SessionRole::Researcher,
        Some("executor") => domain::SessionRole::Executor,
        Some("reviewer") => domain::SessionRole::Reviewer,
        Some("tester") => domain::SessionRole::Tester,
        Some(other) => domain::SessionRole::Custom(other.to_owned()),
    }
}

fn parse_workspace_policy(
    raw: Option<&str>,
    branch_hint: Option<String>,
) -> domain::ChildWorkspacePolicy {
    match raw.map(str::trim) {
        Some("worktree") => domain::ChildWorkspacePolicy::NewManagedWorktree {
            branch_hint,
            base: None,
        },
        _ => domain::ChildWorkspacePolicy::SameWorkspace,
    }
}

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

/// Move the extra column onto the terminal its session now runs in.
///
/// Attach first, as `switch_session_keeping` does: a failed attach leaves the
/// column on the terminal it had rather than on nothing.
fn follow_split_terminal(
    client: &Client,
    store: &mut Store,
    open: &mut SplitAttachment,
    main: TerminalId,
    terminal: TerminalId,
) -> bool {
    let snapshot = match client.attach_terminal(terminal, open.size) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(%error, "failed to follow the split session to its new terminal");
            return false;
        }
    };
    store.attach_terminal(terminal, &snapshot);
    let old = std::mem::replace(&mut open.terminal, terminal);
    if old != main {
        let _ = client.detach_terminal(old);
        store.detach_terminal(&old);
    }
    open.scroll_offset = 0;
    open.pending_echo = 0;
    true
}

/// Mint a shell without touching the window's current attachment.
fn spawn_shell(
    client: &Client,
    store: &Store,
    old_terminal: TerminalId,
    workspace: Option<WorkspaceId>,
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
    client
        .create_shell_session(workspace_id)
        .map_err(CommandError::from_client)
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

    #[test]
    fn a_button_press_answers_but_a_wheel_or_drag_does_not() {
        assert!(mouse_answers("left", "press"));
        assert!(mouse_answers("right", "press"));
        assert!(!mouse_answers("left", "release"));
        assert!(!mouse_answers("left", "motion"));
        assert!(!mouse_answers("wheel_up", "press"));
    }

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
                patches: Vec::new(),
                scrollback_len: 0,
                scrollback_generation: 0,
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
        batch.absorb(&delta(at.terminal, vec![3, 1], 0), &store, &at);
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
        batch.absorb(&delta(at.terminal, vec![0], 2), &store, &at);
        assert_eq!(batch.damage, Some(Damage::Full));
    }

    /// A delta for a terminal this window is not attached to is not shell
    /// state. Publishing the session tree for each of those frames would put
    /// `ShellSnapshot::from_store` on the delta rung.
    #[test]
    fn a_delta_for_another_terminal_is_not_shell_state() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(TerminalId::new(), vec![0], 0), &store, &at);
        assert_eq!(batch.damage, None);
        assert!(!batch.shell);
    }

    #[test]
    fn the_frame_floor_holds_a_burst_and_merges_it() {
        let mut floor = FrameFloor::default();
        let span = Duration::from_secs(60);
        assert_eq!(
            floor.offer(Damage::Rows(vec![1]), span, false),
            Some(Damage::Rows(vec![1]))
        );
        assert_eq!(floor.offer(Damage::Rows(vec![3]), span, false), None);
        assert_eq!(floor.offer(Damage::Rows(vec![2]), span, false), None);
        assert!(floor.wait(span).is_some());
        assert_eq!(floor.due(span), None, "held until the floor lifts");
        assert_eq!(floor.due(Duration::ZERO), Some(Damage::Rows(vec![2, 3])));
        assert_eq!(floor.wait(span), None);
    }

    #[test]
    fn urgent_output_skips_the_floor_and_carries_what_was_held() {
        let mut floor = FrameFloor::default();
        let span = Duration::from_secs(60);
        let _ = floor.offer(Damage::Rows(vec![0]), span, false);
        assert_eq!(floor.offer(Damage::Rows(vec![4]), span, false), None);
        assert_eq!(
            floor.offer(Damage::Rows(vec![5]), span, true),
            Some(Damage::Rows(vec![4, 5]))
        );
        assert_eq!(floor.release(Damage::Full), Damage::Full);
    }

    #[test]
    fn a_split_session_on_a_new_terminal_is_followed() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let mut row = sample_session(domain::SessionState::Running);
        let split = SplitAttachment::new(row.id, TerminalId::new(), DEFAULT_SIZE);
        let store = Store::new();

        let mut batch = Batch::default();
        row.terminal_id = Some(split.terminal);
        batch.absorb_named(
            &DaemonEvent::SessionUpdated(row.clone()),
            &store,
            &at,
            Some(&split),
        );
        assert_eq!(batch.split_retarget, None);

        let restarted = TerminalId::new();
        row.terminal_id = Some(restarted);
        batch.absorb_named(
            &DaemonEvent::SessionUpdated(row.clone()),
            &store,
            &at,
            Some(&split),
        );
        assert_eq!(batch.split_retarget, Some(restarted));

        let mut exited = Batch::default();
        row.terminal_id = None;
        exited.absorb_named(&DaemonEvent::SessionUpdated(row), &store, &at, Some(&split));
        assert_eq!(exited.split_retarget, None, "an exit is not a new terminal");
    }

    #[test]
    fn input_for_a_column_the_host_let_go_reaches_no_terminal() {
        let main_session = SessionId::new();
        let mut at = attached(TerminalId::new(), main_session, 0);
        let extra = TerminalId::new();
        let extra_session = SessionId::new();
        let mut split = Some(SplitAttachment::new(extra_session, extra, DEFAULT_SIZE));
        let mut echo = 0;

        let main = pane(&mut at, &mut echo, &mut split, None).map(|pane| pane.split);
        assert_eq!(main, Some(false));
        let named = pane(&mut at, &mut echo, &mut split, Some(main_session)).map(|p| p.split);
        assert_eq!(named, Some(false));
        let column = pane(&mut at, &mut echo, &mut split, Some(extra_session))
            .map(|pane| pane.route(Effect::shell()))
            .expect("the split column resolves");
        assert_eq!(column.damage, None);
        assert_eq!(column.split_damage, Some((extra, Damage::Full)));
        assert!(column.shell);

        split = None;
        assert!(pane(&mut at, &mut echo, &mut split, Some(extra_session)).is_none());
    }

    #[test]
    fn a_split_terminal_delta_does_not_paint_as_an_editor() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let extra = TerminalId::new();
        let store = Store::new();
        let split = SplitAttachment::new(SessionId::new(), extra, DEFAULT_SIZE);
        let mut batch = Batch::default();
        batch.absorb_named(&delta(extra, vec![2, 4], 0), &store, &at, Some(&split));
        assert_eq!(batch.split_damage, Some((extra, Damage::Rows(vec![2, 4]))));
        assert!(batch.editor_damage.is_empty());
        assert_eq!(batch.damage, None);
        assert!(!batch.shell);
    }

    #[test]
    fn the_editor_attachment_does_not_replace_the_main_pane() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let editor = TerminalId::new();
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(editor, vec![1], 0), &store, &at);
        assert_eq!(batch.damage, None);
        assert_eq!(
            batch.editor_damage.get(&editor).cloned(),
            Some(Damage::Rows(vec![1]))
        );
        assert!(!batch.shell);
    }

    /// The editor TUI rewrites rows in place; a VT `scrolled_lines` hint must
    /// not widen its damage to `Full` or the canvas blanks before painting.
    #[test]
    fn an_editor_delta_with_scrolled_lines_stays_row_damage() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let editor = TerminalId::new();
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(editor, vec![0, 2, 4], 3), &store, &at);
        assert_eq!(
            batch.editor_damage.get(&editor).cloned(),
            Some(Damage::Rows(vec![0, 2, 4]))
        );
        assert_eq!(batch.damage, None);
    }

    /// R27/R29: the pane types and resizes through its own side attachment.
    /// The main terminal is what the shell is on, and an editor keystroke that
    /// reached it would be typing into somebody else's session.
    #[test]
    fn the_editor_attachment_forwards_input_and_resize() {
        let main = TerminalId::new();
        let session = SessionId::new();
        let terminal = TerminalId::new();
        let store = Store::new();
        let mut editors = HashMap::new();
        editors.insert(
            session,
            EditorAttachment {
                terminal,
                size: DEFAULT_SIZE,
            },
        );

        let key = super::super::input::KeyPress {
            key: "a".to_owned(),
            ctrl: false,
            alt: false,
            shift: false,
        };
        let (target, bytes) = editor_input_target(&store, &editors, session, &key)
            .expect("an open editor takes the keystroke");
        assert_eq!(target, terminal, "input goes to the editor's own terminal");
        assert_ne!(target, main, "never the main attachment");
        assert_eq!(bytes, b"a".to_vec());

        // A session with no editor open swallows the keystroke rather than
        // routing it anywhere.
        assert!(editor_input_target(&store, &editors, SessionId::new(), &key).is_none());

        // The pane re-measures every frame of a drag; only a size it does not
        // already have is a resize.
        assert_eq!(
            editor_resize_target(&mut editors, session, DEFAULT_SIZE),
            None,
            "the size it already has is not a resize"
        );
        let wider = PtySize {
            cols: DEFAULT_SIZE.cols + 10,
            ..DEFAULT_SIZE
        };
        assert_eq!(
            editor_resize_target(&mut editors, session, wider),
            Some(terminal)
        );
        assert_eq!(
            editors.get(&session).map(|open| open.size),
            Some(wider),
            "the new size is remembered, so the next identical frame is a no-op"
        );
        assert_eq!(editor_resize_target(&mut editors, session, wider), None);
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
        first.absorb(&event, &store, &at);
        assert!(first.shell, "the badge went from off to on");

        let _ = store.apply_event(&event);
        let mut second = Batch::default();
        second.absorb(&event, &store, &at);
        assert!(!second.shell, "the badge was already on");
    }

    /// OSC 52 lets whatever runs in a PTY set the clipboard, so only a
    /// terminal the person is looking at may.
    #[test]
    fn only_a_terminal_on_screen_may_set_the_clipboard() {
        let focused = TerminalId::new();
        let editor = TerminalId::new();
        let background = TerminalId::new();
        let at = attached(focused, SessionId::new(), 0);
        let mut editors = HashMap::new();
        editors.insert(
            SessionId::new(),
            EditorAttachment {
                terminal: editor,
                size: DEFAULT_SIZE,
            },
        );

        assert!(clipboard_is_allowed(focused, &at, &editors, None));
        assert!(clipboard_is_allowed(editor, &at, &editors, None));
        assert!(
            !clipboard_is_allowed(background, &at, &editors, None),
            "a background agent must not be able to replace the clipboard"
        );
        assert!(
            !clipboard_is_allowed(editor, &at, &HashMap::new(), None),
            "a detached editor is not on screen either"
        );
        let split = SplitAttachment::new(SessionId::new(), background, DEFAULT_SIZE);
        assert!(
            clipboard_is_allowed(background, &at, &editors, Some(&split)),
            "a terminal in the extra column is on screen"
        );
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
        first.absorb(&event, &store, &at);
        assert!(first.shell);

        let _ = store.apply_event(&event);
        let mut second = Batch::default();
        second.absorb(&event, &store, &at);
        assert!(!second.shell);
    }

    #[test]
    fn two_deltas_in_one_batch_merge_their_rows() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(at.terminal, vec![1], 0), &store, &at);
        batch.absorb(&delta(at.terminal, vec![2], 0), &store, &at);
        assert_eq!(batch.damage, Some(Damage::Rows(vec![1, 2])));
    }

    #[test]
    fn a_resync_for_the_attached_terminal_widens_to_full() {
        let at = attached(TerminalId::new(), SessionId::new(), 0);
        let store = Store::new();
        let mut batch = Batch::default();
        batch.absorb(&delta(at.terminal, vec![1], 0), &store, &at);
        batch.absorb(
            &DaemonEvent::TerminalResync {
                terminal_id: at.terminal,
                snapshot: domain::TerminalSnapshot {
                    scrollback_generation: 0,
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

    #[test]
    fn a_queued_reference_is_not_replayed_on_a_new_connection_to_the_same_terminal() {
        let (commands, receiver) = flume::bounded(1);
        let runtime = Runtime {
            commands,
            latest: Arc::new(Mutex::new(Latest::default())),
            workbench: Arc::new(Mutex::new(None)),
        };
        let session = SessionId::new();
        let terminal = TerminalId::new();
        let reference = || RuntimeCommand::PasteTarget {
            session_id: session,
            terminal_id: terminal,
            text: "'/checkout/a b'".into(),
            connection_generation: 1,
        };
        assert!(runtime.send(reference()).is_err());
        let payload = ConnectedPayload {
            connection_generation: 1,
            daemon: DaemonInfoDto {
                protocol_version: 1,
                daemon_version: "test".into(),
                instance_id: "same-daemon".into(),
                started_at: domain::Timestamp::now(),
                editor_surface: client::EditorSurface::Cells,
            },
            session_count: 0,
            store: ShellSnapshot::from_store(&Store::new(), None),
            active_session: Some(session),
            active_terminal: Some(terminal),
        };
        remember_connected(&runtime.latest, payload.clone());
        runtime.send(reference()).unwrap();
        assert!(runtime.send(reference()).is_err());
        let queued = receiver.recv().unwrap();
        assert!(command_on_connection(&queued, 1));
        remember_connected(
            &runtime.latest,
            ConnectedPayload {
                connection_generation: 2,
                ..payload
            },
        );
        assert!(!command_on_connection(&queued, 2));
        assert!(runtime.send(reference()).is_err());
        assert!(receiver.is_empty());
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
            editor: None,
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
    fn a_reference_never_follows_a_changed_session_or_restarted_terminal() {
        let terminal = TerminalId::new();
        let mut session = sample_session(domain::SessionState::Running);
        session.terminal_id = Some(terminal);
        let at = attached(terminal, session.id, 0);
        let mut store = Store::new();
        let _ = store.apply_event(&DaemonEvent::SessionCreated(session.clone()));
        assert!(paste_target_matches(
            &store, &at, None, session.id, terminal
        ));
        assert!(!paste_target_matches(
            &store,
            &at,
            None,
            SessionId::new(),
            terminal
        ));
        assert!(!paste_target_matches(
            &store,
            &at,
            None,
            session.id,
            TerminalId::new()
        ));
        let other = attached(TerminalId::new(), SessionId::new(), 0);
        assert!(!paste_target_matches(
            &store, &other, None, session.id, terminal
        ));
        session.terminal_id = Some(TerminalId::new());
        let _ = store.apply_event(&DaemonEvent::SessionUpdated(session.clone()));
        assert!(!paste_target_matches(
            &store, &at, None, session.id, terminal
        ));
        session.terminal_id = Some(terminal);
        session.state = domain::SessionState::Exited {
            code: Some(0),
            signal: None,
        };
        let _ = store.apply_event(&DaemonEvent::SessionUpdated(session.clone()));
        assert!(!paste_target_matches(
            &store, &at, None, session.id, terminal
        ));
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

    /// Closing the Code view detaches the terminal, not the editor: the
    /// process keeps running with its draft. Reopening the same path must find
    /// that session — by the daemon's `editor.path`, not by this window's
    /// attachments — so the pane re-attaches instead of opening the disk state
    /// under the draft.
    #[test]
    fn a_detached_editor_session_is_still_the_one_holding_the_path() {
        let workspace = WorkspaceId::new();
        let terminal = TerminalId::new();
        let mut session = sample_session(domain::SessionState::Running);
        session.workspace_id = workspace;
        session.kind = domain::SessionKind::Editor;
        session.terminal_id = Some(terminal);
        session.editor = Some(domain::EditorState {
            path: "src/main.rs".to_string(),
            ..domain::EditorState::default()
        });

        let mut store = Store::new();
        let _ = store.apply_event(&DaemonEvent::SessionCreated(session.clone()));
        let detached = HashMap::new();
        assert_eq!(
            live_editor_for(&store, &detached, workspace, "src/main.rs"),
            Some((session.id, terminal)),
            "a detached session still names the terminal to re-attach"
        );
        assert_eq!(
            live_editor_for(&store, &detached, workspace, "src/other.rs"),
            None,
            "a different path is a different file"
        );

        session.state = domain::SessionState::Exited {
            code: Some(0),
            signal: None,
        };
        let mut dead = Store::new();
        let _ = dead.apply_event(&DaemonEvent::SessionCreated(session));
        assert_eq!(
            live_editor_for(&dead, &detached, workspace, "src/main.rs"),
            None,
            "an exited editor is history, not a session to re-attach"
        );
    }
}
