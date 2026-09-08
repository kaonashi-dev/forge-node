//! PTY backend abstraction and a `portable-pty` implementation (§11.2).
//!
//! The daemon owns every PTY (ADR-005). This module defines the two traits the
//! rest of the daemon programs against — [`PtyBackend`] (a factory) and
//! [`PtyHandle`] (one live child) — plus [`PortablePtyBackend`], the default
//! implementation built on the `portable-pty` crate.
//!
//! ## §11.2 substitution criteria
//!
//! The plan keeps `portable-pty` only if it can (a) `setsid` + acquire a
//! controlling TTY, (b) expose the child's process-group id, (c) apply
//! `TIOCSWINSZ` *including pixel size*, and (d) close inherited fds. Reading the
//! 0.9 unix backend, all four hold:
//!
//! - **(a)** `spawn_command` runs `libc::setsid()` and `ioctl(0, TIOCSCTTY)` in
//!   the child's `pre_exec` (controlling-TTY defaults to `true`).
//! - **(b)** [`MasterPty::process_group_leader`] returns `tcgetpgrp(master)`;
//!   because the child called `setsid`, it is its own session/group leader, so
//!   at spawn `pgid == child_pid`. See [`PtyHandle::process_group`].
//! - **(c)** `openpty`/`resize` fill `winsize.ws_xpixel`/`ws_ypixel` from
//!   [`PtySize::pixel_width`]/`pixel_height` and issue `TIOCSWINSZ` (§7.6).
//! - **(d)** master and slave fds are set `FD_CLOEXEC`, and `close_random_fds()`
//!   runs in `pre_exec`, so no daemon fds leak into the child.
//!
//! Therefore `portable-pty` is retained; no custom `nix`/`rustix` backend is
//! needed for the MVP.

use std::io::{Read, Write};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtyPair};

/// How a child terminated, as reported by [`PtyHandle::try_wait`].
///
/// Exactly one of `code`/`signal` is normally `Some`. Note that `portable-pty`
/// only surfaces a *signal name* (from `strsignal`), not the numeric signal, so
/// `signal` is a best-effort parse and is usually `None` for signal-terminated
/// children; the daemon's kill path (§11.3) reaps with `nix::waitpid` when it
/// needs the precise numeric signal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitStatus {
    /// Normal-exit status code, if the child exited rather than being signalled.
    pub code: Option<i32>,
    /// Terminating signal number, when known.
    pub signal: Option<i32>,
}

/// Errors raised while opening, spawning into, or driving a PTY.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PtyError {
    /// The PTY pair could not be opened.
    #[error("failed to open pty: {0}")]
    Open(String),
    /// The child process could not be spawned into the slave side.
    #[error("failed to spawn child: {0}")]
    Spawn(String),
    /// A `TIOCSWINSZ` resize failed.
    #[error("failed to resize pty: {0}")]
    Resize(String),
    /// Reading, writing, or waiting on the PTY failed.
    #[error("pty io error: {0}")]
    Io(#[from] std::io::Error),
    /// Any other backend-specific failure.
    #[error("pty backend error: {0}")]
    Backend(String),
}

/// Factory for PTY-backed child processes (§11.2).
pub trait PtyBackend: Send + Sync {
    /// Open a PTY sized to `size` and spawn `spec` into its slave side.
    ///
    /// # Errors
    /// Returns [`PtyError`] if the pair cannot be opened or the child cannot be
    /// spawned.
    fn spawn(
        &self,
        spec: &domain::SpawnSpec,
        size: domain::PtySize,
    ) -> Result<Box<dyn PtyHandle>, PtyError>;
}

/// A single live PTY child (§11.2).
///
/// The PTY read loop runs on a dedicated blocking thread that reads 64 KiB
/// buffers from [`PtyHandle::reader`] and feeds the engine (§11.2); this trait
/// only exposes the handles and control operations.
pub trait PtyHandle: Send {
    /// A fresh readable handle for the child's output. Cloned from the master,
    /// so it is valid to call more than once.
    fn reader(&mut self) -> Box<dyn Read + Send>;
    /// The writable handle for the child's input. Valid to take only once.
    fn writer(&mut self) -> Box<dyn Write + Send>;
    /// Apply a new window size (including pixel size) via `TIOCSWINSZ`.
    ///
    /// # Errors
    /// Returns [`PtyError::Resize`] if the ioctl fails.
    fn resize(&mut self, size: domain::PtySize) -> Result<(), PtyError>;
    /// PID of the spawned child.
    fn child_pid(&self) -> u32;
    /// Process-group id of the child (the group leader after `setsid`, §11.3).
    fn process_group(&self) -> i32;
    /// Poll whether the child has exited, without blocking.
    ///
    /// # Errors
    /// Returns [`PtyError::Io`] if the underlying wait fails.
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError>;
}

/// Default [`PtyBackend`], built on `portable-pty`'s native system.
#[derive(Debug, Default, Clone, Copy)]
pub struct PortablePtyBackend;

impl PortablePtyBackend {
    /// Construct the backend.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Translate the shared [`domain::PtySize`] into `portable-pty`'s size type,
/// preserving pixel dimensions (§7.6).
fn to_pp_size(size: domain::PtySize) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: size.pixel_width,
        pixel_height: size.pixel_height,
    }
}

fn to_exit_status(status: &portable_pty::ExitStatus) -> ExitStatus {
    match status.signal() {
        // `portable-pty` gives a human-readable signal *name* (via strsignal),
        // not the number; parse only if it happens to be numeric.
        Some(name) => ExitStatus {
            code: None,
            signal: name.parse::<i32>().ok(),
        },
        None => ExitStatus {
            code: Some(status.exit_code() as i32),
            signal: None,
        },
    }
}

impl PtyBackend for PortablePtyBackend {
    fn spawn(
        &self,
        spec: &domain::SpawnSpec,
        size: domain::PtySize,
    ) -> Result<Box<dyn PtyHandle>, PtyError> {
        let pty_system = native_pty_system();
        let PtyPair { master, slave } = pty_system
            .openpty(to_pp_size(size))
            .map_err(|e| PtyError::Open(e.to_string()))?;

        // Build a *complete* environment from the resolved SpawnSpec (§7.6):
        // clear the inherited base env, then set exactly what the spec carries.
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        cmd.env_clear();
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        let child = slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        // Drop the slave in the parent so the master observes EOF once the
        // child (and all its fd inheritors) exit.
        drop(slave);

        let child_pid = child.process_id().unwrap_or(0);
        #[allow(clippy::cast_possible_wrap)] // a pid always fits in i32 on unix
        let child_pgid = child_pid as i32;
        // Resolved once, here, and never again: see `PortablePtyHandle::process_group`.
        #[cfg(unix)]
        let process_group = master.process_group_leader().unwrap_or(child_pgid);
        #[cfg(not(unix))]
        let process_group = child_pgid;

        Ok(Box::new(PortablePtyHandle {
            master,
            child,
            child_pid,
            process_group,
        }))
    }
}

struct PortablePtyHandle {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    child_pid: u32,
    /// The child's own process-group id, captured at spawn (§11.2 criterion b).
    process_group: i32,
}

impl PtyHandle for PortablePtyHandle {
    fn reader(&mut self) -> Box<dyn Read + Send> {
        // `try_clone_reader` dups the master fd; the signature is infallible per
        // §11.2, so a clone failure (fd exhaustion) is unrecoverable here.
        self.master
            .try_clone_reader()
            .expect("clone pty reader from master fd")
    }

    fn writer(&mut self) -> Box<dyn Write + Send> {
        self.master.take_writer().expect("take pty writer once")
    }

    fn resize(&mut self, size: domain::PtySize) -> Result<(), PtyError> {
        self.master
            .resize(to_pp_size(size))
            .map_err(|e| PtyError::Resize(e.to_string()))
    }

    fn child_pid(&self) -> u32 {
        self.child_pid
    }

    fn process_group(&self) -> i32 {
        // The value captured at spawn, not a fresh `tcgetpgrp`.
        //
        // `MasterPty::process_group_leader` reports the terminal's *foreground*
        // process group, which only equals the child's pgid until the child
        // hands the foreground to a job of its own: run `vim` in a session shell
        // and a later call returns vim's pgid instead. `KillSession` signals
        // `-pgid` (§11.3), so re-reading it here would have killed the
        // foreground job and left the shell — and its other children — running.
        // The child called `setsid`, so its pgid equals its pid for its whole
        // life and one read at spawn is both correct and stable.
        self.process_group
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError> {
        Ok(self.child.try_wait()?.map(|s| to_exit_status(&s)))
    }
}
