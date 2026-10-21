//! Per-file definitions, so a second lookup in a warm checkout parses only
//! what changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

/// One `@definition` capture, ready to become a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Definition {
    pub(crate) name: Box<str>,
    /// 1-based.
    pub(crate) line: u32,
    /// 1-based UTF-16 column of the name.
    pub(crate) column: u32,
    pub(crate) text: Box<str>,
}

/// Bookkeeping per entry, so a cache of many tiny files is not costed as free.
const ENTRY_OVERHEAD: usize = 128;
const DEFINITION_OVERHEAD: usize = std::mem::size_of::<Definition>();

pub(crate) fn weight(definitions: &[Definition]) -> usize {
    ENTRY_OVERHEAD
        + definitions
            .iter()
            .map(|d| DEFINITION_OVERHEAD + d.name.len() + d.text.len())
            .sum::<usize>()
}

/// What a file looked like when it was parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    root: PathBuf,
    path: Box<str>,
}

struct Entry {
    stamp: Stamp,
    definitions: Arc<[Definition]>,
    weight: usize,
    used: u64,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Key, Entry>,
    weight: usize,
    clock: u64,
}

/// Bounded by entry count and by the bytes its definitions hold; the least
/// recently used entry goes first. The lock covers a map operation only.
pub(crate) struct Cache {
    inner: Mutex<Inner>,
    max_entries: usize,
    max_weight: usize,
}

impl Cache {
    pub(crate) fn new(max_entries: usize, max_weight: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            max_entries,
            max_weight,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn get(&self, root: &Path, path: &str, stamp: Stamp) -> Option<Arc<[Definition]>> {
        let key = Key {
            root: root.to_path_buf(),
            path: path.into(),
        };
        let mut inner = self.lock();
        inner.clock += 1;
        let clock = inner.clock;
        let entry = inner.entries.get_mut(&key)?;
        if entry.stamp != stamp {
            return None;
        }
        entry.used = clock;
        Some(Arc::clone(&entry.definitions))
    }

    pub(crate) fn put(
        &self,
        root: &Path,
        path: &str,
        stamp: Stamp,
        definitions: Arc<[Definition]>,
    ) {
        let weight = weight(&definitions);
        if weight > self.max_weight || self.max_entries == 0 {
            return;
        }
        let key = Key {
            root: root.to_path_buf(),
            path: path.into(),
        };
        let mut inner = self.lock();
        inner.clock += 1;
        let used = inner.clock;
        if let Some(old) = inner.entries.insert(
            key,
            Entry {
                stamp,
                definitions,
                weight,
                used,
            },
        ) {
            inner.weight -= old.weight;
        }
        inner.weight += weight;
        while inner.entries.len() > self.max_entries || inner.weight > self.max_weight {
            let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(gone) = inner.entries.remove(&oldest) {
                inner.weight -= gone.weight;
            }
        }
    }

    pub(crate) fn invalidate(&self, root: &Path, path: Option<&str>) {
        let mut inner = self.lock();
        let Inner {
            entries, weight, ..
        } = &mut *inner;
        entries.retain(|key, entry| {
            let keep = key.root != root || path.is_some_and(|path| *key.path != *path);
            if !keep {
                *weight -= entry.weight;
            }
            keep
        });
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs(name: &str) -> Arc<[Definition]> {
        Arc::from(vec![Definition {
            name: name.into(),
            line: 1,
            column: 1,
            text: name.into(),
        }])
    }

    fn stamp(len: u64) -> Stamp {
        Stamp {
            len,
            modified: None,
        }
    }

    #[test]
    fn a_changed_stamp_misses_and_the_least_recently_used_entry_is_evicted() {
        let cache = Cache::new(2, usize::MAX);
        let root = Path::new("/r");
        cache.put(root, "a", stamp(1), defs("a"));
        cache.put(root, "b", stamp(1), defs("b"));
        assert!(cache.get(root, "a", stamp(2)).is_none());
        assert!(cache.get(root, "a", stamp(1)).is_some());
        cache.put(root, "c", stamp(1), defs("c"));
        assert_eq!(cache.len(), 2);
        assert!(
            cache.get(root, "b", stamp(1)).is_none(),
            "b was used last longest ago"
        );
        assert!(cache.get(Path::new("/other"), "a", stamp(1)).is_none());
    }

    #[test]
    fn weight_bounds_the_cache_and_invalidation_is_per_root_or_path() {
        let one = weight(&defs("a"));
        let cache = Cache::new(100, one * 2);
        let root = Path::new("/r");
        cache.put(root, "a", stamp(1), defs("a"));
        cache.put(root, "b", stamp(1), defs("b"));
        cache.put(root, "c", stamp(1), defs("c"));
        assert_eq!(cache.len(), 2);
        cache.put(Path::new("/s"), "a", stamp(1), defs("a"));
        cache.invalidate(root, Some("c"));
        assert!(cache.get(root, "c", stamp(1)).is_none());
        cache.invalidate(root, None);
        assert!(cache.get(Path::new("/s"), "a", stamp(1)).is_some());
        assert_eq!(cache.len(), 1);
    }
}
