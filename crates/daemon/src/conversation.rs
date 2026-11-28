//! Each agent session's transcript path, remembered past the process that
//! registered it, so `GetSessionConversation` still reads a finished run.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use domain::SessionId;

/// Entries kept before the map is dropped whole; a forgotten entry only costs
/// one registry read while its process still runs.
const MAX_REMEMBERED: usize = 256;

struct Found {
    pid: u32,
    path: PathBuf,
}

/// Own lock, never held across the core lock or the lookup's filesystem IO.
#[derive(Default)]
pub(crate) struct Transcripts(Mutex<HashMap<SessionId, Found>>);

impl Transcripts {
    /// `live_pid` is the session's current child, `None` once it has exited.
    /// `lookup` runs only for a pid this session has not resolved yet, so a
    /// restart finds its new transcript and keeps the old one until it does.
    pub(crate) fn resolve(
        &self,
        session_id: SessionId,
        live_pid: Option<u32>,
        lookup: impl FnOnce(u32) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        let cached = self
            .map()
            .get(&session_id)
            .map(|found| (found.pid, found.path.clone()));
        let unresolved =
            live_pid.filter(|pid| cached.as_ref().is_none_or(|(known, _)| known != pid));
        if let Some(pid) = unresolved {
            if let Some(path) = lookup(pid) {
                let mut map = self.map();
                if map.len() >= MAX_REMEMBERED && !map.contains_key(&session_id) {
                    map.clear();
                }
                map.insert(
                    session_id,
                    Found {
                        pid,
                        path: path.clone(),
                    },
                );
                return Some(path);
            }
        }
        cached.map(|(_, path)| path)
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, Found>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_resolved_path_outlives_its_process_and_a_new_pid_looks_again() {
        let transcripts = Transcripts::default();
        let id = SessionId::new();
        let lookups = Cell::new(0);
        let find = |path: Option<&str>| {
            lookups.set(lookups.get() + 1);
            path.map(PathBuf::from)
        };

        assert_eq!(transcripts.resolve(id, Some(7), |_| find(None)), None);
        assert_eq!(
            transcripts.resolve(id, Some(7), |_| find(Some("/a"))),
            Some(PathBuf::from("/a"))
        );
        assert_eq!(
            transcripts.resolve(id, Some(7), |_| find(Some("/other"))),
            Some(PathBuf::from("/a")),
        );
        assert_eq!(lookups.get(), 2, "a resolved pid is not looked up again");
        assert_eq!(
            transcripts.resolve(id, None, |_| find(None)),
            Some(PathBuf::from("/a"))
        );
        assert_eq!(
            transcripts.resolve(id, Some(8), |_| find(None)),
            Some(PathBuf::from("/a")),
            "a restarted process keeps the old transcript until it registers"
        );
        assert_eq!(
            transcripts.resolve(id, Some(8), |_| find(Some("/b"))),
            Some(PathBuf::from("/b"))
        );
    }

    #[test]
    fn the_map_is_bounded() {
        let transcripts = Transcripts::default();
        for n in 0..=MAX_REMEMBERED {
            transcripts.resolve(SessionId::new(), Some(1), |_| {
                Some(PathBuf::from(format!("/{n}")))
            });
        }
        assert!(transcripts.map().len() <= MAX_REMEMBERED);
    }
}
