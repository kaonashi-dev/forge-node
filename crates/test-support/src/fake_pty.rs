//! A dependency-free fake [`PtyBackend`] for deterministic terminal tests
//! (§21 "Determinismo").
//!
//! Real PTYs spawn real children, whose timing and output are non-deterministic;
//! terminal-engine and session tests instead drive [`FakePtyBackend`], which
//! replays a preloaded byte script as the child's output, captures every byte
//! written to the PTY for assertions, and lets the test decide exactly when the
//! "child" exits. No process is spawned and no thread is created.
//!
//! ```
//! use test_support::fake_pty::FakePtyBackend;
//! use terminal_core::{PtyBackend, ExitStatus};
//! use domain::{PtySize, SpawnSpec};
//! use std::io::{Read, Write};
//!
//! let backend = FakePtyBackend::new(b"$ ".to_vec());
//! let spec = SpawnSpec { program: "/bin/sh".into(), args: vec![], cwd: "/".into(), env: vec![] };
//! let mut handle = backend.spawn(&spec, PtySize::default()).unwrap();
//!
//! // The scripted bytes are replayed as the child's output.
//! let mut out = Vec::new();
//! handle.reader().read_to_end(&mut out).unwrap();
//! assert_eq!(out, b"$ ");
//!
//! // Everything written to the PTY is captured.
//! handle.writer().write_all(b"exit\n").unwrap();
//! assert_eq!(backend.written(), b"exit\n");
//!
//! // The test controls termination.
//! assert_eq!(handle.try_wait().unwrap(), None);
//! backend.set_exited(ExitStatus { code: Some(0), signal: None });
//! assert!(handle.try_wait().unwrap().is_some());
//! ```

use std::io::{self, Cursor, Read, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use terminal_core::{ExitStatus, PtyBackend, PtyError, PtyHandle, PtyReader};

/// Default fake pid and process-group id, matching the value suggested in §17.
pub const DEFAULT_FAKE_PID: u32 = 424_242;

/// All mutable and configured state shared between a [`FakePtyBackend`] and the
/// [`FakePtyHandle`]s it spawns.
///
/// Both sides hold the same `Arc<Mutex<FakePtyState>>`, so a test can configure
/// or inspect the fake through whichever it still owns.
#[derive(Debug)]
struct FakePtyState {
    /// Bytes the fake child "emits" on the master; each `reader()` replays them.
    script: Vec<u8>,
    /// Everything written through `writer()`, captured for assertions.
    written: Vec<u8>,
    /// The most recent size passed to `resize()`.
    last_resize: Option<domain::PtySize>,
    /// The spec passed to the most recent `spawn()`.
    last_spawn: Option<domain::SpawnSpec>,
    /// Value reported by `child_pid()`.
    child_pid: u32,
    /// Value reported by `process_group()`.
    process_group: i32,
    /// Number of times `try_wait()` has been polled.
    poll_count: u32,
    /// If set, `try_wait()` reports an exit once `poll_count` reaches this bound.
    exit_after_polls: Option<u32>,
    /// The exit status reported once the child is considered terminated.
    exit_status: Option<ExitStatus>,
    /// Keep an empty reader open until the test marks the child exited.
    wait_for_exit: bool,
    /// Status synthesized when the poll bound trips (defaults to a clean exit).
    exit_on_poll_status: ExitStatus,
}

impl Default for FakePtyState {
    fn default() -> Self {
        Self {
            script: Vec::new(),
            written: Vec::new(),
            last_resize: None,
            last_spawn: None,
            child_pid: DEFAULT_FAKE_PID,
            process_group: DEFAULT_FAKE_PID as i32,
            poll_count: 0,
            exit_after_polls: None,
            exit_status: None,
            wait_for_exit: false,
            exit_on_poll_status: ExitStatus {
                code: Some(0),
                signal: None,
            },
        }
    }
}

/// Lock the shared state, panicking with a clear message on poisoning (a panic
/// while a guard was held). Acceptable in test-support code.
fn lock(state: &Mutex<FakePtyState>) -> MutexGuard<'_, FakePtyState> {
    state.lock().expect("fake pty state mutex poisoned")
}

/// A fake [`PtyBackend`] that replays a preloaded byte script and records input,
/// resizes, and termination for assertions (§21).
///
/// The backend and the handle it spawns share one state object, so control
/// methods ([`set_exited`](Self::set_exited)) and inspectors
/// ([`written`](Self::written), [`last_resize`](Self::last_resize)) work whether
/// called on the backend the test kept or on the handle it was handed.
#[derive(Debug, Clone)]
pub struct FakePtyBackend {
    state: Arc<Mutex<FakePtyState>>,
}

impl FakePtyBackend {
    /// Construct a backend whose spawned handle replays `script` as the child's
    /// output, then reports EOF.
    #[must_use]
    pub fn new(script: Vec<u8>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakePtyState {
                script,
                ..FakePtyState::default()
            })),
        }
    }

    /// Construct a backend whose child produces no output and remains alive
    /// until [`set_exited`](Self::set_exited) is called.
    #[must_use]
    pub fn empty() -> Self {
        let backend = Self::new(Vec::new());
        lock(&backend.state).wait_for_exit = true;
        backend
    }

    /// Set the pid reported by [`PtyHandle::child_pid`] (default
    /// [`DEFAULT_FAKE_PID`]).
    #[must_use]
    pub fn with_pid(self, pid: u32) -> Self {
        lock(&self.state).child_pid = pid;
        self
    }

    /// Set the process-group id reported by [`PtyHandle::process_group`]
    /// (default [`DEFAULT_FAKE_PID`]).
    #[must_use]
    pub fn with_process_group(self, pgid: i32) -> Self {
        lock(&self.state).process_group = pgid;
        self
    }

    /// Make [`PtyHandle::try_wait`] report `status` once it has been polled
    /// `polls` times, so a test can model a child that exits on its own without
    /// calling [`set_exited`](Self::set_exited).
    #[must_use]
    pub fn exit_after_polls(self, polls: u32, status: ExitStatus) -> Self {
        {
            let mut st = lock(&self.state);
            st.exit_after_polls = Some(polls);
            st.exit_on_poll_status = status;
        }
        self
    }

    /// Mark the child as exited with `status`; the next (and every later)
    /// [`PtyHandle::try_wait`] returns `Ok(Some(status))`.
    pub fn set_exited(&self, status: ExitStatus) {
        lock(&self.state).exit_status = Some(status);
    }

    /// A copy of every byte written to the PTY so far.
    #[must_use]
    pub fn written(&self) -> Vec<u8> {
        lock(&self.state).written.clone()
    }

    /// The most recent size passed to [`PtyHandle::resize`], if any.
    #[must_use]
    pub fn last_resize(&self) -> Option<domain::PtySize> {
        lock(&self.state).last_resize
    }

    /// The [`domain::SpawnSpec`] of the most recent [`spawn`](PtyBackend::spawn).
    #[must_use]
    pub fn last_spawn(&self) -> Option<domain::SpawnSpec> {
        lock(&self.state).last_spawn.clone()
    }

    /// How many times [`PtyHandle::try_wait`] has been polled.
    #[must_use]
    pub fn poll_count(&self) -> u32 {
        lock(&self.state).poll_count
    }
}

impl PtyBackend for FakePtyBackend {
    fn spawn(
        &self,
        spec: &domain::SpawnSpec,
        _size: domain::PtySize,
    ) -> Result<Box<dyn PtyHandle>, PtyError> {
        lock(&self.state).last_spawn = Some(spec.clone());
        Ok(Box::new(FakePtyHandle {
            state: Arc::clone(&self.state),
        }))
    }
}

/// A single fake PTY child, sharing state with the [`FakePtyBackend`] that
/// spawned it. The inherent inspector/control methods mirror the backend's so a
/// test that keeps the concrete handle (rather than the backend) can use them.
#[derive(Debug, Clone)]
pub struct FakePtyHandle {
    state: Arc<Mutex<FakePtyState>>,
}

impl FakePtyHandle {
    /// A copy of every byte written to this PTY so far.
    #[must_use]
    pub fn written(&self) -> Vec<u8> {
        lock(&self.state).written.clone()
    }

    /// The most recent size passed to [`resize`](PtyHandle::resize).
    #[must_use]
    pub fn last_resize(&self) -> Option<domain::PtySize> {
        lock(&self.state).last_resize
    }

    /// Mark the child as exited with `status` (see
    /// [`FakePtyBackend::set_exited`]).
    pub fn set_exited(&self, status: ExitStatus) {
        lock(&self.state).exit_status = Some(status);
    }

    /// How many times [`try_wait`](PtyHandle::try_wait) has been polled.
    #[must_use]
    pub fn poll_count(&self) -> u32 {
        lock(&self.state).poll_count
    }
}

impl PtyHandle for FakePtyHandle {
    fn reader(&mut self) -> Box<dyn PtyReader> {
        // A fresh cursor over the full script; valid to call more than once,
        // mirroring the real backend's cloned reader.
        let state = lock(&self.state);
        let script = state.script.clone();
        let wait_for_exit = state.wait_for_exit;
        drop(state);

        if wait_for_exit {
            Box::new(WaitingReader {
                state: Arc::clone(&self.state),
            })
        } else {
            Box::new(Cursor::new(script))
        }
    }

    fn writer(&mut self) -> Box<dyn Write + Send> {
        Box::new(CapturingWriter {
            state: Arc::clone(&self.state),
        })
    }

    fn resize(&mut self, size: domain::PtySize) -> Result<(), PtyError> {
        lock(&self.state).last_resize = Some(size);
        Ok(())
    }

    fn child_pid(&self) -> u32 {
        lock(&self.state).child_pid
    }

    fn process_group(&self) -> i32 {
        lock(&self.state).process_group
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError> {
        let mut st = lock(&self.state);
        st.poll_count += 1;
        if let Some(status) = st.exit_status {
            return Ok(Some(status));
        }
        if let Some(bound) = st.exit_after_polls {
            if st.poll_count >= bound {
                let status = st.exit_on_poll_status;
                st.exit_status = Some(status);
                return Ok(Some(status));
            }
        }
        Ok(None)
    }
}

/// Empty fake reader used by daemon tests. A real PTY does not emit EOF merely
/// because it has no output, so wait until the fake child is explicitly exited.
struct WaitingReader {
    state: Arc<Mutex<FakePtyState>>,
}

impl PtyReader for WaitingReader {
    fn read_timeout(
        &mut self,
        buffer: &mut [u8],
        timeout: Option<Duration>,
    ) -> io::Result<Option<usize>> {
        if let Some(timeout) = timeout {
            // The fake exit flag has no fd; production readers wait on the PTY descriptor.
            std::thread::sleep(timeout);
            return Ok(lock(&self.state).exit_status.map(|_| 0));
        }
        self.read(buffer).map(Some)
    }
}

impl Read for WaitingReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        loop {
            if lock(&self.state).exit_status.is_some() {
                return Ok(0);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A [`Write`] that appends everything written to the shared capture buffer.
struct CapturingWriter {
    state: Arc<Mutex<FakePtyState>>,
}

impl Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        lock(&self.state).written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{PtySize, SpawnSpec};

    fn spec() -> SpawnSpec {
        SpawnSpec {
            program: "/bin/sh".into(),
            args: Vec::new(),
            cwd: "/".into(),
            env: Vec::new(),
        }
    }

    #[test]
    fn reader_replays_scripted_bytes_then_eof() {
        let backend = FakePtyBackend::new(b"hello world".to_vec());
        let mut handle = backend.spawn(&spec(), PtySize::default()).unwrap();

        let mut out = Vec::new();
        handle.reader().read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello world");

        // A second reader replays the same script from the start.
        let mut again = Vec::new();
        handle.reader().read_to_end(&mut again).unwrap();
        assert_eq!(again, b"hello world");
    }

    #[test]
    fn writer_captures_input() {
        let backend = FakePtyBackend::empty();
        let mut handle = backend.spawn(&spec(), PtySize::default()).unwrap();
        {
            let mut w = handle.writer();
            w.write_all(b"ls -la\n").unwrap();
            w.flush().unwrap();
        }
        assert_eq!(backend.written(), b"ls -la\n");
    }

    #[test]
    fn resize_records_last_size() {
        let backend = FakePtyBackend::empty();
        let mut handle = backend.spawn(&spec(), PtySize::default()).unwrap();
        let size = PtySize {
            cols: 120,
            rows: 40,
            pixel_width: 8,
            pixel_height: 16,
        };
        handle.resize(size).unwrap();
        assert_eq!(backend.last_resize(), Some(size));
    }

    #[test]
    fn try_wait_flips_after_set_exited() {
        let backend = FakePtyBackend::empty();
        let mut handle = backend.spawn(&spec(), PtySize::default()).unwrap();

        assert_eq!(handle.try_wait().unwrap(), None);
        let status = ExitStatus {
            code: Some(0),
            signal: None,
        };
        backend.set_exited(status);
        assert_eq!(handle.try_wait().unwrap(), Some(status));
        // Stays exited on subsequent polls.
        assert_eq!(handle.try_wait().unwrap(), Some(status));
    }

    #[test]
    fn try_wait_exits_after_configured_polls() {
        let status = ExitStatus {
            code: Some(3),
            signal: None,
        };
        let backend = FakePtyBackend::empty().exit_after_polls(2, status);
        let mut handle = backend.spawn(&spec(), PtySize::default()).unwrap();

        assert_eq!(handle.try_wait().unwrap(), None);
        assert_eq!(handle.try_wait().unwrap(), Some(status));
    }

    #[test]
    fn reports_configured_and_default_ids() {
        let default = FakePtyBackend::empty();
        let h = default.spawn(&spec(), PtySize::default()).unwrap();
        assert_eq!(h.child_pid(), DEFAULT_FAKE_PID);
        assert_eq!(h.process_group(), DEFAULT_FAKE_PID as i32);

        let custom = FakePtyBackend::empty().with_pid(7).with_process_group(-7);
        let h = custom.spawn(&spec(), PtySize::default()).unwrap();
        assert_eq!(h.child_pid(), 7);
        assert_eq!(h.process_group(), -7);
    }

    #[test]
    fn records_last_spawn_spec() {
        let backend = FakePtyBackend::empty();
        let _ = backend.spawn(&spec(), PtySize::default()).unwrap();
        assert_eq!(backend.last_spawn().unwrap().program, spec().program);
    }
}
