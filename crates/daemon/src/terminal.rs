//! Live terminal runtime and the per-terminal PTY read loop (§11.2, §11.4).
//!
//! One [`TerminalRuntime`] exists per live PTY, owned by the daemon core behind
//! its lock. A dedicated OS thread ([`pty_loop`]) reads 64 KiB buffers from the
//! master, feeds the authoritative engine under the lock, writes any device
//! replies back to the PTY, and emits a coalesced [`TerminalDelta`] to
//! subscribers at most every [`FRAME`] (≤125/s, §10.5). On EOF the child has
//! exited and the loop notifies the core.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use domain::{PtySize, SessionId, TerminalId};
use terminal_core::{AlacrittyEngine, DeltaBuilder, PtyHandle, PtyReader, TerminalEngine};

use crate::core::Daemon;

/// Minimum interval between emitted deltas (§10.5 coalescing: ~8 ms ⇒ ≤125/s).
pub const FRAME: Duration = Duration::from_millis(8);
const READ_BATCH_BUDGET: Duration = Duration::from_millis(2);
/// PTY read buffer size (§11.2).
const READ_BUF: usize = 64 * 1024;
/// Minimum interval between two activity notes for the same terminal (§10.4).
///
/// One note does two things: it sends `TerminalActivity` to non-subscribers for
/// their unread badge, and it advances the session's `last_activity_at` for the
/// idle policy. Both are coalesced to 1/s, and the clock lives here — on the
/// PTY thread that owns it — rather than in a map under the core lock, so a
/// chatty terminal no longer takes the daemon lock 125 times a second to
/// discover it has nothing to say.
const ACTIVITY_NOTE: Duration = Duration::from_secs(1);
/// Minimum interval between two `SessionUpdated` broadcasts carrying a bumped
/// `last_activity_at`. Keeps every client's idle clock within this of the
/// daemon's for a live session.
pub const ACTIVITY_BROADCAST: Duration = Duration::from_secs(30);

/// The write half of one PTY master, shared out of the core lock.
///
/// Writing to a PTY blocks once the kernel buffer fills — a child that stops
/// reading its stdin is enough. Holding the daemon's single `Mutex<Inner>`
/// across that write stalls every other client and every PTY thread, so the
/// writer lives behind its own lock: the core clones this handle, releases the
/// core lock, and only then writes (§9.3).
pub type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Write `bytes` to a PTY and flush, without holding the core lock.
pub fn write_pty(writer: &SharedWriter, bytes: &[u8]) -> std::io::Result<()> {
    let mut writer = writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    writer.write_all(bytes)?;
    writer.flush()
}

/// A live PTY child plus its authoritative engine (§7.4, §11.4).
pub struct TerminalRuntime {
    /// Reserved for diagnostics/stats (§22); the map key is the id at runtime.
    #[allow(dead_code)]
    pub id: TerminalId,
    pub session_id: SessionId,
    pub pty: Box<dyn PtyHandle>,
    /// Cloned out of the core lock before any write; see [`SharedWriter`].
    pub writer: SharedWriter,
    pub engine: AlacrittyEngine,
    pub delta_builder: DeltaBuilder,
    /// Per-terminal wire sequence, incremented once per *emitted* delta so
    /// deltas are consecutive for the client's `seq` gap check (§10.5) even
    /// though the engine feeds many byte-chunks between emissions.
    pub emit_seq: u64,
    pub last_emit: Option<Instant>,
    pub pending_damage: bool,
    pub size: PtySize,
    /// Reserved for diagnostics/stats (§22); killing uses `process_group`.
    #[allow(dead_code)]
    pub child_pid: u32,
    pub process_group: i32,
    /// Last terminal-reported (OSC 0/2) title we propagated to the session.
    pub last_title: Option<String>,
    /// Last time this terminal's `Session::last_activity_at` was broadcast.
    ///
    /// The field itself is bumped as often as the terminal is busy, but the
    /// domain event is throttled to [`ACTIVITY_BROADCAST`]: a client only needs
    /// its idle clock to be roughly right, and one `SessionUpdated` per delta
    /// would be 125 domain broadcasts a second per terminal.
    pub last_activity_broadcast: Instant,
}

impl TerminalRuntime {
    /// Feed PTY output into the engine, returning device-reply bytes and
    /// whether this chunk completed visible damage. Caller holds the core lock.
    ///
    /// Synchronized-output bodies return `false` until their closing sequence
    /// applies the buffered frame; the daemon must not turn those deliberately
    /// hidden intermediate chunks into deltas.
    pub fn feed(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let publish_damage = self.engine.feed(bytes);
        (self.engine.take_pty_writes(), publish_damage)
    }
}

pub(crate) struct PumpStatus {
    pub subscribed: bool,
    pub next_wake: Option<Instant>,
}

/// Drain bounded batches independently of the emission clock; block when there is no work.
pub fn pty_loop(daemon: Arc<Daemon>, terminal_id: TerminalId, mut reader: Box<dyn PtyReader>) {
    let mut buf = [0u8; READ_BUF];
    let mut deadline: Option<Instant> = None;
    let mut last_note = Instant::now() - ACTIVITY_NOTE;
    loop {
        let timeout = deadline.map(|end| end.saturating_duration_since(Instant::now()));
        let mut eof = false;
        let mut len = match reader.read_timeout(&mut buf, timeout) {
            Ok(Some(0)) => break,
            Ok(Some(len)) => len,
            Ok(None) => 0,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let batch_end = Instant::now() + READ_BATCH_BUDGET;
        while len > 0 && len < buf.len() && Instant::now() < batch_end {
            match reader.read_timeout(&mut buf[len..], Some(Duration::ZERO)) {
                Ok(Some(0)) => {
                    eof = true;
                    break;
                }
                Ok(Some(n)) => len += n,
                Ok(None) => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    eof = true;
                    break;
                }
            }
        }
        let note = len > 0 && last_note.elapsed() >= ACTIVITY_NOTE;
        if note {
            last_note = Instant::now();
        }
        deadline = daemon
            .pump_terminal_batch(terminal_id, &buf[..len], note, false, false)
            .next_wake;
        if eof {
            break;
        }
    }
    // EOF must publish the final batch even if its frame deadline has not arrived.
    daemon.pump_terminal_batch(terminal_id, &[], false, true, true);
    daemon.on_terminal_exited(terminal_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::ErrorKind;

    use domain::SpawnSpec;
    use terminal_core::PtyBackend;
    use test_support::FakePtyBackend;

    /// A PTY writer stand-in that records the bytes it received and how many
    /// times it was flushed.
    #[derive(Clone, Default)]
    struct Recorder {
        bytes: Arc<Mutex<Vec<u8>>>,
        flushes: Arc<Mutex<usize>>,
    }

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            *self.flushes.lock().unwrap() += 1;
            Ok(())
        }
    }

    /// A writer whose master has gone away, as when the child already exited.
    struct BrokenPipe;

    impl Write for BrokenPipe {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(ErrorKind::BrokenPipe, "master closed"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn shared(writer: impl Write + Send + 'static) -> SharedWriter {
        Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>))
    }

    #[test]
    fn the_frame_and_activity_budgets_are_bounded() {
        assert_eq!(FRAME, Duration::from_millis(8));
        assert_eq!(ACTIVITY_NOTE, Duration::from_secs(1));
        assert_eq!(ACTIVITY_BROADCAST, Duration::from_secs(30));
        assert!(ACTIVITY_BROADCAST > ACTIVITY_NOTE);
        assert_eq!(READ_BUF, 64 * 1024);
    }

    #[test]
    fn write_pty_writes_every_byte_and_flushes_once() {
        let recorder = Recorder::default();
        let writer = shared(recorder.clone());

        write_pty(&writer, b"ls -la\n").unwrap();
        write_pty(&writer, b"exit\n").unwrap();

        assert_eq!(&*recorder.bytes.lock().unwrap(), b"ls -la\nexit\n");
        assert_eq!(
            *recorder.flushes.lock().unwrap(),
            2,
            "input must not sit in a buffer waiting for the next keystroke"
        );
    }

    #[test]
    fn write_pty_surfaces_a_dead_master_to_the_caller() {
        let writer = shared(BrokenPipe);
        let err = write_pty(&writer, b"x").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    }

    /// The writer lock is taken outside the core lock by whichever thread has
    /// input to deliver. A panic while holding it must not turn every later
    /// keystroke into a panic of its own.
    #[test]
    fn a_poisoned_writer_lock_still_delivers_input() {
        let recorder = Recorder::default();
        let writer = shared(recorder.clone());

        let poisoner = Arc::clone(&writer);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("writer panic");
        })
        .join();
        assert!(writer.is_poisoned());

        write_pty(&writer, b"still here\n").unwrap();
        assert_eq!(&*recorder.bytes.lock().unwrap(), b"still here\n");
    }

    fn runtime(backend: &FakePtyBackend) -> TerminalRuntime {
        let size = PtySize {
            cols: 20,
            rows: 5,
            pixel_width: 0,
            pixel_height: 0,
        };
        let spec = SpawnSpec {
            program: "/bin/sh".into(),
            args: vec![],
            cwd: "/".into(),
            env: vec![],
        };
        let mut pty = backend.spawn(&spec, size).unwrap();
        let writer = Arc::new(Mutex::new(pty.writer()));
        TerminalRuntime {
            id: TerminalId::new(),
            session_id: SessionId::new(),
            pty,
            writer,
            engine: terminal_core::AlacrittyEngine::new(size),
            delta_builder: terminal_core::DeltaBuilder::new(),
            emit_seq: 0,
            last_emit: None,
            pending_damage: false,
            size,
            child_pid: 1,
            process_group: 1,
            last_title: None,
            last_activity_broadcast: Instant::now(),
        }
    }

    /// Ordinary output produces nothing to send back to the child.
    #[test]
    fn feeding_plain_output_asks_for_no_reply() {
        let backend = FakePtyBackend::empty();
        let mut rt = runtime(&backend);
        let (reply, publish_damage) = rt.feed(b"hello\r\n");
        assert!(reply.is_empty());
        assert!(publish_damage);
    }

    /// A device status report is answered by the engine, and `feed` hands those
    /// bytes back so the loop can write them to the PTY — a program that asks
    /// for the cursor position and never hears back typically hangs.
    #[test]
    fn feeding_a_device_query_returns_the_reply_to_write_back() {
        let backend = FakePtyBackend::empty();
        let mut rt = runtime(&backend);

        // ESC [ 6 n — report cursor position.
        let (reply, _) = rt.feed(b"\x1b[6n");
        assert_eq!(reply, b"\x1b[1;1R", "expected a CPR for the home position");

        // The reply is taken, not repeated on the next feed.
        assert!(rt.feed(b"x").0.is_empty());
    }

    #[test]
    fn synchronized_output_publishes_only_the_completed_frame() {
        let backend = FakePtyBackend::empty();
        let mut rt = runtime(&backend);

        // Entering the mode itself may publish the preceding frame, matching
        // alacritty_terminal's event loop. Its body is intentionally held.
        assert!(rt.feed(b"\x1b[?2026h").1);
        assert!(!rt.feed(b"first ").1);
        assert!(!rt.feed(b"second").1);

        // ESU applies the buffered body atomically and wakes the renderer once.
        assert!(rt.feed(b"\x1b[?2026l").1);
        let rendered: String = rt.engine.snapshot(0).visible[0]
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        assert!(
            rendered.starts_with("first second"),
            "completed synchronized frame was {rendered:?}"
        );
    }
}
