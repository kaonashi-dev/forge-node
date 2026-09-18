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

/// Last successful `list_files` per workspace.
#[derive(Default)]
pub struct Cache {
    inner: Mutex<HashMap<WorkspaceId, Entry>>,
}

impl Cache {
    /// The listing if it was built for this checkout.
    #[must_use]
    pub fn get(&self, workspace: WorkspaceId, root: &Path) -> Option<Arc<FileTree>> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get(&workspace)
            .filter(|entry| entry.root == root)
            .map(|entry| Arc::clone(&entry.tree))
    }

    /// Remember a listing. Replaces any previous one for `workspace`.
    pub fn put(&self, workspace: WorkspaceId, root: PathBuf, tree: FileTree) -> Arc<FileTree> {
        let tree = Arc::new(tree);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(
            workspace,
            Entry {
                root,
                tree: Arc::clone(&tree),
            },
        );
        tree
    }

    /// Drop the listing so the next read walks the disk.
    pub fn invalidate(&self, workspace: WorkspaceId) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&workspace);
    }

    /// Drop every listing (factory reset).
    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clear();
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
        cache.put(workspace, root.clone(), tree("a.rs"));
        assert_eq!(cache.get(workspace, &root).unwrap().entries[0].path, "a.rs");
        assert!(cache.get(workspace, Path::new("/other")).is_none());
        cache.invalidate(workspace);
        assert!(cache.get(workspace, &root).is_none());
    }
}
