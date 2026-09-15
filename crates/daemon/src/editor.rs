//! Supervisor for one integrated editor session (feature 19).
//!
//! The daemon binds a private Unix socket **before** it spawns
//! `forge-editor`; the editor connects after spawn (`portable-pty` closes
//! every inherited fd over 2, so the path travels in the environment), performs
//! a versioned handshake, and takes the buffer. The PTY carries the editing
//! stream; this channel carries the buffer and its metadata, so the daemon's
//! view of path, dirty and position is never parsed out of ANSI.
//!
//! Everything that can block — accept, handshake, socket IO — runs on this
//! module's thread, never under the core lock. `Supervisor`'s `Drop` removes
//! the socket file on every path, failure included, because it lives inside the
//! service thread until that thread ends.

use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{EditorState, SessionId};
use editor_control::{
    read_frame, version_matches, write_frame, ControlError, DaemonMessage, EditorMessage,
    EditorStateWire, CONTROL_VERSION,
};

use crate::core::Daemon;

/// How long the daemon waits for the editor to connect, handshake and take the
/// buffer. A spawn that never reaches the handshake is a failed session, not a
/// hang.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounded command queue toward one editor. Full means busy, never a drop.
pub const COMMAND_QUEUE: usize = 8;

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
        }
    }
}

/// The live sender for one editor's command queue.
#[derive(Clone, Debug)]
pub struct CommandPort {
    tx: flume::Sender<Outgoing>,
}

impl CommandPort {
    #[must_use]
    pub fn new(tx: flume::Sender<Outgoing>) -> Self {
        Self { tx }
    }

    /// Enqueue, or say busy. Never drops a `request_id` on the floor.
    pub fn try_send(&self, command: Outgoing) -> Result<(), Busy> {
        self.tx.try_send(command).map_err(|_| Busy)
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

/// One socket per editor session, under the daemon's runtime dir.
///
/// The full session UUID usually fits; when the runtime dir is long (macOS
/// `$TMPDIR`), the hex is truncated just enough to stay under
/// [`crate::paths::MAX_SOCKET_PATH_LEN`]. A stale file is unlinked before
/// binding, like the daemon's own socket.
pub fn control_socket_path(session_id: SessionId) -> io::Result<PathBuf> {
    let dir = crate::paths::resolve_runtime_dir().map_err(|e| io::Error::other(e.to_string()))?;
    crate::paths::ensure_private_dir(&dir).map_err(|e| io::Error::other(e.to_string()))?;
    let hex: String = session_id
        .to_string()
        .chars()
        .filter(|c| *c != '-')
        .collect();
    for keep in [hex.len(), 16, 12] {
        let suffix = &hex[..keep.min(hex.len())];
        let path = dir.join(format!("editor-{suffix}.sock"));
        if path.as_os_str().len() < crate::paths::MAX_SOCKET_PATH_LEN {
            return Ok(path);
        }
    }
    Err(io::Error::other(
        "the control socket path does not fit sun_path in any runtime directory",
    ))
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
        buffer: fs_service::FileContents,
        line: Option<u32>,
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
                serve(daemon, session_id, listener, buffer, line, commands);
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
    buffer: fs_service::FileContents,
    line: Option<u32>,
    commands: flume::Receiver<Outgoing>,
) {
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
    if let Err(error) = handshake(&mut stream, session_id, &buffer, line) {
        fail(
            &daemon,
            session_id,
            format!("control handshake failed: {error}"),
        );
        return;
    }
    // The buffer is open; the editor is usable.
    daemon.set_editor_running(session_id);

    // Outgoing requests share the socket via a cloned fd; the read loop below
    // owns the original. A full `commands` channel is refused at enqueue.
    if let Ok(mut writer) = stream.try_clone() {
        std::thread::Builder::new()
            .name("forge-editor-cmd".to_owned())
            .spawn(move || {
                while let Ok(command) = commands.recv() {
                    if write_frame(&mut writer, &command.into_message()).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the editor command writer");
    }

    let mut last_broadcast: Option<Instant> = None;
    let mut pending: Option<EditorStateWire> = None;
    loop {
        let wait = pending.as_ref().map(|_| {
            let since = last_broadcast.unwrap_or_else(Instant::now);
            STATE_BROADCAST_COOLDOWN
                .checked_sub(since.elapsed())
                .unwrap_or(Duration::ZERO)
        });
        let _ = stream.set_read_timeout(wait);
        match read_frame::<EditorMessage>(&mut stream) {
            Ok(EditorMessage::State { state, .. }) => {
                pending = Some(state);
                if due(last_broadcast) {
                    flush(&daemon, session_id, pending.take());
                    last_broadcast = Some(Instant::now());
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
                    flush(&daemon, session_id, Some(state));
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

/// Read the editor's `Hello`, answer `Welcome`, and hand over the buffer.
///
/// The handshake is the one place a request id is minted by the daemon; the
/// editor's `Opened` echoes it. H1 opens every buffer read-only (design D6).
fn handshake(
    stream: &mut UnixStream,
    session_id: SessionId,
    buffer: &fs_service::FileContents,
    line: Option<u32>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
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
            read_only: true,
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
fn flush(daemon: &Arc<Daemon>, session_id: SessionId, state: Option<EditorStateWire>) {
    let Some(state) = state else { return };
    let session = daemon.record_editor_state(
        session_id,
        EditorState {
            path: state.path,
            line: state.line,
            column: state.column,
            dirty: state.dirty,
            read_only: state.read_only,
            document_version: state.document_version,
        },
    );
    if let Some(session) = session {
        daemon
            .registry
            .broadcast_domain(protocol::DaemonEvent::SessionUpdated(session));
    }
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
        let error = handshake(&mut daemon_side, id, &buffer(), None).expect_err("mismatch");
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
}
