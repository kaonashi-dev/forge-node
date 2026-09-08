//! Provisioning a workspace from its project's sharing rules (§14.2).
//!
//! A managed worktree lives under `worktrees.root`, away from the repository,
//! so it starts without the untracked files the project needs to run. These
//! modules put them there.
//!
//! The split follows `idle.rs`: [`plan`] is a **pure** function of what the
//! rules say and what the disk currently looks like, [`apply`] performs the
//! effects, and `core.rs` decides which workspace, when, and what to broadcast.
//! Nothing here takes the core lock, and nothing here is called while it is
//! held: every function below can block on the filesystem or on a subprocess.

pub mod apply;
pub mod detect;
pub mod plan;

use std::path::{Path, PathBuf};

/// Directory inside the repository's **common** git dir that holds the files
/// `Link` rules point at (§14.2).
///
/// The common dir is the one path every workspace of a project agrees on, so
/// the store moves with the repository, dies with it, and never makes one
/// checkout load-bearing for the others.
pub const STORE_DIR: &str = "forge/shared";

/// Where a displaced file is kept when something had to be moved aside.
pub const BACKUP_DIR: &str = "forge/backups";

/// Bytes a `Copy` (or a `Clone` that had to fall back) may write for one rule
/// before it is refused. A fallback copy of `node_modules` on a volume without
/// copy-on-write is a several-hundred-megabyte surprise, and a surprise is not
/// a provisioning strategy.
pub const MAX_COPY_BYTES: u64 = 250 * 1024 * 1024;

/// Entries a `Copy` may write for one rule before it is refused.
pub const MAX_COPY_ENTRIES: u64 = 50_000;

/// Entries the sizing walk visits before it gives up and reports `None`.
/// A missing number is honest; an incomplete one pretends to be exhaustive.
pub const MAX_MEASURE_ENTRIES: u64 = 20_000;

/// What one path looks like on disk right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EntryKind {
    /// Nothing at that path.
    #[default]
    Missing,
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link, whatever it points at.
    Symlink,
    /// A socket, fifo or device — never something we copy.
    Other,
}

impl EntryKind {
    /// Read the kind without following the link, so a symlink reports as one.
    #[must_use]
    pub fn of(path: &Path) -> Self {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_symlink() => Self::Symlink,
            Ok(meta) if meta.is_file() => Self::File,
            Ok(meta) if meta.is_dir() => Self::Dir,
            Ok(_) => Self::Other,
            Err(_) => Self::Missing,
        }
    }

    /// Whether anything at all is there.
    #[must_use]
    pub const fn exists(self) -> bool {
        !matches!(self, Self::Missing)
    }
}

/// The filesystem facts planning needs, injected so the decision table is
/// testable without a filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The platform can clone a file (or tree) copy-on-write.
    pub supports_clone: bool,
    /// Source and target are on the same filesystem, which is what
    /// `clonefile`/`FICLONE` require.
    pub same_filesystem: bool,
}

impl Capabilities {
    /// Whether a `Clone` rule can actually clone here, rather than fall back.
    #[must_use]
    pub const fn can_clone(self) -> bool {
        self.supports_clone && self.same_filesystem
    }
}

/// Everything one apply or preview needs to know about where it is working.
#[derive(Clone, Debug)]
pub struct ShareContext {
    /// The workspace the files come from: the project's `Main` checkout.
    pub source: PathBuf,
    /// The workspace being provisioned. May equal `source` (P4: the main
    /// checkout is a workspace like any other).
    pub target: PathBuf,
    /// `<git common dir>/forge/shared`, when the project is a git repository.
    pub store: Option<PathBuf>,
    /// `<git common dir>/forge/backups`.
    pub backups: Option<PathBuf>,
    /// What the filesystem can do.
    pub caps: Capabilities,
    /// The user asked for these rules by name, so an existing file may be
    /// backed up and replaced. A background run never overwrites.
    pub explicit: bool,
}

impl ShareContext {
    /// Where a `Link` rule's real file lives.
    #[must_use]
    pub fn store_path(&self, relative: &str) -> Option<PathBuf> {
        self.store.as_ref().map(|store| store.join(relative))
    }

    /// Whether the target workspace is the source workspace.
    #[must_use]
    pub fn target_is_source(&self) -> bool {
        self.source == self.target
    }
}
