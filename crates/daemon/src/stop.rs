//! `forge-daemon stop`: ends whichever daemon holds the singleton lock by signal,
//! so it works on a daemon that speaks another protocol or no longer answers.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

use crate::lockfile;

/// How long a SIGKILLed daemon has to let the kernel release its lock.
const KILL_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub enum Stopped {
    NotRunning,
    Exited { pid: u32 },
    Killed { pid: u32 },
}

/// SIGTERM runs the daemon's own shutdown, which ends every session with its
/// kill grace; SIGKILL follows only when the lock is still held after `grace`.
pub fn stop(lock_path: &Path, grace: Duration) -> anyhow::Result<Stopped> {
    let Some(pid) = lockfile::holder(lock_path)? else {
        return Ok(Stopped::NotRunning);
    };
    let target = i32::try_from(pid)
        .ok()
        .filter(|&pid| pid > 1 && pid.unsigned_abs() != std::process::id())
        .map(Pid::from_raw)
        .ok_or_else(|| anyhow::anyhow!("{} names pid {pid}", lock_path.display()))?;
    let released = waiter(lock_path.to_path_buf());
    signal(target, Signal::SIGTERM)?;
    if released.recv_timeout(grace).is_ok() {
        return Ok(Stopped::Exited { pid });
    }
    signal(target, Signal::SIGKILL)?;
    if released.recv_timeout(KILL_WAIT).is_ok() {
        return Ok(Stopped::Killed { pid });
    }
    anyhow::bail!(
        "pid {pid} still holds {} after SIGKILL",
        lock_path.display()
    )
}

fn signal(pid: Pid, signal: Signal) -> anyhow::Result<()> {
    match kill(pid, signal) {
        // Gone between the probe and the signal: the waiter sees the release.
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(anyhow::anyhow!("{signal} to pid {pid}: {error}")),
    }
}

/// Ready when the lock is free. The thread stays blocked in `flock` if it never
/// is; it dies with this short-lived process.
fn waiter(lock_path: PathBuf) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if lockfile::wait_released(&lock_path).is_ok() {
            let _ = tx.send(());
        }
    });
    rx
}
