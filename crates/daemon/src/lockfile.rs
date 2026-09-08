//! Singleton enforcement via an advisory `flock` (§9.2).
//!
//! The authority is the `flock(LOCK_EX | LOCK_NB)`, not the PID written into the
//! file: the JSON body is purely informative. If the lock cannot be taken,
//! another daemon owns it and this process exits with code 0 (§9.1 double-spawn
//! race).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use nix::fcntl::{Flock, FlockArg};

/// A held singleton lock. Dropping it (or process exit) releases the `flock`.
pub struct DaemonLock {
    _lock: Flock<File>,
}

/// Informative lock metadata (§9.2). The `flock` is the real authority.
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

    let mut lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
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
}
