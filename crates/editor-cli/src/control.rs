//! The integrated control channel: handshake, open, and the traffic that keeps
//! the daemon's view of the buffer honest.
//!
//! One connection per editor. The daemon creates the socket before spawn and
//! the editor joins it after; the PTY carries keys and pixels while this
//! carries the buffer and its metadata, so state is never parsed out of ANSI.
//!
//! The wait is blocking on both the tty and this channel (the main loop's
//! `flume::Selector`); a reader thread owns the socket because the main thread
//! also writes to it.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use editor_control::{
    read_frame, version_matches, write_frame, DaemonMessage, EditorMessage, CONTROL_VERSION,
};

/// How long the editor waits for `Welcome`. The daemon has its own handshake
/// timeout; this one only keeps a half-open socket from hanging the process.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Capacity of the channel the reader thread fills. Control messages are rare
/// next to keystrokes; a full queue means the main loop is stuck.
const CONTROL_QUEUE: usize = 64;

/// What the daemon said in the handshake and the open exchange.
pub struct Opened {
    /// The `Open` request's id, echoed by the editor's `Opened` answer.
    pub request_id: u64,
    /// The daemon's name for this buffer, echoed on every `ViewFrame` so a
    /// frame from a replaced session cannot be painted into the live one.
    pub buffer_id: u64,
    pub path: String,
    pub text: String,
    pub revision: Option<String>,
    pub line: Option<u32>,
    pub read_only: bool,
    /// Save on a pause, as the opener asked.
    pub autosave: bool,
}

/// What the reader thread delivers: a request from the daemon, or the socket
/// going away.
#[derive(Clone, Debug)]
pub enum Incoming {
    Message(DaemonMessage),
    Closed { reason: String },
}

pub struct ControlChannel {
    writer: UnixStream,
    incoming: flume::Receiver<Incoming>,
}

impl ControlChannel {
    /// Connect, handshake, and take the `Open` the daemon sends.
    ///
    /// # Errors
    /// [`anyhow::Error`] when the socket does not accept, the handshake times
    /// out or its version/identity disagree, or the first `Open` is missing.
    pub fn connect(socket: &Path) -> Result<(Self, Opened)> {
        let session_id = std::env::var("FORGE_SESSION_ID").unwrap_or_default();
        let mut stream = UnixStream::connect(socket)
            .with_context(|| format!("control socket {}", socket.display()))?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        write_frame(
            &mut stream,
            &EditorMessage::Hello {
                version: CONTROL_VERSION,
                session_id: session_id.clone(),
                pid: std::process::id(),
            },
        )?;
        match read_frame::<DaemonMessage>(&mut stream)? {
            DaemonMessage::Welcome {
                version,
                session_id: welcome,
                ..
            } => {
                if !version_matches(version) {
                    bail!(
                        "control version mismatch: daemon speaks {version}, this editor speaks {CONTROL_VERSION}"
                    );
                }
                if welcome != session_id {
                    bail!("control session mismatch: daemon named {welcome}, this is {session_id}");
                }
            }
            other => bail!("expected Welcome, got {other:?}"),
        }
        let opened = match read_frame::<DaemonMessage>(&mut stream)? {
            DaemonMessage::Open {
                request_id,
                buffer_id,
                path,
                text,
                revision,
                line,
                read_only,
                autosave,
            } => Opened {
                request_id,
                buffer_id,
                path,
                text,
                revision,
                line,
                read_only,
                autosave,
            },
            other => bail!("expected Open, got {other:?}"),
        };
        stream.set_read_timeout(None)?;

        let (sender, incoming) = flume::bounded(CONTROL_QUEUE);
        let mut reader = stream.try_clone()?;
        thread::spawn(move || loop {
            match read_frame::<DaemonMessage>(&mut reader) {
                Ok(message) => {
                    if sender.send(Incoming::Message(message)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Incoming::Closed {
                        reason: error.to_string(),
                    });
                    break;
                }
            }
        });
        Ok((
            Self {
                writer: stream,
                incoming,
            },
            opened,
        ))
    }

    #[must_use]
    pub fn incoming(&self) -> &flume::Receiver<Incoming> {
        &self.incoming
    }

    /// Send one message; blocks only as long as the socket write does.
    pub fn send(&mut self, message: &EditorMessage) -> Result<()> {
        write_frame(&mut self.writer, message)?;
        self.writer.flush()?;
        Ok(())
    }
}
