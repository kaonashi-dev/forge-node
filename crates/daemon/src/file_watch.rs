//! Connection-scoped directory watches. Native callbacks only enqueue bounded
//! invalidations; a worker coalesces them without touching the core lock.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{ClientId, WorkspaceId};
use notify::Watcher;
use protocol::{DaemonEvent, DaemonMessage, ProtocolError};

use crate::core::Daemon;

const MAX_DIRECTORIES: usize = 128;
const MAX_PATHS: usize = 32;
const MAX_PATH_BYTES: usize = 4096;
const COALESCE: Duration = Duration::from_millis(150);
const RETRY: Duration = Duration::from_millis(250);

pub(crate) struct Subscription {
    _watcher: notify::RecommendedWatcher,
}

impl Subscription {
    pub(crate) fn start(
        daemon: &Arc<Daemon>,
        client: ClientId,
        workspace: WorkspaceId,
        root: &Path,
        directories: &[String],
    ) -> Result<Self, ProtocolError> {
        let (root, directories) = resolve_directories(root, directories)?;
        let (tx, rx) = flume::bounded::<Vec<PathBuf>>(16);
        let overflow = Arc::new(AtomicBool::new(false));
        let exceeded = Arc::clone(&overflow);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let paths = match event {
                    Ok(event) if event.kind.is_access() => return,
                    Ok(mut event) if !event.need_rescan() && event.paths.len() <= MAX_PATHS => {
                        if event
                            .paths
                            .iter()
                            .any(|path| path.as_os_str().len() > MAX_PATH_BYTES)
                        {
                            exceeded.store(true, Ordering::Release);
                            Vec::new()
                        } else {
                            std::mem::take(&mut event.paths)
                        }
                    }
                    _ => {
                        exceeded.store(true, Ordering::Release);
                        Vec::new()
                    }
                };
                if tx.try_send(paths).is_err() {
                    exceeded.store(true, Ordering::Release);
                }
            })
            .map_err(watch_error)?;
        for directory in directories {
            watcher
                .watch(&directory, notify::RecursiveMode::NonRecursive)
                .map_err(watch_error)?;
        }
        let daemon = Arc::downgrade(daemon);
        std::thread::Builder::new()
            .name("forge-files".into())
            .spawn(move || {
                // A resync the client could not take stays owed until it does.
                let mut owed = false;
                loop {
                    let first = match if owed {
                        rx.recv_timeout(RETRY)
                    } else {
                        rx.recv().map_err(|_| flume::RecvTimeoutError::Disconnected)
                    } {
                        Ok(paths) => paths,
                        Err(flume::RecvTimeoutError::Timeout) => Vec::new(),
                        Err(flume::RecvTimeoutError::Disconnected) => break,
                    };
                    let mut pending = BTreeSet::new();
                    collect_paths(&root, first, &mut pending, &overflow);
                    let deadline = Instant::now() + COALESCE;
                    while let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                        match rx.recv_timeout(wait) {
                            Ok(paths) => collect_paths(&root, paths, &mut pending, &overflow),
                            Err(_) => break,
                        }
                    }
                    if overflow.swap(false, Ordering::AcqRel) || owed {
                        pending.clear();
                        // Empty path invalidates the watched surface after lost native events.
                        pending.insert(String::new());
                    }
                    let Some(daemon) = daemon.upgrade() else {
                        break;
                    };
                    owed = false;
                    for path in pending {
                        if !daemon.registry.send_to(
                            client,
                            DaemonMessage::Event(DaemonEvent::FileChanged {
                                workspace_id: workspace,
                                path,
                            }),
                        ) {
                            // `send_to` cannot say whether the client is gone
                            // or its queue is momentarily full, and that queue
                            // is the one carrying terminal deltas: dropping the
                            // connection over a burst would tear down a live
                            // GUI over a notification. So this takes the way
                            // out a lost native event already has — one resync,
                            // owed until it lands, retried without needing a
                            // further event to carry it. A client that is
                            // really gone ends this thread by dropping its
                            // watch when the connection closes.
                            owed = true;
                            break;
                        }
                    }
                }
            })
            .map_err(watch_error)?;
        Ok(Self { _watcher: watcher })
    }
}

fn watch_error(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::invalid_request(format!("file watch unavailable: {error}"))
}

fn resolve_directories(
    root: &Path,
    paths: &[String],
) -> Result<(PathBuf, BTreeSet<PathBuf>), ProtocolError> {
    if paths.len() > MAX_DIRECTORIES || paths.iter().any(|path| path.len() > MAX_PATH_BYTES) {
        return Err(ProtocolError::invalid_request(
            "file watch exceeds directory budget",
        ));
    }
    let root = root.canonicalize().map_err(watch_error)?;
    let mut directories = BTreeSet::new();
    for relative in paths {
        if Path::new(relative).is_absolute()
            || Path::new(relative)
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(ProtocolError::invalid_request(
                "watch path escapes workspace",
            ));
        }
        // A directory that will not resolve is skipped, not fatal: the listing
        // the client watched from is a moment older than the checkout, and one
        // folder deleted in between must not cost live updates for the rest of
        // it. An escape is still refused — that is a claim about the request,
        // not a race.
        let Ok(path) = root.join(relative).canonicalize() else {
            continue;
        };
        if !path.starts_with(&root) {
            return Err(ProtocolError::invalid_request(
                "watch path escapes workspace",
            ));
        }
        if path.is_dir() {
            directories.insert(path);
        }
    }
    Ok((root, directories))
}

fn collect_paths(
    root: &Path,
    paths: Vec<PathBuf>,
    pending: &mut BTreeSet<String>,
    overflow: &AtomicBool,
) {
    for path in paths {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if pending.len() >= MAX_PATHS {
            overflow.store(true, Ordering::Release);
            return;
        }
        pending.insert(relative.to_string_lossy().into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watches_are_bounded_and_cannot_follow_symlinks_outside_workspace() {
        let root = tempfile::tempdir().expect("fixture");
        let outside = tempfile::tempdir().expect("fixture");
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).expect("symlink");
        assert!(resolve_directories(root.path(), &["escape".into()]).is_err());
        assert!(resolve_directories(root.path(), &["..".into()]).is_err());
        assert!(
            resolve_directories(root.path(), &vec![String::new(); MAX_DIRECTORIES + 1]).is_err()
        );
        assert_eq!(
            resolve_directories(root.path(), &[String::new(), String::new()])
                .expect("root")
                .1
                .len(),
            1
        );
    }

    #[test]
    fn a_directory_that_vanished_is_skipped_and_the_rest_stay_watched() {
        let root = tempfile::tempdir().expect("fixture");
        std::fs::create_dir(root.path().join("src")).expect("fixture");
        std::fs::write(root.path().join("README.md"), "").expect("fixture");
        let (_, directories) = resolve_directories(
            root.path(),
            &[
                String::new(),
                "src".into(),
                // Deleted between the listing and the watch, and a file where a
                // directory was: neither may cost live updates for the rest.
                "gone".into(),
                "README.md".into(),
            ],
        )
        .expect("partial watch");
        assert_eq!(directories.len(), 2);
    }

    #[test]
    fn an_event_burst_requests_resync_instead_of_growing_the_path_set() {
        let root = Path::new("/workspace");
        let mut pending = BTreeSet::new();
        let overflow = AtomicBool::new(false);
        collect_paths(
            root,
            (0..100).map(|i| root.join(format!("{i}.rs"))).collect(),
            &mut pending,
            &overflow,
        );
        assert_eq!(pending.len(), MAX_PATHS);
        assert!(overflow.load(Ordering::Acquire));
    }
}
