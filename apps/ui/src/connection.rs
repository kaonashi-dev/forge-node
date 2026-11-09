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
    RefreshSnapshot,
    ForceReconnect,
    Shutdown,
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
    CommandResult {
        generation: u64,
        label: &'static str,
        ok: bool,
        message: String,
        took_us: u64,
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

    pub fn refresh_snapshot(&self) -> Result<(), String> {
        self.send(Command::RefreshSnapshot)
    }

    pub fn force_reconnect(&self) -> Result<(), String> {
        self.send(Command::ForceReconnect)
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
        Command::RefreshSnapshot | Command::ForceReconnect | Command::Shutdown => {}
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
        Command::RefreshSnapshot => {
            if let Ok((store, took_us)) = load_snapshot(client, metrics) {
                emit_snapshot(generation, &store, took_us, sink, metrics);
            }
        }
        Command::ForceReconnect | Command::Shutdown => {}
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
    let Some(session) = session else {
        return Ok(());
    };
    metrics.note_socket_op();
    let snapshot = client
        .attach_terminal(terminal_id, size.unwrap_or_default())
        .map_err(|error| error.to_string())?;
    terminal.publish_snapshot(generation, session, terminal_id, &snapshot);
    Ok(())
}

fn bootstrap(client: &Client, metrics: &Metrics) -> Result<(Store, u64), String> {
    let started = Instant::now();
    let mut store = load_snapshot(client, metrics)?.0;
    if store
        .sessions
        .iter()
        .any(|session| session.terminal_id.is_some())
    {
        return Ok((store, started.elapsed().as_micros() as u64));
    }
    if store.workspaces.is_empty() {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        metrics.note_socket_op();
        client
            .add_project(&cwd)
            .map_err(|error| format!("AddProject {}: {error}", cwd.display()))?;
        store = load_snapshot(client, metrics)?.0;
    }
    let workspace = store
        .workspaces
        .first()
        .map(|workspace| workspace.id)
        .ok_or_else(|| "daemon has no workspace after AddProject".to_string())?;
    metrics.note_socket_op();
    client
        .create_shell_session(workspace)
        .map_err(|error| format!("CreateShellSession: {error}"))?;
    let store = load_snapshot(client, metrics)?.0;
    Ok((store, started.elapsed().as_micros() as u64))
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
        Command::RefreshSnapshot => "GetSnapshot",
        Command::ForceReconnect => "Reconnect",
        Command::Shutdown => "Shutdown",
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
