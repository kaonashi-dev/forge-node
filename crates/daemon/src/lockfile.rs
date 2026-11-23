//! Singleton enforcement via an advisory `flock`.
//!
//! The authority is the `flock(LOCK_EX | LOCK_NB)`, not the PID written into the
//! file: the JSON body is purely informative. If the lock cannot be taken,
//! another daemon owns it and this process exits with code 0 (double-spawn
//! race).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use nix::fcntl::{Flock, FlockArg};

/// A held singleton lock. Dropping it (or process exit) releases the `flock`.
pub struct DaemonLock {
    _lock: Flock<File>,
}

/// Informative metadata; only the `flock` proves ownership.
#[derive(serde::Serialize)]
struct LockInfo<'a> {
    pid: u32,
    instance_id: &'a str,
    version: &'a str,
    started_at: String,
}

/// Errors acquiring the singleton lock.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("another daemon instance already holds the lock")]
    AlreadyHeld,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Try to acquire the singleton lock at `path`, writing informative metadata.
///
/// # Errors
/// Returns [`LockError::AlreadyHeld`] if another instance owns the lock, or
/// [`LockError::Io`] if the file cannot be opened/written.
pub fn acquire(
    path: &Path,
    instance_id: &str,
    version: &str,
    started_at: domain::Timestamp,
) -> Result<DaemonLock, LockError> {
    acquire_with(
        path,
        instance_id,
        version,
        started_at,
        FlockArg::LockExclusiveNonblock,
    )
}

/// Acquire the singleton lock, waiting for a running daemon to release it.
///
/// # Errors
/// [`LockError::Io`] if the file cannot be opened or written. A held lock is
/// not an error here: the call simply blocks until it is free.
pub fn acquire_blocking(
    path: &Path,
    instance_id: &str,
    version: &str,
    started_at: domain::Timestamp,
) -> Result<DaemonLock, LockError> {
    acquire_with(
        path,
        instance_id,
        version,
        started_at,
        FlockArg::LockExclusive,
    )
}

/// The pid the daemon holding the lock at `path` wrote into it, or `None`
/// when nobody holds it. Never writes: a probe must not repaint the body.
///
/// # Errors
/// I/O on the lock file, or [`std::io::ErrorKind::InvalidData`] when the
/// lock is held but its body names no pid.
pub fn holder(path: &Path) -> std::io::Result<Option<u32>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut file = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(_free) => return Ok(None),
        Err((file, nix::errno::Errno::EWOULDBLOCK)) => file,
        Err((_file, error)) => return Err(std::io::Error::from(error)),
    };
    let mut body = String::new();
    std::io::Read::read_to_string(&mut file, &mut body)?;
    #[derive(serde::Deserialize)]
    struct Pid {
        pid: u32,
    }
    serde_json::from_str::<Pid>(&body)
        .map(|info| Some(info.pid))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Block until nobody holds the lock at `path`, without writing to it.
///
/// # Errors
/// I/O opening or locking the file; a missing file is already released.
pub fn wait_released(path: &Path) -> std::io::Result<()> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    Flock::lock(file, FlockArg::LockExclusive)
        .map(drop)
        .map_err(|(_file, error)| std::io::Error::from(error))
}

fn acquire_with(
    path: &Path,
    instance_id: &str,
    version: &str,
    started_at: domain::Timestamp,
    arg: FlockArg,
) -> Result<DaemonLock, LockError> {
    if let Some(parent) = path.parent() {
        crate::paths::ensure_private_dir(parent)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;

    let mut lock = match Flock::lock(file, arg) {
        Ok(lock) => lock,
        Err((_file, nix::errno::Errno::EWOULDBLOCK)) => return Err(LockError::AlreadyHeld),
        Err((_file, e)) => return Err(LockError::Io(std::io::Error::from(e))),
    };

    // We own the lock: overwrite the informative body.
    let info = LockInfo {
        pid: std::process::id(),
        instance_id,
        version,
        started_at: started_at.to_rfc3339(),
    };
    lock.set_len(0)?;
    let json = serde_json::to_string_pretty(&info).unwrap_or_default();
    lock.write_all(json.as_bytes())?;
    lock.flush()?;

    Ok(DaemonLock { _lock: lock })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_rejected_while_first_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");
        let ts = domain::Timestamp::now();

        let _first = acquire(&path, "inst-1", "0.1.0", ts).expect("first lock");
        let second = acquire(&path, "inst-2", "0.1.0", ts);
        assert!(matches!(second, Err(LockError::AlreadyHeld)));
    }

    #[test]
    fn lock_is_reacquirable_after_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");
        let ts = domain::Timestamp::now();

        {
            let _first = acquire(&path, "inst-1", "0.1.0", ts).expect("first lock");
        } // released here
        let _again = acquire(&path, "inst-2", "0.1.0", ts).expect("reacquire after release");
    }

    #[test]
    fn holder_names_the_pid_only_while_the_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");
        assert_eq!(holder(&path).unwrap(), None, "a missing lock has no holder");

        let held = acquire(&path, "inst-1", "0.1.0", domain::Timestamp::now()).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(holder(&path).unwrap(), Some(std::process::id()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);

        drop(held);
        assert_eq!(holder(&path).unwrap(), None);
        wait_released(&path).expect("a released lock does not block");
    }
}
