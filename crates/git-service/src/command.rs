//! Running `git` as a plain, captured subprocess (ADR-008).
//!
//! Every git invocation goes through [`run_git`], which:
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

    /// The command exceeded [`GIT_TIMEOUT`] and was killed.
    #[error("git command timed out after {}s", GIT_TIMEOUT.as_secs())]
    Timeout,

    /// `check-ref-format --branch` rejected the branch name.
    #[error("invalid branch name {branch:?}: {reason}")]
    InvalidBranchName { branch: String, reason: String },

    /// The requested branch is already checked out in another worktree.
    #[error("branch {branch:?} is already checked out at {path}")]
    Conflict { branch: String, path: PathBuf },

    /// A path argument could not be represented as UTF-8 for the CLI.
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),

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
    run_git_inner(repo, args, GIT_TIMEOUT, false)
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
    run_git_inner(repo, args, timeout, true)
}

fn run_git_inner(
    repo: Option<&Path>,
    args: &[&str],
    timeout: Duration,
    network: bool,
) -> Result<GitOutput, GitError> {
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
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(GitOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status.code().unwrap_or(-1),
        }),
        Ok(Err(e)) => Err(GitError::Io(e)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_process_group(pid);
            // Do not return while the worker still owns an unreaped child: the
            // SIGKILL closes the pipes, so `wait_with_output` returns almost at
            // once. A bounded wait keeps a wedged descendant (uninterruptible
            // sleep, or one that escaped the group with its own `setsid`) from
            // blocking the caller indefinitely.
            let _ = rx.recv_timeout(KILL_REAP_GRACE);
            Err(GitError::Timeout)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(GitError::Io(std::io::Error::other(
            "git worker thread disconnected before reporting a result",
        ))),
    }
}

/// How long to wait for the worker to reap a killed child before giving up on it.
const KILL_REAP_GRACE: Duration = Duration::from_secs(5);

/// `SIGKILL` a timed-out child's whole process group, then the child directly as
/// a fallback. Best effort: an already-exited target just yields `ESRCH`. Killing
/// the group (`-pid`) reaches the `ssh`/helper/submodule descendants that
/// inherited the output pipes; killing the bare pid covers a child that moved
/// itself to another group. The worker then reaps via `wait_with_output`.
fn kill_process_group(pid: u32) {
    #[allow(clippy::cast_possible_wrap)]
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
            GitError::Timeout.to_string(),
            "git command timed out after 30s"
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
