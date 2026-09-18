//! Resident navigation index, one listing per checkout.
//!
//! `ListFiles` and `SearchKind::Name` share it. Own mutex: a `git ls-files`
//! must not sit on the core lock. Invalidated on `FileChanged` and mutations,
//! not by a TTL — a watch already says when the disk moved.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use domain::WorkspaceId;
use fs_service::FileTree;

struct Entry {
    root: PathBuf,
    tree: Arc<FileTree>,
}

struct Slot {
    generation: u64,
    entry: Option<Entry>,
}

#[derive(Default)]
struct Inner {
    /// Bumped on factory reset so in-flight walks cannot refill a cleared map.
    epoch: u64,
    workspaces: HashMap<WorkspaceId, Slot>,
}

/// Start-of-walk fence: [`Cache::put`] drops the listing if invalidate/clear
/// ran after [`Cache::begin`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation {
    epoch: u64,
    local: u64,
}

/// Last successful `list_files` per workspace.
#[derive(Default)]
pub struct Cache {
    inner: Mutex<Inner>,
}

impl Cache {
    /// The listing if it was built for this checkout.
    #[must_use]
    pub fn get(&self, workspace: WorkspaceId, root: &Path) -> Option<Arc<FileTree>> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .workspaces
            .get(&workspace)
            .and_then(|slot| slot.entry.as_ref())
            .filter(|entry| entry.root == root)
            .map(|entry| Arc::clone(&entry.tree))
    }

    /// Capture before `list_files`. Pair with [`Cache::put`] after the walk.
    #[must_use]
    pub fn begin(&self, workspace: WorkspaceId) -> Generation {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let local = guard
            .workspaces
            .get(&workspace)
            .map(|slot| slot.generation)
            .unwrap_or(0);
        Generation {
            epoch: guard.epoch,
            local,
        }
    }

    /// Remember a listing unless a later invalidate/clear won the race.
    pub fn put(
        &self,
        workspace: WorkspaceId,
        root: PathBuf,
        tree: FileTree,
        generation: Generation,
    ) -> Arc<FileTree> {
        let tree = Arc::new(tree);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if generation.epoch != guard.epoch {
            return tree;
        }
        let slot = guard.workspaces.entry(workspace).or_insert(Slot {
            generation: 0,
            entry: None,
        });
        if generation.local != slot.generation {
            return tree;
        }
        slot.entry = Some(Entry {
            root,
            tree: Arc::clone(&tree),
        });
        tree
    }

    /// Drop the listing so the next read walks the disk.
    pub fn invalidate(&self, workspace: WorkspaceId) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let slot = guard.workspaces.entry(workspace).or_insert(Slot {
            generation: 0,
            entry: None,
        });
        slot.generation = slot.generation.wrapping_add(1);
        slot.entry = None;
    }

    /// Drop every listing (factory reset).
    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.epoch = guard.epoch.wrapping_add(1);
        guard.workspaces.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_service::{EntryKind, FileEntry};

    fn tree(path: &str) -> FileTree {
        FileTree {
            entries: vec![FileEntry {
                path: path.to_owned(),
                kind: EntryKind::File,
                ignored: false,
                symlink: None,
            }],
            truncated: false,
        }
    }

    #[test]
    fn remembers_a_listing_until_invalidated_or_the_root_moves() {
        let cache = Cache::default();
        let workspace = WorkspaceId::new();
        let root = PathBuf::from("/repo");
        let generation = cache.begin(workspace);
        cache.put(workspace, root.clone(), tree("a.rs"), generation);
        assert_eq!(cache.get(workspace, &root).unwrap().entries[0].path, "a.rs");
        assert!(cache.get(workspace, Path::new("/other")).is_none());
        cache.invalidate(workspace);
        assert!(cache.get(workspace, &root).is_none());
        let generation = cache.begin(workspace);
        cache.put(workspace, root.clone(), tree("b.rs"), generation);
        assert_eq!(cache.get(workspace, &root).unwrap().entries[0].path, "b.rs");
    }

    #[test]
    fn put_loses_to_invalidate_or_clear_during_the_walk() {
        let cache = Cache::default();
        let workspace = WorkspaceId::new();
        let root = PathBuf::from("/repo");
        let stale = cache.begin(workspace);
        cache.invalidate(workspace);
        cache.put(workspace, root.clone(), tree("stale.rs"), stale);
        assert!(cache.get(workspace, &root).is_none());

        let generation = cache.begin(workspace);
        cache.put(workspace, root.clone(), tree("fresh.rs"), generation);
        assert_eq!(
            cache.get(workspace, &root).unwrap().entries[0].path,
            "fresh.rs"
        );
        let before_reset = cache.begin(workspace);
        cache.clear();
        cache.put(workspace, root.clone(), tree("reset.rs"), before_reset);
        assert!(cache.get(workspace, &root).is_none());
    }
}
