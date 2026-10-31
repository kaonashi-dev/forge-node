//! Running `git` as a plain, captured subprocess (ADR-008).
//!
//! Local Git invocations go through [`run_git`] or [`run_git_bounded`], which:
//! - prepends `-C <repo>` when a working directory is given,
//! - forces a stable, parseable locale (`LC_ALL=C`),
//! - disables interactive credential/prompt hangs (`GIT_TERMINAL_PROMPT=0`),
//! - captures stdout, stderr and the exit status,
//! - enforces the ADR-008 per-command timeout of **30 s** (a worker thread plus
//!   [`std::sync::mpsc::Receiver::recv_timeout`]; on timeout the child is killed
//!   and [`GitError::Timeout`] is returned).
//!
//! Git is **never** run inside a user PTY (ADR-008): this is a background
//! subprocess whose output is returned as [`GitOutput`].
//!
//! ## Network commands: a documented deviation from ADR-008
//!
//! ADR-008 fixes one timeout for every git command. That number was chosen for
//! *local* commands, where 30 s means "something is wrong"; for `fetch` it
//! means "this monorepo is large". A single constant cannot serve both, so
//! [`run_git_network`] exists with its own, longer, caller-supplied budget.
//!
//! It also hardens two things `run_git` cannot:
//!
//! - **`GIT_TERMINAL_PROMPT=0` does not reach `ssh`.** It stops git's own
//!   credential prompt, but `ssh` reads the TTY directly, so an encrypted key
//!   with no agent loaded, or an unknown host key, blocks until the timeout.
//!   [`run_git_network`] sets `GIT_SSH_COMMAND` with `BatchMode=yes` so `ssh`
//!   fails immediately and says why instead of hanging.
//! - **Askpass helpers.** `GIT_ASKPASS`/`SSH_ASKPASS` are pointed at a command
//!   that fails, and `SSH_ASKPASS_REQUIRE=never`, so a desktop credential
//!   dialog can never be spawned by a daemon that has no window to show it in.
//!
//! The result is that a network command fails *fast and legibly* — the stderr
//! reaching the GUI says `Permission denied (publickey)`, not `Timeout`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use thiserror::Error;

/// Per-command timeout mandated by ADR-008, for local commands.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default budget for a command that talks to a remote (see the module docs).
///
/// Four times [`GIT_TIMEOUT`], because the first `fetch` of a large repository
/// legitimately takes minutes and a user who waits for it is not looking at a
/// bug. Callers may pass their own.
pub const GIT_NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

/// `ssh` options forced on every network command: never prompt, never hang.
///
/// `accept-new` trusts a host the first time and pins it afterwards, which is
/// the only setting that both works on a fresh machine and still detects a key
/// change later. `BatchMode=yes` turns every would-be prompt into an immediate
/// failure with a real message.
const SSH_BATCH_COMMAND: &str =
    "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10";

/// The captured result of a `git` invocation that ran to completion.
///
/// A non-zero [`status`](GitOutput::status) is still a `GitOutput`: callers
/// decide whether that is an error (see [`GitOutput::ok`]) or an expected
/// signal (e.g. `rev-parse --show-toplevel` failing means "not a repo").
#[derive(Clone, Debug)]
pub struct GitOutput {
    /// Captured standard output, decoded lossily as UTF-8.
    pub stdout: String,
    /// Captured standard error, decoded lossily as UTF-8.
    pub stderr: String,
    /// Process exit code, or `-1` if the process was terminated by a signal.
    pub status: i32,
}

impl GitOutput {
    /// Whether the process exited with status `0`.
    #[must_use]
    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// `stdout` with surrounding whitespace (including the trailing newline)
    /// removed — the common case for single-value queries.
    #[must_use]
    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }

    /// Return `self` if the command succeeded, otherwise a
    /// [`GitError::CommandFailed`] carrying `args` and the captured stderr.
    ///
    /// # Errors
    /// [`GitError::CommandFailed`] when [`status`](GitOutput::status) is non-zero.
    pub fn ok(self, args: &[&str]) -> Result<Self, GitError> {
        if self.success() {
            Ok(self)
        } else {
            Err(GitError::CommandFailed {
                args: args.iter().map(|s| (*s).to_string()).collect(),
                stderr: self.stderr.trim().to_string(),
                status: self.status,
            })
        }
    }
}

/// Errors produced by the git-service crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GitError {
    /// `rev-parse --show-toplevel` failed: the path is not inside a repository.
    #[error("not a git repository: {0}")]
    NotAGitRepo(PathBuf),

    /// A git command ran but exited with a non-zero status.
    #[error("git {args:?} failed (status {status}): {stderr}")]
    CommandFailed {
        args: Vec<String>,
        stderr: String,
        status: i32,
    },

    /// The command exceeded its budget and was killed.
    #[error("git command timed out after {}s", timeout.as_secs())]
    Timeout {
        /// Wall-clock budget that elapsed, in the same unit the caller passed.
        timeout: Duration,
    },

    /// `check-ref-format --branch` rejected the branch name.
    #[error("invalid branch name {branch:?}: {reason}")]
    InvalidBranchName { branch: String, reason: String },

    /// The requested branch is already checked out in another worktree.
    #[error("branch {branch:?} is already checked out at {path}")]
    Conflict { branch: String, path: PathBuf },

    /// A path argument could not be represented as UTF-8 for the CLI.
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),

    /// [`run_git_bounded`] killed the command when its output passed `limit`.
    #[error("git output exceeds {limit}-byte capture budget")]
    OutputLimit { limit: usize },

    /// An I/O error while spawning or communicating with `git`.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolve the git-managed file or directory named `name` for the worktree at
/// `dir`, via `rev-parse --git-path`.
///
/// The indirection matters for a linked worktree: its `MERGE_HEAD` and its
/// `rebase-merge/` live under `.git/worktrees/<name>/`, not in the repository's
/// own git dir, and only git knows which. Returns `None` when git cannot answer
/// or the path does not exist.
pub(crate) fn git_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let out = run_git(Some(dir), &["rev-parse", "--git-path", name]).ok()?;
    if !out.success() {
        return None;
    }
    let reported = out.stdout_trimmed();
    if reported.is_empty() {
        return None;
    }
    let reported = Path::new(reported);
    let full = if reported.is_absolute() {
        reported.to_path_buf()
    } else {
        dir.join(reported)
    };
    full.exists().then_some(full)
}

/// Run `git` with the given arguments, optionally inside `repo` (via `-C`).
///
/// The returned [`GitOutput`] is produced for **any** completed process,
/// including non-zero exits; only spawn/I-O failures and timeouts become an
/// `Err`. See [`GitOutput::ok`] for turning a non-zero exit into an error.
///
/// # Errors
/// - [`GitError::Io`] if the `git` binary cannot be spawned or the worker
///   thread disconnects unexpectedly.
/// - [`GitError::Timeout`] if the command runs longer than [`GIT_TIMEOUT`]; the
///   child process is sent `SIGKILL` before returning.
pub fn run_git(repo: Option<&Path>, args: &[&str]) -> Result<GitOutput, GitError> {
    run_git_inner(repo, args, GIT_TIMEOUT, false, Capture::All).map(|(output, _)| output)
}

/// Most a bounded capture keeps of either stream.
pub(crate) const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;

/// Stderr kept by [`run_git_prefix`], whatever its stdout limit.
const PREFIX_STDERR_BYTES: usize = 64 * 1024;

/// Refuse stdout or stderr over `limit` bytes, killing and reaping the command group.
///
/// # Errors
/// As [`run_git`], plus [`GitError::OutputLimit`] when either stream passes
/// `limit` (itself capped at 8 MiB).
pub fn run_git_bounded(
    repo: Option<&Path>,
    args: &[&str],
    limit: usize,
) -> Result<GitOutput, GitError> {
    run_git_inner(
        repo,
        args,
        GIT_TIMEOUT,
        false,
        Capture::Refuse(limit.min(MAX_CAPTURE_BYTES)),
    )
    .map(|(output, _)| output)
}

/// Keep the first `limit` bytes of stdout, and kill and reap the command group
/// once it passes them.
///
/// Returns whether anything was cut. A cut command was killed, so its status
/// says nothing about the prefix; a caller reads `cut` first. Stderr has its
/// own cap, so a small `limit` is not spent on a warning; passing that cap
/// also cuts, because the command is killed with its stdout unfinished.
///
/// # Errors
/// As [`run_git`].
pub fn run_git_prefix(
    repo: Option<&Path>,
    args: &[&str],
    limit: usize,
) -> Result<(GitOutput, bool), GitError> {
    run_git_inner(
        repo,
        args,
        GIT_TIMEOUT,
        false,
        Capture::Prefix(limit.min(MAX_CAPTURE_BYTES)),
    )
}

/// How much of a command's output is held.
#[derive(Clone, Copy)]
enum Capture {
    All,
    Refuse(usize),
    Prefix(usize),
}

/// Run a `git` command that talks to a remote, with its own timeout and with
/// every interactive prompt turned into an immediate failure.
///
/// Use this for `fetch`, `ls-remote`, `push` — anything that opens a socket.
/// Local commands keep [`run_git`] and the ADR-008 budget. See the module docs
/// for why the two differ.
///
/// # Errors
/// - [`GitError::Io`] if the `git` binary cannot be spawned.
/// - [`GitError::Timeout`] if the command outlives `timeout`; the child is sent
///   `SIGKILL` before returning.
pub fn run_git_network(
    repo: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<GitOutput, GitError> {
    run_git_inner(
        repo,
        args,
        timeout,
        true,
        Capture::Refuse(MAX_CAPTURE_BYTES),
    )
    .map(|(output, _)| output)
}

fn run_git_inner(
    repo: Option<&Path>,
    args: &[&str],
    timeout: Duration,
    network: bool,
    capture: Capture,
) -> Result<(GitOutput, bool), GitError> {
    use std::os::unix::process::CommandExt as _;

    let mut cmd = Command::new("git");
    if let Some(dir) = repo {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args)
        // Stable, machine-parseable output regardless of the user's locale.
        .env("LC_ALL", "C")
        // Never block on a credential/password prompt (ADR-008).
        .env("GIT_TERMINAL_PROMPT", "0")
        // A sequencer command (`rebase --continue`, `merge --continue`) opens
        // `$EDITOR` for the commit message it already has. With stdin closed
        // that editor would sit there until the timeout killed it, so both
        // editor hooks are pinned to `true`: it exits 0 and git keeps the
        // message. No application command here ever wants an editor.
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A separate process group so a timeout can terminate the *whole* tree
        // (P2). `wait_with_output` drains the pipes to EOF before it reaps, and
        // a network `git` forks `ssh` / `git-remote-https` / a credential helper
        // that inherited the write ends: killing git's pid alone leaves those
        // holding the pipes open, so the worker thread blocks in `read` forever,
        // two fds leak, and the process that was actually hanging keeps running.
        // `run_gh` already does exactly this.
        .process_group(0);

    if network {
        // `GIT_TERMINAL_PROMPT` does not reach `ssh`; these do.
        cmd.env("GIT_SSH_COMMAND", SSH_BATCH_COMMAND)
            // A helper that exits non-zero is how you say "no credentials" to
            // git without it falling back to the terminal.
            .env("GIT_ASKPASS", "/usr/bin/false")
            .env("SSH_ASKPASS", "/usr/bin/false")
            .env("SSH_ASKPASS_REQUIRE", "never")
            // Do not let a repo-local config re-enable an interactive helper.
            .env("GIT_CONFIG_PARAMETERS", "'credential.interactive=never'");
    }

    tracing::debug!(target: "git", ?repo, ?args, network, "run git");

    let child = cmd.spawn()?;
    let pid = child.id();

    // Wait (and drain the pipes) on a worker thread so the main thread can
    // enforce a wall-clock timeout with `recv_timeout` (std has none built in).
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(match capture {
            Capture::All => child.wait_with_output().map(|output| (output, false)),
            Capture::Refuse(limit) => bounded_output(child, limit, limit, false),
            Capture::Prefix(limit) => bounded_output(child, limit, PREFIX_STDERR_BYTES, true),
        });
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok((output, cut))) => Ok((
            GitOutput {
                stdout: decode(output.stdout),
                stderr: decode(output.stderr),
                status: output.status.code().unwrap_or(-1),
            },
            cut,
        )),
        Ok(Err(e)) => Err(match e.get_ref().and_then(|inner| inner.downcast_ref()) {
            Some(&OverLimit(limit)) => GitError::OutputLimit { limit },
            None => GitError::Io(e),
        }),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_process_group(pid);
            // Do not return while the worker still owns an unreaped child: the
            // SIGKILL closes the pipes, so `wait_with_output` returns almost at
            // once. A bounded wait keeps a wedged descendant (uninterruptible
            // sleep, or one that escaped the group with its own `setsid`) from
            // blocking the caller indefinitely.
            let _ = rx.recv_timeout(KILL_REAP_GRACE);
            Err(GitError::Timeout { timeout })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(GitError::Io(std::io::Error::other(
            "git worker thread disconnected before reporting a result",
        ))),
    }
}

/// Lossy only when it has to be: valid UTF-8, the usual case, keeps its buffer
/// instead of being copied.
fn decode(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

#[derive(Debug)]
struct OverLimit(usize);

impl std::fmt::Display for OverLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "git output exceeds {}-byte capture budget", self.0)
    }
}

impl std::error::Error for OverLimit {}

/// Capture each stream up to its limit; past it, `prefix` keeps what fit and
/// reports a cut, otherwise the capture is refused. Either way the group is
/// killed and the child reaped before returning.
/// Wait for a grouped child with both streams capped.
///
/// The caller owns the deadline and the group kill; this only makes the
/// capture bounded, so an executable that streams cannot grow the daemon.
pub(crate) fn wait_bounded(
    child: std::process::Child,
    limit: usize,
) -> Result<std::process::Output, GitError> {
    bounded_output(child, limit, limit, false)
        .map(|(output, _)| output)
        .map_err(
            |error| match error.get_ref().and_then(|inner| inner.downcast_ref()) {
                Some(&OverLimit(limit)) => GitError::OutputLimit { limit },
                None => GitError::Io(error),
            },
        )
}

fn bounded_output(
    mut child: std::process::Child,
    stdout_limit: usize,
    stderr_limit: usize,
    prefix: bool,
) -> std::io::Result<(std::process::Output, bool)> {
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    std::thread::scope(|scope| {
        let stderr = scope.spawn(move || capture(stderr, stderr_limit, prefix, pid));
        let stdout = capture(stdout, stdout_limit, prefix, pid);
        let stderr = stderr.join().unwrap_or_else(|_| {
            kill_process_group(pid);
            Err(std::io::Error::other("git stderr reader panicked"))
        });
        let status = child.wait()?;
        let (stdout, stdout_cut) = stdout?;
        let (stderr, stderr_cut) = stderr?;
        Ok((
            std::process::Output {
                status,
                stdout,
                stderr,
            },
            stdout_cut || stderr_cut,
        ))
    })
}

fn capture(
    reader: Option<impl std::io::Read>,
    limit: usize,
    prefix: bool,
    pid: u32,
) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        if let Err(error) = reader.take(limit as u64 + 1).read_to_end(&mut bytes) {
            kill_process_group(pid);
            return Err(error);
        }
    }
    if bytes.len() <= limit {
        return Ok((bytes, false));
    }
    kill_process_group(pid);
    if !prefix {
        return Err(std::io::Error::other(OverLimit(limit)));
    }
    bytes.truncate(limit);
    Ok((bytes, true))
}

/// How long to wait for the worker to reap a killed child before giving up on it.
const KILL_REAP_GRACE: Duration = Duration::from_secs(5);

/// `SIGKILL` a timed-out child's whole process group, then the child directly as
/// a fallback. Best effort: an already-exited target just yields `ESRCH`. Killing
/// the group (`-pid`) reaches the `ssh`/helper/submodule descendants that
/// inherited the output pipes; killing the bare pid covers a child that moved
/// itself to another group. The worker then reaps via `wait_with_output`.
fn kill_process_group(pid: u32) {
    #[allow(
        clippy::cast_possible_wrap,
        reason = "a pid always fits in i32 on unix"
    )]
    let raw = pid as i32;
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(-raw),
        nix::sys::signal::Signal::SIGKILL,
    );
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(raw),
        nix::sys::signal::Signal::SIGKILL,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_capture_kills_and_reaps_an_overproducing_process() {
        use std::os::unix::process::CommandExt;
        for redirect in ["", " >&2"] {
            let child = Command::new("/bin/sh")
                .args([
                    "-c",
                    &format!("while :; do printf '0123456789abcdef'{redirect}; done"),
                ])
                .process_group(0)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let pid = child.id();
            let error = bounded_output(child, 128, 128, false).unwrap_err();
            assert!(error.to_string().contains("capture budget"));
            assert_eq!(
                nix::sys::wait::waitpid(
                    nix::unistd::Pid::from_raw(pid as i32),
                    Some(nix::sys::wait::WaitPidFlag::WNOHANG)
                ),
                Err(nix::errno::Errno::ECHILD)
            );
        }
    }

    #[test]
    fn prefix_capture_keeps_exactly_the_limit_and_reaps_the_killed_process() {
        use std::os::unix::process::CommandExt;
        let child = Command::new("/bin/sh")
            .args(["-c", "while :; do printf 'a line\\n'; done"])
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let (output, cut) = bounded_output(child, 64, PREFIX_STDERR_BYTES, true).unwrap();
        assert!(cut);
        assert_eq!(output.stdout, "a line\n".repeat(10).as_bytes()[..64]);
        assert_eq!(
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(pid as i32),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG)
            ),
            Err(nix::errno::Errno::ECHILD)
        );
    }

    #[test]
    fn an_overflowing_bounded_command_is_its_own_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = run_git_bounded(Some(dir.path()), &["--version"], 4).unwrap_err();
        assert!(
            matches!(error, GitError::OutputLimit { limit: 4 }),
            "{error:?}"
        );
    }

    fn output(status: i32, stdout: &str, stderr: &str) -> GitOutput {
        GitOutput {
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            status,
        }
    }

    /// ADR-008 fixes the per-command budget; a silent change here would let a
    /// hung `git` stall a caller far longer than the contract allows.
    #[test]
    fn the_timeout_is_the_adr_008_budget() {
        assert_eq!(GIT_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn only_status_zero_is_success() {
        assert!(output(0, "", "").success());
        assert!(!output(1, "", "").success());
        assert!(!output(128, "", "").success());
        // A signalled child is reported as -1, which is not success.
        assert!(!output(-1, "", "").success());
    }

    #[test]
    fn stdout_trimmed_drops_the_trailing_newline_of_a_single_value_query() {
        assert_eq!(output(0, "main\n", "").stdout_trimmed(), "main");
        assert_eq!(
            output(0, "  /repo/root \n\n", "").stdout_trimmed(),
            "/repo/root"
        );
        assert_eq!(output(0, "", "").stdout_trimmed(), "");
        // Interior whitespace is preserved: only the ends are trimmed.
        assert_eq!(output(0, " a b \n", "").stdout_trimmed(), "a b");
    }

    #[test]
    fn ok_passes_a_successful_run_through_unchanged() {
        let out = output(0, "hello\n", "").ok(&["status"]).unwrap();
        assert_eq!(out.stdout, "hello\n");
        assert_eq!(out.status, 0);
    }

    #[test]
    fn ok_turns_a_non_zero_exit_into_a_failure_carrying_the_args_and_stderr() {
        let err = output(128, "", "fatal: not a git repository\n")
            .ok(&["rev-parse", "--show-toplevel"])
            .unwrap_err();
        match err {
            GitError::CommandFailed {
                args,
                stderr,
                status,
            } => {
                assert_eq!(
                    args,
                    vec!["rev-parse".to_owned(), "--show-toplevel".to_owned()]
                );
                assert_eq!(stderr, "fatal: not a git repository", "stderr is trimmed");
                assert_eq!(status, 128);
            }
            other => panic!("expected CommandFailed, got {other:?}"),
        }
    }

    /// The messages end up in daemon logs and client notices, so they must name
    /// what failed rather than just its type.
    #[test]
    fn errors_say_what_went_wrong() {
        let failed = output(1, "", "bad ref")
            .ok(&["checkout", "nope"])
            .unwrap_err()
            .to_string();
        assert!(failed.contains("checkout"), "{failed}");
        assert!(failed.contains("bad ref"), "{failed}");
        assert!(failed.contains('1'), "{failed}");

        assert_eq!(
            GitError::Timeout {
                timeout: GIT_TIMEOUT
            }
            .to_string(),
            "git command timed out after 30s"
        );
        assert_eq!(
            GitError::Timeout {
                timeout: GIT_NETWORK_TIMEOUT
            }
            .to_string(),
            "git command timed out after 120s"
        );
        assert!(GitError::NotAGitRepo(PathBuf::from("/tmp/x"))
            .to_string()
            .contains("/tmp/x"));
        assert!(GitError::InvalidBranchName {
            branch: "--force".to_owned(),
            reason: "looks like an option".to_owned(),
        }
        .to_string()
        .contains("--force"));
        assert!(GitError::Conflict {
            branch: "feat".to_owned(),
            path: PathBuf::from("/wt/feat"),
        }
        .to_string()
        .contains("/wt/feat"));
    }
}
