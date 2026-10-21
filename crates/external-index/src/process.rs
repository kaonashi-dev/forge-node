//! One grouped, bounded, deadline-limited subprocess, the `git_service::run_gh` shape.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long to wait for the worker to reap a killed child.
const KILL_REAP_GRACE: Duration = Duration::from_secs(5);

/// The cancel flag is an `AtomicBool`, which cannot wake a waiter, so the wait
/// is sliced this finely while one is supplied.
const CANCEL_POLL: Duration = Duration::from_millis(50);

const STDERR_KEEP_BYTES: usize = 4 * 1024;

/// A child that exited (or was killed for overproducing) and was reaped.
#[derive(Debug, Default)]
pub(crate) struct Finished {
    pub stdout: Vec<u8>,
    /// Stdout passed its cap: the child was killed and `stdout` is a prefix.
    pub cut: bool,
    /// `None` when a signal ended it.
    pub status: Option<i32>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Finished(Finished),
    /// The deadline passed or the caller cancelled; the group was killed.
    /// Carries what was read before the kill when the worker reaped in time.
    Expired(Option<Finished>),
}

/// Run `command` in its own process group with `input` on stdin.
///
/// Stdout is held up to `stdout_cap` bytes; past it the group is killed and
/// the prefix kept. Every path kills with a negative pgid, then the bare pid,
/// and reaps before returning (bounded by [`KILL_REAP_GRACE`]).
pub(crate) fn run(
    mut command: Command,
    input: Option<Vec<u8>>,
    stdout_cap: usize,
    deadline: Instant,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<Outcome> {
    use std::os::unix::process::CommandExt as _;

    command
        .env("LC_ALL", "C")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (tx, rx) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("external-index-run".into())
        .spawn(move || {
            let (stdout, cut, stderr) = std::thread::scope(|scope| {
                // Written from its own thread: a child that prints before it
                // has read all of its input would otherwise deadlock on a full
                // stdout pipe while we block on a full stdin one.
                if let (Some(mut pipe), Some(bytes)) = (stdin, input) {
                    scope.spawn(move || {
                        let _ = pipe.write_all(&bytes);
                    });
                }
                let stderr = scope.spawn(move || drain(stderr, STDERR_KEEP_BYTES));
                let (stdout, cut) = read_capped(stdout, stdout_cap, pid);
                (stdout, cut, stderr.join().unwrap_or_default())
            });
            let status = child.wait().map(|status| status.code());
            let _ = tx.send(status.map(|status| Finished {
                stdout,
                cut,
                status,
                stderr,
            }));
        });
    if let Err(error) = worker {
        // The closure that owned the `Child` was dropped without a wait.
        kill_group(pid);
        reap_killed(pid);
        return Err(error);
    }

    loop {
        let now = Instant::now();
        if now >= deadline || cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            kill_group(pid);
            let partial = rx.recv_timeout(KILL_REAP_GRACE).ok().and_then(Result::ok);
            return Ok(Outcome::Expired(partial));
        }
        let mut slice = deadline - now;
        if cancel.is_some() {
            slice = slice.min(CANCEL_POLL);
        }
        match rx.recv_timeout(slice) {
            Ok(result) => return result.map(Outcome::Finished),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                kill_group(pid);
                return Err(std::io::Error::other(
                    "subprocess worker ended without a result",
                ));
            }
        }
    }
}

fn read_capped(reader: Option<impl Read>, cap: usize, pid: u32) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    let Some(reader) = reader else {
        return (bytes, false);
    };
    if reader.take(cap as u64 + 1).read_to_end(&mut bytes).is_err() {
        kill_group(pid);
    }
    if bytes.len() <= cap {
        return (bytes, false);
    }
    kill_group(pid);
    bytes.truncate(cap);
    (bytes, true)
}

/// The first `keep` bytes, with the rest read into a sink so the child never
/// blocks on a full stderr pipe.
fn drain(reader: Option<impl Read>, keep: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    if let Some(mut reader) = reader {
        let _ = (&mut reader).take(keep as u64).read_to_end(&mut kept);
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    }
    kept
}

pub(crate) fn kill_group(pid: u32) {
    use nix::sys::signal::{kill, Signal::SIGKILL};
    use nix::unistd::Pid;
    let Ok(raw) = i32::try_from(pid) else {
        return;
    };
    // The group reaches descendants holding the pipes; the bare pid covers a
    // child that moved itself out of the group.
    let _ = kill(Pid::from_raw(-raw), SIGKILL);
    let _ = kill(Pid::from_raw(raw), SIGKILL);
}

/// Reap a child already sent `SIGKILL` whose `Child` handle is gone.
///
/// Polled with `WNOHANG` to a deadline rather than a blocking `waitpid`: a
/// child wedged in uninterruptible sleep must not hold the caller.
fn reap_killed(pid: u32) {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    let Ok(raw) = i32::try_from(pid) else {
        return;
    };
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        match waitpid(nix::unistd::Pid::from_raw(raw), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) => std::thread::sleep(Duration::from_millis(10)),
            _ => return,
        }
    }
}

/// Whole lines of a possibly cut stream: a cut one loses its last, partial line.
pub(crate) fn whole_lines(bytes: &[u8], cut: bool) -> &[u8] {
    if !cut {
        return bytes;
    }
    bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(&[][..], |at| &bytes[..=at])
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::sys::wait::{waitpid, WaitPidFlag};
    use nix::unistd::Pid;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn gone(pid: i32) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if kill(Pid::from_raw(pid), None) == Err(Errno::ESRCH) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn an_expired_child_has_its_whole_group_killed_and_is_reaped() {
        let started = Instant::now();
        let outcome = run(
            sh("sleep 30 & echo $$ $!; wait"),
            None,
            1024,
            Instant::now() + Duration::from_millis(400),
            None,
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        let Outcome::Expired(Some(partial)) = outcome else {
            panic!("expected an expired run with its partial output: {outcome:?}");
        };
        let said = String::from_utf8(partial.stdout).unwrap();
        let pids: Vec<i32> = said
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect();
        let [shell, sleeper] = pids[..] else {
            panic!("expected two pids, got {said:?}");
        };
        assert_eq!(
            waitpid(Pid::from_raw(shell), Some(WaitPidFlag::WNOHANG)),
            Err(Errno::ECHILD),
            "the direct child was reaped"
        );
        assert!(gone(sleeper), "the grandchild in the group was killed too");
    }

    #[test]
    fn cancelling_ends_the_wait_before_the_deadline() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let outcome = run(
            sh("sleep 30"),
            None,
            1024,
            Instant::now() + Duration::from_secs(30),
            Some(&cancel),
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Expired(_)));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn stdin_is_fed_and_an_overproducing_child_is_cut() {
        let outcome = run(
            sh("cat"),
            Some(b"hello\n".to_vec()),
            1024,
            Instant::now() + Duration::from_secs(10),
            None,
        )
        .unwrap();
        let Outcome::Finished(done) = outcome else {
            panic!("cat should finish");
        };
        assert_eq!(done.stdout, b"hello\n");
        assert_eq!(done.status, Some(0));

        let outcome = run(
            sh("while :; do echo 0123456789; done"),
            None,
            100,
            Instant::now() + Duration::from_secs(10),
            None,
        )
        .unwrap();
        let Outcome::Finished(done) = outcome else {
            panic!("a cut child is still reaped in time");
        };
        assert!(done.cut);
        assert_eq!(done.stdout.len(), 100);
        assert_eq!(whole_lines(&done.stdout, true).len(), 99);
    }
}
