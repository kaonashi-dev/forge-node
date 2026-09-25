//! Supervisor for one integrated editor session.
//!
//! The daemon binds a private Unix socket **before** it spawns
//! `forge-editor`; the editor connects after spawn (`portable-pty` closes
//! every inherited fd over 2, so the path travels in the environment), performs
//! a versioned handshake, and takes the buffer. Cells editing uses the PTY;
//! headless editing uses structured input and bounded line windows on this
//! channel. Buffer metadata is never parsed out of ANSI.
//!
//! Everything that can block — accept, handshake, socket IO — runs on this
//! module's thread, never under the core lock. `Supervisor`'s `Drop` removes
//! the socket file on every path, failure included, because it lives inside the
//! service thread until that thread ends.

use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use domain::{EditorState, SessionId};
use editor_control::{
    read_frame, version_matches, write_frame, ControlError, DaemonMessage, EditorMessage,
    EditorStateWire, CONTROL_VERSION,
};

use crate::core::{Daemon, SaveOutcome};

/// How long the daemon waits for the editor to connect, handshake and take the
/// buffer. A spawn that never reaches the handshake is a failed session, not a
/// hang.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounded command queue toward one editor. Full means busy, never a drop.
pub const COMMAND_QUEUE: usize = 8;

// At most 32 encoded bytes per mark leaves room for framing and message fields.
pub const MAX_GIT_MARKS: usize = 65_536;

/// A request the daemon wants the editor to run, in order.
#[derive(Clone, Debug)]
pub enum Outgoing {
    Reveal {
        request_id: u64,
        line: u32,
        column: Option<u32>,
    },
    GetState {
        request_id: u64,
    },
    /// Take disk: replace the buffer with what the daemon just read.
    Reload {
        request_id: u64,
        text: String,
        revision: Option<String>,
    },
    /// Keep mine: ask for the draft again, now that the revision is the disk's.
    Save {
        request_id: u64,
    },
    /// Which lines the working tree changed, for the gutter.
    GitMarks {
        request_id: u64,
        marks: Vec<editor_control::WireMark>,
    },
    /// Turn saving-on-a-pause on or off.
    SetAutosave {
        request_id: u64,
        autosave: bool,
    },
    /// What the person did in a DOM surface, already clamped.
    Input {
        events: Vec<editor_control::EditorInput>,
    },
    /// Which lines that surface is showing.
    SetView {
        request_id: u64,
        first_line: u32,
        line_count: u32,
    },
    /// What the GUI's find panel asked, already checked against its limit.
    Find {
        command: editor_control::WireFindCommand,
    },
}

impl Outgoing {
    fn into_message(self) -> DaemonMessage {
        match self {
            Self::Reveal {
                request_id,
                line,
                column,
            } => DaemonMessage::Reveal {
                request_id,
                line,
                column,
            },
            Self::GetState { request_id } => DaemonMessage::GetState { request_id },
            Self::Reload {
                request_id,
                text,
                revision,
            } => DaemonMessage::Reload {
                request_id,
                text,
                revision,
            },
            Self::Save { request_id } => DaemonMessage::Save { request_id },
            Self::GitMarks { request_id, marks } => DaemonMessage::GitMarks { request_id, marks },
            Self::SetAutosave {
                request_id,
                autosave,
            } => DaemonMessage::SetAutosave {
                request_id,
                autosave,
            },
            Self::Input { events } => DaemonMessage::Input {
                request_id: None,
                events,
            },
            Self::SetView {
                request_id,
                first_line,
                line_count,
            } => DaemonMessage::SetView {
                request_id,
                view: editor_control::ViewRequest {
                    first_line,
                    line_count,
                },
            },
            Self::Find { command } => DaemonMessage::Find { command },
        }
    }
}

/// The live sender for one editor's command queue.
#[derive(Clone, Debug)]
pub struct CommandPort {
    tx: flume::Sender<Outgoing>,
    reload: Arc<Mutex<Option<PendingReload>>>,
    retarget: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Debug)]
struct PendingReload {
    request_id: u64,
    revision: Option<String>,
}

impl CommandPort {
    #[must_use]
    pub fn new(tx: flume::Sender<Outgoing>) -> Self {
        Self {
            tx,
            reload: Arc::new(Mutex::new(None)),
            retarget: Arc::new(Mutex::new(None)),
        }
    }

    /// One coalesced metadata slot survives a saturated command queue.
    pub fn retarget(&self, path: String) {
        *self.retarget.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
        // A full queue already wakes the writer, which checks the slot before every command.
        let _ = self.tx.try_send(Outgoing::GetState { request_id: 0 });
    }

    pub fn take_retarget(&self) -> Option<String> {
        self.retarget
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Enqueue, or say busy. Never drops a `request_id` on the floor.
    pub fn try_send(&self, mut command: Outgoing) -> Result<(), Busy> {
        if let Outgoing::GitMarks { marks, .. } = &mut command {
            if marks.len() > MAX_GIT_MARKS {
                tracing::warn!(
                    count = marks.len(),
                    "editor gutter marks skipped: over budget"
                );
                marks.clear();
            }
        }
        if let Outgoing::Reload {
            request_id,
            revision,
            ..
        } = &command
        {
            let mut pending = self.reload.lock().unwrap_or_else(|e| e.into_inner());
            if pending.is_some() {
                return Err(Busy);
            }
            *pending = Some(PendingReload {
                request_id: *request_id,
                revision: revision.clone(),
            });
            if self.tx.try_send(command).is_err() {
                *pending = None;
                return Err(Busy);
            }
            return Ok(());
        }
        self.tx.try_send(command).map_err(|_| Busy)
    }

    pub fn finish_reload(&self, request_id: u64) -> Option<Option<String>> {
        let mut pending = self.reload.lock().unwrap_or_else(|e| e.into_inner());
        if pending
            .as_ref()
            .is_some_and(|reload| reload.request_id == request_id)
        {
            pending.take().map(|reload| reload.revision)
        } else {
            None
        }
    }
}

/// The editor's command queue is full; the caller must retry or refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// How often the daemon may re-broadcast a changed editor state: a burst of
/// typing becomes at most four `SessionUpdated`s a second. The last state
/// always lands — the read deadline flushes it, the way `pty_loop`'s emit
/// floor flushes held-back terminal damage.
const STATE_BROADCAST_COOLDOWN: Duration = Duration::from_millis(250);

/// Uses a shorter runtime directory when needed, preserving the full session id.
pub fn control_socket_path(session_id: SessionId) -> io::Result<PathBuf> {
    // UUID v7 prefixes can coincide for sessions created in the same millisecond.
    let name = format!("editor-{}.sock", session_id.as_uuid().simple());
    let dir = crate::paths::resolve_runtime_dir_for_name(&name)
        .map_err(|e| io::Error::other(e.to_string()))?;
    crate::paths::ensure_private_dir(&dir).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(dir.join(name))
}

/// Bind the editor's control socket with private permissions, replacing a
/// stale file (the same shape as `server::bind`).
fn bind_socket(socket_path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = socket_path.parent() {
        crate::paths::ensure_private_dir(parent).map_err(|e| io::Error::other(e.to_string()))?;
    }
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// What one buffer is opened with.
///
/// Grouped rather than passed one by one: they travel together from the
/// request to the handshake, and four positional `bool`s in a row is how a
/// caller ends up swapping `read_only` for `autosave`.
pub struct OpenSpec {
    pub buffer: fs_service::FileContents,
    /// 1-based line to reveal at startup.
    pub line: Option<u32>,
    pub read_only: bool,
    pub autosave: bool,
}

/// The supervisor of one editor session.
///
/// Created (socket bound) before the spawn; [`Supervisor::supervise`] moves it
/// into the service thread, so its `Drop` — which removes the socket file —
/// runs when that thread ends, on every path.
pub struct Supervisor {
    session_id: SessionId,
    socket: PathBuf,
    listener: Option<UnixListener>,
}

impl Supervisor {
    /// Bind the control socket. Errors here fail the whole creation request.
    pub fn bind(session_id: SessionId) -> io::Result<Self> {
        let socket = control_socket_path(session_id)?;
        let listener = bind_socket(&socket)?;
        Ok(Self {
            session_id,
            socket,
            listener: Some(listener),
        })
    }

    /// The socket path, for the launch environment.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Hand the buffer over and serve the session.
    ///
    /// Accept with a deadline, handshake, flip the session to `Running`, then
    /// coalesce the editor's state into `SessionUpdated` broadcasts. Any
    /// failure — accept deadline, version mismatch, socket death — kills the
    /// process group and marks the session `Failed`.
    pub fn supervise(
        mut self,
        daemon: Arc<Daemon>,
        spec: OpenSpec,
        commands: flume::Receiver<Outgoing>,
    ) {
        let Some(listener) = self.listener.take() else {
            return;
        };
        let session_id = self.session_id;
        let _span = tracing::info_span!("editor.supervise", %session_id).entered();
        std::thread::Builder::new()
            .name("forge-editor".to_owned())
            .spawn(move || {
                // `self` (and with it the socket file) dies with this closure,
                // on every path.
                let _supervisor = self;
                serve(daemon, session_id, listener, spec, commands);
            })
            .expect("spawn the editor supervisor thread");
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// The session's whole service life: accept, handshake, request loop.
fn serve(
    daemon: Arc<Daemon>,
    session_id: SessionId,
    listener: UnixListener,
    spec: OpenSpec,
    commands: flume::Receiver<Outgoing>,
) {
    let OpenSpec {
        buffer,
        line,
        read_only,
        autosave,
    } = spec;
    struct PortGuard {
        daemon: Arc<Daemon>,
        session_id: SessionId,
    }
    impl Drop for PortGuard {
        fn drop(&mut self) {
            self.daemon.drop_editor_port(self.session_id);
        }
    }
    let _port = PortGuard {
        daemon: daemon.clone(),
        session_id,
    };

    let mut stream = match accept_with_deadline(listener, HANDSHAKE_TIMEOUT) {
        Ok(stream) => stream,
        Err(reason) => {
            fail(&daemon, session_id, reason);
            return;
        }
    };
    let opening = Opening {
        buffer: &buffer,
        line,
        read_only,
        autosave,
    };
    if let Err(error) = handshake(&mut stream, session_id, opening) {
        fail(
            &daemon,
            session_id,
            format!("control handshake failed: {error}"),
        );
        return;
    }
    // The buffer is open; the editor is usable.
    daemon.set_editor_running(session_id);
    // Marks are decoration: a git failure must not take the buffer with it.
    let push_marks = |daemon: &Arc<Daemon>, path: &str, text: &str| {
        if let Some(marks) = daemon.editor_git_marks(session_id, path, text) {
            daemon.publish_editor_git_marks(session_id, path, marks);
        }
    };
    push_marks(&daemon, &buffer.path, &buffer.text);

    // Two producers write this socket — the command thread below, and the read
    // loop answering a save — so the write half is shared behind one `Mutex`.
    // Without it two `write_frame`s can interleave and the editor decodes a
    // frame built from halves of both. The read loop keeps its own handle for
    // reading, which never contends.
    let writer = match stream.try_clone() {
        Ok(half) => Arc::new(Mutex::new(half)),
        Err(error) => {
            fail(
                &daemon,
                session_id,
                format!("could not split the control stream: {error}"),
            );
            return;
        }
    };
    {
        let writer = Arc::clone(&writer);
        let daemon = Arc::clone(&daemon);
        std::thread::Builder::new()
            .name("forge-editor-cmd".to_owned())
            .spawn(move || {
                while let Ok(command) = commands.recv() {
                    if let Some(path) = daemon.take_editor_retarget(session_id) {
                        if send(&writer, &DaemonMessage::Retarget { path }).is_err() {
                            break;
                        }
                    }
                    let optional = matches!(command, Outgoing::GitMarks { .. });
                    match send(&writer, &command.into_message()) {
                        Ok(()) => {}
                        Err(ControlError::Encode(_) | ControlError::FrameTooLarge { .. })
                            if optional =>
                        {
                            tracing::warn!("editor gutter marks skipped: encoding failed");
                        }
                        Err(_) => break,
                    }
                }
            })
            // The queue is bounded and the thread only writes: a spawn failure
            // here is the process being out of threads, which no editor
            // session can recover from.
            .expect("spawn the editor command writer");
    }

    let mut revision = buffer.revision.clone();
    // Whether the last save was refused because the file moved. Held here
    // rather than on the session, because this thread is what learns it.
    let mut conflict = false;

    let mut last_broadcast: Option<Instant> = None;
    let mut pending: Option<EditorStateWire> = None;
    let mut decoder = editor_control::FrameReader::default();
    loop {
        let wait = pending.as_ref().map(|_| {
            let since = last_broadcast.unwrap_or_else(Instant::now);
            STATE_BROADCAST_COOLDOWN
                .checked_sub(since.elapsed())
                .unwrap_or(Duration::ZERO)
        });
        let _ = stream.set_read_timeout(wait.map(|wait| wait.max(Duration::from_millis(1))));
        match decoder.read::<EditorMessage>(&mut stream) {
            Ok(EditorMessage::State { state, .. }) => {
                pending = Some(state);
                if due(last_broadcast) {
                    flush(&daemon, session_id, pending.take(), conflict);
                    last_broadcast = Some(Instant::now());
                }
            }
            Ok(EditorMessage::SaveRequest {
                request_id,
                text,
                document_version,
            }) => {
                let operation = daemon.editor_file_operation();
                let Ok((_, path)) = daemon.editor_target(session_id) else {
                    return;
                };
                // Blocking disk IO, on this thread and never under the core
                // lock: `save_editor_buffer` clones the root and writes off it.
                // Enforced here, not only in the editor: `read_only` travels in
                // `Open` as a courtesy, but the daemon owns the checkout and a
                // buggy or replaced editor must not be able to write through
                // a buffer that was opened read-only.
                if read_only {
                    drop(operation);
                    let refusal = DaemonMessage::SaveRefused {
                        request_id,
                        reason: "this buffer is open read-only".to_owned(),
                    };
                    if send(&writer, &refusal).is_err() {
                        return;
                    }
                    continue;
                }
                let answer = match daemon.save_editor_buffer(session_id, &path, &text, &revision) {
                    SaveOutcome::Written { revision: written } => {
                        tracing::debug!(%session_id, %document_version, "editor buffer saved");
                        revision = written.clone();
                        conflict = false;
                        daemon.clear_editor_conflict(session_id);
                        DaemonMessage::Saved {
                            request_id,
                            revision: written,
                        }
                    }
                    // Remember what is on disk even though nothing was
                    // written: the next save is then a decision the person
                    // makes, not one the stale revision forbids forever.
                    SaveOutcome::Stale {
                        revision: current,
                        disk,
                        reason,
                    } => {
                        tracing::debug!(%session_id, "editor save refused: the file moved");
                        revision = current;
                        conflict = true;
                        // Kept so the person can see both sides. This is the
                        // only moment the daemon holds the draft, so it is the
                        // only moment it can offer the comparison at all.
                        daemon.record_editor_conflict(
                            session_id,
                            crate::core::EditorConflict {
                                path: path.clone(),
                                disk,
                                mine: text.clone(),
                            },
                        );
                        DaemonMessage::SaveRefused { request_id, reason }
                    }
                    SaveOutcome::Failed(reason) => {
                        DaemonMessage::SaveRefused { request_id, reason }
                    }
                };
                drop(operation);
                if send(&writer, &answer).is_err() {
                    return;
                }
                if matches!(answer, DaemonMessage::Saved { .. }) {
                    push_marks(&daemon, &path, &text);
                }
            }
            // Straight through: the editor already coalesced this against its
            // own emit floor and against the last window it published, so a
            // frame arriving here is one the surface has not seen. Broadcast
            // and never stored — a window is runtime state like a
            // `TerminalDelta`, not something a session carries.
            Ok(EditorMessage::ViewFrame { frame }) => {
                daemon
                    .registry
                    .broadcast_domain(protocol::DaemonEvent::EditorFrame {
                        session_id,
                        frame: crate::editor_wire::frame_to_domain(frame),
                    });
            }
            Ok(EditorMessage::Applied { request_id, .. }) => {
                if let Some(loaded) = daemon.finish_editor_reload(session_id, request_id) {
                    if let Some(loaded) = loaded {
                        revision = loaded;
                    }
                    conflict = false;
                    pending = None;
                    daemon.clear_editor_conflict(session_id);
                }
            }
            Ok(EditorMessage::Refused { request_id, .. }) => {
                daemon.finish_editor_reload(session_id, request_id);
            }
            // Disk work, on this thread and off the core lock — the rule
            // go-to-definition exists under (AGENTS.md). The symbol is
            // validated as an identifier by `fs-service` before it reaches
            // `git grep`, so a caret in a buffer cannot become a regex.
            Ok(EditorMessage::FindDefinition { request_id, symbol }) => {
                let places = daemon.editor_definitions(session_id, &symbol);
                let answer = DaemonMessage::Definitions {
                    request_id,
                    symbol,
                    places,
                };
                if send(&writer, &answer).is_err() {
                    return;
                }
            }
            Ok(EditorMessage::RunDiagnostics { request_id }) => {
                let Ok((_, path)) = daemon.editor_target(session_id) else {
                    return;
                };
                let found = daemon.editor_diagnostics(session_id, &path);
                let answer = DaemonMessage::Diagnostics {
                    request_id,
                    command: found.is_some(),
                    items: found.unwrap_or_default(),
                };
                if send(&writer, &answer).is_err() {
                    return;
                }
            }
            Ok(EditorMessage::ChangeDetails { request_id, line }) => {
                let Ok((_, path)) = daemon.editor_target(session_id) else {
                    return;
                };
                let found = daemon.editor_change_details(session_id, &path, line);
                let answer = match found {
                    Some(hunk) => DaemonMessage::ChangeDetails {
                        request_id,
                        line: hunk.line,
                        before: hunk.before,
                        after: hunk.after,
                        truncated: hunk.truncated,
                    },
                    None => DaemonMessage::ChangeDetails {
                        request_id,
                        line: 0,
                        before: Vec::new(),
                        after: Vec::new(),
                        truncated: false,
                    },
                };
                if send(&writer, &answer).is_err() {
                    return;
                }
            }
            Ok(EditorMessage::OpenPath {
                request_id,
                path: wanted,
                line,
            }) => {
                // A refusal is worth saying: the candidate came from a grep of
                // a tree that may have moved since.
                if let Err(error) = daemon.editor_open_path(session_id, &wanted, line) {
                    let answer = DaemonMessage::SaveRefused {
                        request_id,
                        reason: format!("could not open {wanted}: {}", error.message),
                    };
                    if send(&writer, &answer).is_err() {
                        return;
                    }
                }
            }
            Ok(other) => {
                tracing::debug!(%session_id, message = ?other, "editor message");
            }
            Err(ControlError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                // The deadline fired with a state still queued: that flush is
                // the coalescing, not a sleep-and-check loop.
                if let Some(state) = pending.take() {
                    flush(&daemon, session_id, Some(state), conflict);
                    last_broadcast = Some(Instant::now());
                }
            }
            Err(error) => {
                tracing::debug!(%session_id, error = %error, "editor control channel ended");
                return;
            }
        }
    }
}

/// Write one frame through the shared write half.
///
/// Every producer goes through here: the lock is what keeps two frames from
/// interleaving on one socket. A poisoned lock means a writer panicked
/// mid-frame, so the stream is no longer trustworthy and the caller ends the
/// session rather than appending to a half-written frame.
fn send(writer: &Arc<Mutex<UnixStream>>, message: &DaemonMessage) -> Result<(), ControlError> {
    let mut half = writer
        .lock()
        .map_err(|_| ControlError::Io(io::Error::other("the control writer panicked mid-frame")))?;
    write_frame(&mut *half, message)
}

/// Wait for the editor to connect, with a deadline, on `poll(2)` — the shape
/// `pty_loop` already uses for its read deadline. A nonblocking listener plus a
/// poll deadline is the only way std offers to cancel an `accept`.
fn accept_with_deadline(listener: UnixListener, timeout: Duration) -> Result<UnixStream, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("could not arm the control socket: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("the editor never connected to its control socket".to_owned());
        }
        let ready = {
            // The `PollFd` borrows the listener; the borrow ends with the
            // block, before `accept`.
            let mut fds = [nix::poll::PollFd::new(
                listener.as_fd(),
                nix::poll::PollFlags::POLLIN,
            )];
            let millis = remaining.as_millis().min(u16::MAX as u128) as u16;
            nix::poll::poll(&mut fds, millis)
                .map_err(|e| format!("control accept poll failed: {e}"))?
        };
        if ready == 0 {
            return Err("the editor never connected to its control socket".to_owned());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|e| format!("could not restore the control stream: {e}"))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(format!("control accept failed: {error}")),
        }
    }
}

/// The borrowed half of an [`OpenSpec`], for the handshake.
struct Opening<'a> {
    buffer: &'a fs_service::FileContents,
    line: Option<u32>,
    read_only: bool,
    autosave: bool,
}

/// Read the editor's `Hello`, answer `Welcome`, and hand over the buffer.
///
/// The handshake is the one place a request id is minted by the daemon; the
/// editor's `Opened` echoes it. `read_only` is the opener's, not a constant:
/// an integrated buffer saves through [`EditorMessage::SaveRequest`].
fn handshake(
    stream: &mut UnixStream,
    session_id: SessionId,
    opening: Opening<'_>,
) -> Result<(), String> {
    let Opening {
        buffer,
        line,
        read_only,
        autosave,
    } = opening;
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    // `Open` carries the whole file, which is larger than any socket buffer:
    // an editor that connects and never reads would otherwise block this
    // thread in `write_all` forever, with the read deadline never reached and
    // the session stuck in `Starting`.
    stream
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let who = match read_frame::<EditorMessage>(stream).map_err(|e| e.to_string())? {
        EditorMessage::Hello {
            version,
            session_id: who,
            pid,
        } => {
            tracing::debug!(%session_id, %pid, "editor connected");
            (version, who)
        }
        other => return Err(format!("expected Hello, got {other:?}")),
    };
    if !version_matches(who.0) {
        return Err(format!(
            "control version mismatch: the editor speaks {}, this daemon speaks {CONTROL_VERSION}",
            who.0
        ));
    }
    if who.1 != session_id.to_string() {
        return Err(format!(
            "control session mismatch: the editor named {}, this session is {}",
            who.1, session_id
        ));
    }
    write_frame(
        stream,
        &DaemonMessage::Welcome {
            version: CONTROL_VERSION,
            session_id: session_id.to_string(),
            buffer_id: 1,
        },
    )
    .map_err(|e| e.to_string())?;
    write_frame(
        stream,
        &DaemonMessage::Open {
            request_id: 1,
            buffer_id: 1,
            path: buffer.path.clone(),
            text: buffer.text.clone(),
            revision: Some(buffer.revision.clone()),
            line,
            read_only,
            autosave,
        },
    )
    .map_err(|e| e.to_string())?;
    match read_frame::<EditorMessage>(stream).map_err(|e| e.to_string())? {
        EditorMessage::Opened { .. } => Ok(()),
        other => Err(format!("expected Opened, got {other:?}")),
    }
}

/// Kill the process group and say why the session failed.
fn fail(daemon: &Arc<Daemon>, session_id: SessionId, reason: String) {
    tracing::warn!(%session_id, reason = %reason, "editor session failed before its buffer opened");
    // `Failed` first: the PTY EOF that follows the kill would otherwise be a
    // legal `Starting → Exited`, and the reason is the useful one to keep.
    daemon.mark_session_failed(session_id, reason);
    let _ = daemon.kill_session(session_id);
}

/// Convert and store one state, then broadcast it.
///
/// `conflict` is the supervisor's, not the editor's: the editor is only told a
/// reason string, while this thread is the one that saw the revision mismatch.
/// It is merged in here so a later state notification cannot quietly clear it.
fn flush(
    daemon: &Arc<Daemon>,
    session_id: SessionId,
    state: Option<EditorStateWire>,
    conflict: bool,
) {
    let Some(state) = state else { return };
    daemon.record_editor_state(
        session_id,
        EditorState {
            path: state.path,
            line: state.line,
            column: state.column,
            dirty: state.dirty,
            read_only: state.read_only,
            document_version: state.document_version,
            top_line: state.top_line,
            visible_lines: state.visible_lines,
            total_lines: state.total_lines,
            caret_line: state.caret_line,
            selection_length: state.selection_length,
            cursor_count: state.cursor_count,
            status: state.status,
            conflict,
            find: state
                .find
                .map(|find| Box::new(crate::editor_wire::find_to_domain(find))),
        },
    );
}

/// Whether a state may be broadcast right now.
fn due(last: Option<Instant>) -> bool {
    last.is_none_or(|at| at.elapsed() >= STATE_BROADCAST_COOLDOWN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::SessionId;
    use editor_control::{write_frame, EditorMessage, CONTROL_VERSION};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::Duration;

    fn buffer() -> fs_service::FileContents {
        fs_service::FileContents {
            path: "src/main.rs".into(),
            text: "fn main() {}\n".into(),
            revision: "rev".into(),
            language: "rust".into(),
            binary: false,
            too_large: false,
        }
    }

    #[test]
    fn same_millisecond_sessions_keep_distinct_control_socket_paths() {
        let first: SessionId = "01990aab-1234-7000-8000-000000000001".parse().unwrap();
        let second: SessionId = "01990aab-1234-7000-8000-000000000002".parse().unwrap();
        let first_path = control_socket_path(first).unwrap();
        let second_path = control_socket_path(second).unwrap();

        assert_ne!(first_path, second_path);
        for (id, path) in [(first, first_path), (second, second_path)] {
            assert!(path.ends_with(format!("editor-{}.sock", id.as_uuid().simple())));
            assert!(path.as_os_str().len() < crate::paths::MAX_SOCKET_PATH_LEN);
        }
    }

    #[test]
    fn the_control_socket_is_private_and_removed_on_drop() {
        let id = SessionId::new();
        let supervisor = Supervisor::bind(id).expect("bind");
        let path = supervisor.socket().to_path_buf();
        assert!(path.exists(), "the socket file is created before spawn");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the control socket is private");
        drop(supervisor);
        assert!(
            !path.exists(),
            "Drop removes the socket on every path, including unused"
        );
    }

    #[test]
    fn a_handshake_version_mismatch_is_refused() {
        let id = SessionId::new();
        let (mut editor, mut daemon_side) = UnixStream::pair().expect("pair");
        thread::spawn(move || {
            let _ = write_frame(
                &mut editor,
                &EditorMessage::Hello {
                    version: CONTROL_VERSION + 1,
                    session_id: id.to_string(),
                    pid: 1,
                },
            );
        });
        let error = handshake(
            &mut daemon_side,
            id,
            Opening {
                buffer: &buffer(),
                line: None,
                read_only: true,
                autosave: false,
            },
        )
        .expect_err("mismatch");
        assert!(
            error.contains("version"),
            "a foreign control version must be named: {error}"
        );
    }

    #[test]
    fn editor_session_handshake_timeout_fails_the_session() {
        let id = SessionId::new();
        let supervisor = Supervisor::bind(id).expect("bind");
        let listener = supervisor.listener.as_ref().unwrap().try_clone().unwrap();
        let started = Instant::now();
        let error = accept_with_deadline(listener, Duration::from_millis(50))
            .expect_err("nobody connected");
        assert!(error.contains("never connected"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the accept deadline must not hang"
        );
    }

    #[test]
    fn reload_acknowledgements_only_commit_the_matching_revision() {
        let (tx, rx) = flume::bounded(2);
        let port = CommandPort::new(tx);
        let reload = |id| Outgoing::Reload {
            request_id: id,
            text: "disk".into(),
            revision: Some("new".into()),
        };
        port.try_send(reload(10)).unwrap();
        assert_eq!(port.try_send(reload(11)), Err(Busy));
        assert_eq!(port.finish_reload(9), None);
        assert_eq!(port.finish_reload(10), Some(Some("new".into())));
        rx.recv().unwrap();
        port.try_send(reload(11)).unwrap();
    }

    #[test]
    fn oversized_marks_leave_the_control_queue_usable() {
        let (tx, rx) = flume::bounded(2);
        let port = CommandPort::new(tx);
        let mark = editor_control::WireMark {
            line: u32::MAX,
            kind: editor_control::WireMarkKind::Modified,
        };
        let maximum = Outgoing::GitMarks {
            request_id: u64::MAX,
            marks: vec![mark; MAX_GIT_MARKS],
        };
        assert!(editor_control::encode(&maximum.into_message()).is_ok());
        port.try_send(Outgoing::GitMarks {
            request_id: 0,
            marks: vec![mark; 200_000],
        })
        .unwrap();
        port.try_send(Outgoing::Reveal {
            request_id: 1,
            line: 10,
            column: None,
        })
        .unwrap();
        assert!(matches!(rx.recv().unwrap(), Outgoing::GitMarks { marks, .. } if marks.is_empty()));
        assert!(matches!(
            rx.recv().unwrap(),
            Outgoing::Reveal { request_id: 1, .. }
        ));
    }

    #[test]
    fn a_full_control_queue_answers_busy() {
        let (tx, _rx) = flume::bounded(1);
        let port = CommandPort::new(tx);
        port.try_send(Outgoing::GetState { request_id: 1 })
            .expect("first slot");
        assert_eq!(
            port.try_send(Outgoing::GetState { request_id: 2 }),
            Err(Busy),
            "a saturated queue refuses rather than dropping the request_id"
        );
    }

    #[test]
    fn retarget_coalesces_even_when_the_command_queue_is_full() {
        let (tx, rx) = flume::bounded(1);
        let port = CommandPort::new(tx);
        port.try_send(Outgoing::GetState { request_id: 1 }).unwrap();
        port.retarget("first.rs".into());
        port.retarget("last.rs".into());
        rx.recv().unwrap();
        assert_eq!(port.take_retarget().as_deref(), Some("last.rs"));
        assert_eq!(port.take_retarget(), None);
        port.retarget("idle.rs".into());
        assert!(rx.try_recv().is_ok(), "an idle writer must be woken");
        assert_eq!(port.take_retarget().as_deref(), Some("idle.rs"));
    }
}
