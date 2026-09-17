//! Disk directory reads, a separate Git-backed navigation index, and file operations.
//! The daemon calls this service outside its core lock; paths stay inside the checkout.
//! Content writes require a revision and replace atomically (ADR-012).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use git_service::{discover_root, run_git, run_git_bounded, GitError};
use thiserror::Error;

mod entry_move;

/// Soft ceiling for one file's contents on the wire.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Ceiling for one image read. The daemon ships it as base64 — a third larger —
/// inside a frame capped at 16 MiB, so this leaves room for the envelope.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Soft ceiling for how many paths a tree listing returns.
pub const MAX_TREE_ENTRIES: usize = 10_000;
const MAX_TREE_BYTES: usize = 1024 * 1024;

/// Soft ceiling for one [`list_directory`] answer (a single folder's children).
pub const MAX_DIRECTORY_ENTRIES: usize = 2_000;

pub const MAX_DIRECTORY_PATH_BYTES: usize = 4096;
/// Conservative JSON wire-size budget, including worst-case string escaping.
pub const MAX_DIRECTORY_BYTES: usize = 1024 * 1024;

/// Soft ceiling for search hits.
pub const MAX_SEARCH_RESULTS: usize = 200;

/// Lines of context kept either side of a content hit.
pub const SEARCH_CONTEXT_LINES: usize = 3;

/// A context line is orientation, not content: one minified line must not ride
/// along once per neighbouring hit.
const MAX_CONTEXT_LINE_BYTES: usize = 512;

/// Package-manager / language dependency directories the file tree never names.
///
/// Matched on a path component (case-insensitive), the same set
/// `daemon::shares::detect` buckets as `ShareClass::Dependencies`. Build
/// output (`dist`, `target`, …) stays visible as an opaque ignored row so the
/// GUI can peel it with [`list_directory`].
fn is_dependency_dir_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "node_modules" | ".venv" | "venv" | "vendor" | "pods" | ".bundle" | "bower_components"
    )
}

/// True when any path component is a [`is_dependency_dir_name`].
fn path_under_dependency_dir(rel: &str) -> bool {
    rel.split('/')
        .filter(|s| !s.is_empty())
        .any(is_dependency_dir_name)
}

/// Raw `git grep` hits a definition search will read before giving up.
///
/// A definition search greps for the bare word and throws most of it away, so
/// its cap cannot be the one on the answer: `new` matches thousands of lines
/// and none of the first two hundred need be a declaration. This bounds the
/// parse instead, and the excess is reported as `truncated`.
const MAX_DEFINITION_SCAN: usize = 5_000;

/// Bytes inspected for a `NUL` when deciding whether a file is binary.
const BINARY_PROBE: usize = 8 * 1024;

/// What went wrong talking to the workspace filesystem.
#[derive(Debug, Error)]
pub enum FsError {
    /// The path escaped the workspace root.
    #[error("path escapes workspace: {0}")]
    EscapesWorkspace(String),
    /// The path is already there, and this operation will not overwrite it.
    #[error("path already exists: {0}")]
    AlreadyExists(String),
    /// The path does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The file is larger than [`MAX_FILE_BYTES`].
    #[error("file too large ({size} bytes, limit {limit})")]
    TooLarge {
        /// Actual size in bytes.
        size: u64,
        /// Configured limit.
        limit: usize,
    },
    /// The file contains a `NUL` in its probe window.
    #[error("binary file")]
    Binary,
    /// [`read_image`] was asked for a path without an image extension.
    #[error("not an image: {0}")]
    NotAnImage(String),
    /// The on-disk content no longer matches the expected revision.
    #[error("file changed on disk")]
    RevisionMismatch {
        /// What is on disk now.
        current: FileContents,
    },
    /// A git subprocess failed for a listing or search.
    #[error(transparent)]
    Git(#[from] GitError),
    /// An IO error reading or writing.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Whether a listed path is a file or a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file (or symlink to one).
    File,
    /// A directory.
    Directory,
}

/// One path relative to the workspace root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Workspace-relative path, forward-slashed.
    pub path: String,
    /// File or directory.
    pub kind: EntryKind,
    /// Excluded by `.gitignore`. Always `false` from the `read_dir` fallback,
    /// which has no exclude rules to consult.
    pub ignored: bool,
    pub symlink: Option<SymlinkTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymlinkTarget {
    File,
    Directory,
    External,
    Broken,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

/// A bounded listing of paths under a workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTree {
    /// Relative paths, files only from `git ls-files`; directories are inferred
    /// by the GUI when it builds the tree.
    pub entries: Vec<FileEntry>,
    /// Whether entries were dropped for exceeding [`MAX_TREE_ENTRIES`].
    pub truncated: bool,
}

/// The text of one file, plus the revision a later write must present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileContents {
    /// Workspace-relative path.
    pub path: String,
    /// UTF-8 text. Empty when [`FileContents::binary`] or
    /// [`FileContents::too_large`].
    pub text: String,
    /// Hash of the on-disk bytes; required by [`write_file`].
    pub revision: String,
    /// A language hint from the extension (`"rust"`, `"typescript"`, …), or
    /// empty when unknown.
    pub language: String,
    /// True when a `NUL` was found in the probe window, or the bytes are not
    /// valid UTF-8. Both mean the same thing to a caller: there is no text
    /// here that can be edited and written back without changing the file.
    pub binary: bool,
    /// True when the file exceeded [`MAX_FILE_BYTES`] and was not opened.
    pub too_large: bool,
}

/// The bytes of one image, for a Markdown preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageBytes {
    /// Workspace-relative path.
    pub path: String,
    /// Media type from the extension (`image/png`, `image/svg+xml`, …).
    pub mime: &'static str,
    /// The whole file.
    pub bytes: Vec<u8>,
}

/// Name match or content match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchKind {
    /// Subsequence match against the relative path.
    Name,
    /// Fixed-string line match against file contents (`git grep -F`).
    Content,
    /// Lines that declare the queried symbol (`git grep -w -F`, then filtered).
    Definition,
}

/// One hit from [`search_files`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    /// Workspace-relative path.
    pub path: String,
    /// 1-based line, or `0` for a name match.
    pub line: u32,
    /// 1-based column, or `0` when unknown / name match.
    pub column: u32,
    /// The matching line (content) or the path (name).
    pub text: String,
    /// Up to [`SEARCH_CONTEXT_LINES`] lines directly above a content hit, in
    /// file order; empty for name and definition hits.
    pub before: Vec<String>,
    /// Up to [`SEARCH_CONTEXT_LINES`] lines directly below a content hit.
    pub after: Vec<String>,
}

/// Bounded search results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResults {
    /// Hits, capped at [`MAX_SEARCH_RESULTS`].
    pub matches: Vec<SearchMatch>,
    /// Whether more hits existed beyond the cap.
    pub truncated: bool,
}

/// List every tracked and untracked-but-not-ignored file under `root`.
///
/// Prefers `git ls-files`. Falls back to a recursive `read_dir` when `root` is
/// not inside a git repository. Wholly-ignored directories (except language
/// dependency dirs, which are omitted) arrive as one [`EntryKind::Directory`]
/// row so the GUI can peel them with [`list_directory`].
pub fn list_files(root: &Path) -> Result<FileTree, FsError> {
    let root = canonicalize_root(root)?;
    if is_git_repo(&root) {
        list_via_git(&root)
    } else {
        list_via_walk(&root)
    }
}

/// List the immediate children of one directory under `root`.
///
/// Empty means root. Symlink aliases remain in returned paths, but expansion
/// and reads must resolve inside the checkout. Permission errors propagate.
pub fn list_directory(root: &Path, relative: &str) -> Result<DirectoryListing, FsError> {
    if relative.len() > MAX_DIRECTORY_PATH_BYTES {
        return Err(FsError::TooLarge {
            size: relative.len() as u64,
            limit: MAX_DIRECTORY_PATH_BYTES,
        });
    }
    let root = canonicalize_root(root)?;
    let dir = resolve_inside(&root, relative)?;
    let rel = Path::new(relative)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if path_under_dependency_dir(&rel) || rel.split('/').any(|p| p == ".git") {
        return Ok(DirectoryListing {
            path: rel,
            entries: Vec::new(),
            truncated: false,
        });
    }
    let meta = fs::metadata(&dir).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound(rel.clone())
        } else {
            FsError::Io(e)
        }
    })?;
    if !meta.is_dir() {
        return Err(FsError::NotFound(rel));
    }

    let read = fs::read_dir(&dir)?;
    let mut entries = Vec::new();
    let mut bytes = rel.len() * 6 + 128;
    let mut truncated = false;
    for entry in read {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            truncated = true;
            continue;
        };
        if name == ".git" || is_dependency_dir_name(name) {
            continue;
        }
        if entries.len() >= MAX_DIRECTORY_ENTRIES {
            truncated = true;
            break;
        }
        let path_bytes = rel.len() + usize::from(!rel.is_empty()) + name.len();
        let entry_bytes = path_bytes * 6 + 128;
        if path_bytes > MAX_DIRECTORY_PATH_BYTES || bytes + entry_bytes > MAX_DIRECTORY_BYTES {
            truncated = true;
            break;
        }
        let child_rel = if rel.is_empty() {
            name.to_owned()
        } else {
            format!("{rel}/{name}")
        };
        let ft = entry.file_type()?;
        let symlink = ft
            .is_symlink()
            .then(|| symlink_target(&root, &entry.path()));
        let kind = if ft.is_dir() || symlink == Some(SymlinkTarget::Directory) {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        bytes += entry_bytes;
        entries.push(FileEntry {
            path: child_rel,
            kind,
            ignored: false,
            symlink,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let ignored = if is_git_repo(&root) {
        ignored_paths_among(
            &dir,
            entries
                .iter()
                .map(|e| e.path.rsplit('/').next().unwrap_or(&e.path)),
        )?
    } else {
        HashSet::new()
    };

    for entry in &mut entries {
        entry.ignored = ignored.contains(entry.path.rsplit('/').next().unwrap_or(&entry.path));
    }
    Ok(DirectoryListing {
        path: rel,
        entries,
        truncated,
    })
}

fn symlink_target(root: &Path, path: &Path) -> SymlinkTarget {
    match fs::canonicalize(path) {
        Ok(target) if !target.starts_with(root) => SymlinkTarget::External,
        Ok(target) => match fs::metadata(target) {
            Ok(meta) if meta.is_dir() => SymlinkTarget::Directory,
            Ok(meta) if meta.is_file() => SymlinkTarget::File,
            _ => SymlinkTarget::Unavailable,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SymlinkTarget::Broken,
        Err(_) => SymlinkTarget::Unavailable,
    }
}

/// Paths `git check-ignore` reports as ignored, among `candidates`.
fn ignored_paths_among<'a>(
    root: &Path,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Result<HashSet<String>, FsError> {
    let paths: Vec<&str> = candidates.into_iter().collect();
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut ignored = HashSet::new();
    // Each answer only echoes these bounded candidates (at most 4x for quoting),
    // so Git cannot recursively accumulate ignored descendants before the cap.
    for chunk in paths.chunks(128) {
        let literal: Vec<_> = chunk.iter().map(|path| format!("./{path}")).collect();
        let mut args: Vec<&str> = Vec::with_capacity(4 + chunk.len());
        args.extend(["-c", "core.quotePath=true"]);
        args.push("check-ignore");
        args.push("--");
        args.extend(literal.iter().map(String::as_str));
        let out = run_git(Some(root), &args)?;
        if out.status != 0 && out.status != 1 {
            out.ok(&args)?;
            continue;
        }
        for (candidate, literal) in chunk.iter().zip(&literal) {
            let quoted = git_quoted_path(literal);
            if out.stdout.lines().any(|path| path == quoted) {
                ignored.insert((*candidate).to_string());
            }
        }
    }
    Ok(ignored)
}

fn git_quoted_path(path: &str) -> String {
    if !path
        .bytes()
        .any(|b| !(32..127).contains(&b) || b == b'"' || b == b'\\')
    {
        return path.to_owned();
    }
    let mut quoted = String::with_capacity(path.len() * 4 + 2);
    quoted.push('"');
    for b in path.bytes() {
        match b {
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            b'\t' => quoted.push_str("\\t"),
            7 => quoted.push_str("\\a"),
            8 => quoted.push_str("\\b"),
            11 => quoted.push_str("\\v"),
            12 => quoted.push_str("\\f"),
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            32..=126 => quoted.push(char::from(b)),
            _ => {
                quoted.push('\\');
                quoted.push(char::from(b'0' + (b >> 6)));
                quoted.push(char::from(b'0' + ((b >> 3) & 7)));
                quoted.push(char::from(b'0' + (b & 7)));
            }
        }
    }
    quoted.push('"');
    quoted
}

/// Read one file relative to `root`, rejecting binaries and oversize files.
pub fn read_file(root: &Path, relative: &str) -> Result<FileContents, FsError> {
    let root = canonicalize_root(root)?;
    let path = resolve_inside(&root, relative)?;
    let meta = fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound(relative.to_string())
        } else {
            FsError::Io(e)
        }
    })?;
    if !meta.is_file() {
        return Err(FsError::NotFound(relative.to_string()));
    }
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Ok(FileContents {
            path: normalize_rel(relative),
            text: String::new(),
            revision: String::new(),
            language: language_for(relative).to_string(),
            binary: false,
            too_large: true,
        });
    }
    let mut buf = Vec::with_capacity(meta.len() as usize + 1);
    // The file can grow between `metadata` and the read; `take` holds the cap,
    // and a result over it is the same answer as an oversize `metadata`.
    File::open(&path)?
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut buf)?;
    if buf.len() > MAX_FILE_BYTES {
        return Ok(FileContents {
            path: normalize_rel(relative),
            text: String::new(),
            revision: String::new(),
            language: language_for(relative).to_string(),
            binary: false,
            too_large: true,
        });
    }
    // A lossy decode is reported as binary rather than repaired: the editor
    // writes `text` back, and U+FFFD in place of a byte it could not read
    // would rewrite the file with content that was never in it.
    let text = match String::from_utf8(buf) {
        Ok(text) if !is_binary(text.as_bytes()) => text,
        Ok(text) => {
            return Ok(FileContents {
                path: normalize_rel(relative),
                text: String::new(),
                revision: revision_of(text.as_bytes()),
                language: language_for(relative).to_string(),
                binary: true,
                too_large: false,
            })
        }
        Err(error) => {
            return Ok(FileContents {
                path: normalize_rel(relative),
                text: String::new(),
                revision: revision_of(error.as_bytes()),
                language: language_for(relative).to_string(),
                binary: true,
                too_large: false,
            })
        }
    };
    Ok(FileContents {
        path: normalize_rel(relative),
        revision: revision_of(text.as_bytes()),
        language: language_for(relative).to_string(),
        text,
        binary: false,
        too_large: false,
    })
}

/// Read one image relative to `root`, whole or not at all.
///
/// The extension is the gate, checked before the path is touched: this hands
/// back raw bytes, and without it a preview could name `.env` and turn this
/// into a second, unrevisioned `read_file` for anything.
pub fn read_image(root: &Path, relative: &str) -> Result<ImageBytes, FsError> {
    let mime = image_mime(relative).ok_or_else(|| FsError::NotAnImage(relative.to_string()))?;
    let root = canonicalize_root(root)?;
    let path = resolve_inside(&root, relative)?;
    let meta = fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound(relative.to_string())
        } else {
            FsError::Io(e)
        }
    })?;
    if !meta.is_file() {
        return Err(FsError::NotFound(relative.to_string()));
    }
    if meta.len() > MAX_IMAGE_BYTES as u64 {
        return Err(FsError::TooLarge {
            size: meta.len(),
            limit: MAX_IMAGE_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // The file can grow between `metadata` and the read; `take` holds the cap.
    File::open(&path)?
        .take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(FsError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_IMAGE_BYTES,
        });
    }
    Ok(ImageBytes {
        path: normalize_rel(relative),
        mime,
        bytes,
    })
}

/// Media type for an image extension a WebView can draw, or `None`.
#[must_use]
pub fn image_mime(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// Write `text` to `relative` only when the on-disk revision still matches
/// `expected_revision`.
pub fn write_file(
    root: &Path,
    relative: &str,
    text: &str,
    expected_revision: &str,
) -> Result<(), FsError> {
    let root = canonicalize_root(root)?;
    let path = resolve_inside(&root, relative)?;
    if path.exists() {
        let current = read_file(&root, relative)?;
        if current.binary || current.too_large {
            return Err(FsError::RevisionMismatch { current });
        }
        if current.revision != expected_revision {
            return Err(FsError::RevisionMismatch { current });
        }
    } else if !expected_revision.is_empty() {
        // Caller thought the file existed.
        return Err(FsError::NotFound(relative.to_string()));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mode = path.metadata().ok().map(|m| m.permissions());

    let tmp = temp_path(&path)?;
    {
        // `create_new`, and a name built from the whole file name: with
        // `with_extension` a save of `a.rs` and a save of `a.md` in the same
        // directory pick the same temp path and race for it.
        let mut out = File::options().write(true).create_new(true).open(&tmp)?;
        if let Err(error) = out.write_all(text.as_bytes()).and_then(|()| out.sync_all()) {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
    }
    if let Some(perms) = mode {
        let _ = fs::set_permissions(&tmp, perms);
    }
    if let Err(error) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

/// A temp name next to `path` that no concurrent save can pick.
fn temp_path(path: &Path) -> Result<PathBuf, FsError> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let pid = std::process::id();
    for attempt in 0..64 {
        let candidate = directory.join(format!(".{name}.forge-tmp.{pid}.{attempt}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(FsError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free temporary name next to the target",
    )))
}

/// What a `CreatePath` is asked to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    /// An empty file, and any directories above it that do not exist yet.
    File,
    /// A directory, and any above it.
    Directory,
}

/// Create an empty file or a directory at `relative` (ADR-012, A11).
///
/// Refuses to overwrite. "Create" and "replace what is there" are different
/// acts and only one of them was asked for, so an existing path is an error
/// rather than a silent truncation — the GUI has no revision to condition on
/// here, and a create that clobbered an agent's file would be the one hole in
/// the write path `write_file`'s revision check exists to close.
pub fn create_path(root: &Path, relative: &str, kind: PathKind) -> Result<(), FsError> {
    let root = canonicalize_root(root)?;
    let path = resolve_inside(&root, relative)?;
    if path.exists() {
        return Err(FsError::AlreadyExists(normalize_rel(relative)));
    }
    match kind {
        PathKind::Directory => fs::create_dir_all(&path)?,
        PathKind::File => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            // `create_new` rather than `File::create`: between the check above
            // and here an agent may have written the same path, and losing its
            // work to a race is exactly what this is trying not to do.
            File::options().write(true).create_new(true).open(&path)?;
        }
    }
    Ok(())
}

/// Move workspace-relative entries without overwriting; identical paths are refused.
pub fn rename_path(root: &Path, from: &str, to: &str) -> Result<(), FsError> {
    prepare_rename(root, from, to)?.apply()
}

/// Validated entry paths; final symlinks are moved, never followed.
pub struct Rename {
    root: PathBuf,
    source: PathBuf,
    source_directory: Option<PathBuf>,
    target: PathBuf,
    from: PathBuf,
    to: PathBuf,
}

pub fn prepare_rename(root: &Path, from: &str, to: &str) -> Result<Rename, FsError> {
    const MAX_RENAME_PATH_BYTES: usize = 4096;
    for path in [from, to] {
        if path.len() > MAX_RENAME_PATH_BYTES {
            return Err(FsError::TooLarge {
                size: path.len() as u64,
                limit: MAX_RENAME_PATH_BYTES,
            });
        }
    }
    let root = canonicalize_root(root)?;
    let source = resolve_entry_inside(&root, from)?;
    let target = resolve_entry_inside(&root, to)?;
    let meta = source.symlink_metadata().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound(normalize_rel(from))
        } else {
            FsError::Io(error)
        }
    })?;
    let source_directory = if meta.is_dir() {
        Some(fs::canonicalize(&source)?)
    } else {
        None
    };
    if source == target
        || source_directory
            .as_ref()
            .is_some_and(|dir| target.starts_with(dir))
    {
        return Err(FsError::AlreadyExists(to.to_owned()));
    }
    Ok(Rename {
        root,
        source,
        source_directory,
        target,
        from: PathBuf::from(from),
        to: PathBuf::from(to),
    })
}

impl Rename {
    /// Map an open document by path components, including aliases of a moved directory.
    pub fn retarget(&self, relative: &str) -> Result<Option<String>, FsError> {
        let path = Path::new(relative);
        let mapped = if let Ok(suffix) = path.strip_prefix(&self.from) {
            Some(self.to.join(suffix))
        } else {
            // A broken alias in an unrelated editor cannot veto this validated move.
            let Ok(entry) = resolve_entry_inside(&self.root, relative) else {
                return Ok(None);
            };
            entry
                .strip_prefix(self.source_directory.as_ref().unwrap_or(&self.source))
                .ok()
                .map(|suffix| {
                    self.target
                        .strip_prefix(&self.root)
                        .unwrap_or(&self.target)
                        .join(suffix)
                })
        };
        Ok(mapped.map(|p| {
            p.components()
                .collect::<PathBuf>()
                .to_string_lossy()
                .into_owned()
        }))
    }

    /// Refuses an occupied destination atomically, including dangling symlinks.
    pub fn apply(self) -> Result<(), FsError> {
        entry_move::rename(&self.root, &self.source, &self.target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                FsError::AlreadyExists(self.to.to_string_lossy().into_owned())
            } else {
                FsError::Io(error)
            }
        })
    }
}

/// Delete `relative`, recursively for a directory (A11).
///
/// No trash and no undo: this is the checkout, and the checkout is under git —
/// which is the real undo, and a far better one than a bespoke bin the daemon
/// would have to keep, garbage-collect and explain. The GUI asks first.
pub fn delete_path(root: &Path, relative: &str) -> Result<(), FsError> {
    let root = canonicalize_root(root)?;
    let path = resolve_entry_inside(&root, relative)?;
    let meta = path.symlink_metadata().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound(normalize_rel(relative))
        } else {
            FsError::Io(error)
        }
    })?;
    if meta.is_dir() {
        fs::remove_dir_all(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// Search by file name or by content under `root`.
pub fn search_files(
    root: &Path,
    query: &str,
    kind: SearchKind,
    limit: usize,
) -> Result<SearchResults, FsError> {
    let limit = limit.clamp(1, MAX_SEARCH_RESULTS);
    let query = query.trim();
    if query.is_empty() {
        return Ok(SearchResults {
            matches: Vec::new(),
            truncated: false,
        });
    }
    match kind {
        SearchKind::Name => search_by_name(root, query, limit),
        SearchKind::Content => search_by_content(root, query, limit),
        SearchKind::Definition => search_by_definition(root, query, limit),
    }
}

/// Content hash used as an optimistic-concurrency token.
#[must_use]
pub fn revision_of(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Language id for `InputState::code_editor`, keyed on the file extension.
#[must_use]
pub fn language_for(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "py" | "pyi" | "pyw" => "python",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "javascript",
        "json" => "json",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        _ => "",
    }
}

fn list_via_git(root: &Path) -> Result<FileTree, FsError> {
    /*
     * `--cached` answers from the *index*, not from the worktree, and nothing
     * that removes a file touches the index on its own: a `rm`, an agent
     * deleting a path in its terminal, and this crate's own `delete_path` all
     * leave the entry behind. Listing the index alone is what kept a deleted
     * `plan.md` — and every file under a deleted directory — in the tree until
     * the deletion was staged.
     *
     * `--deleted` is the set of index entries whose file is gone from the
     * worktree, so subtracting it makes the listing describe the disk. It is a
     * second subprocess rather than a `-t` tag pass because the tags would
     * still need this subtraction and would have to be parsed out of the same
     * NUL-separated stream.
     */
    let gone = run_git_bounded(Some(root), &["ls-files", "--deleted", "-z"], MAX_TREE_BYTES)?;
    let deleted: HashSet<&str> = gone
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();

    let out = run_git_bounded(
        Some(root),
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        MAX_TREE_BYTES,
    )?;
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut bytes = 0;
    // An unmerged path is in the index once per stage, so `--cached` names it
    // up to three times; the tree wants one row.
    let mut seen = HashSet::new();
    for path in out.stdout.split('\0') {
        if path.is_empty() {
            continue;
        }
        if entries.len() >= MAX_TREE_ENTRIES {
            truncated = true;
            break;
        }
        if path.ends_with('/') || deleted.contains(path) {
            continue;
        }
        let entry_bytes = path.len() * 6 + 128;
        if bytes + entry_bytes > MAX_TREE_BYTES {
            truncated = true;
            break;
        }
        let path = normalize_rel(path);
        if path_under_dependency_dir(&path) {
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        if let Some(entry) = index_entry(root, path, false)? {
            bytes += entry_bytes;
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    /*
     * A third pass rather than a fourth flag on the second: `--others -i` is
     * the only `ls-files` spelling that reports *excluded* untracked files, and
     * it cannot be combined with the un-excluded `--others` above — the two
     * are complementary halves of the same set, not a superset and a subset.
     *
     * `--directory` is what makes the pass affordable, and it is not optional.
     * Without it this repository answers with 385 751 ignored paths, almost
     * all of them inside `node_modules` and `target`; the budget is 10 000, so
     * the listing truncates on build output before it has said anything about
     * the source. With it the same repository answers with twelve: a directory
     * ignored *whole* comes back as its own name with a trailing slash,
     * standing for contents nothing needs to walk, while a file ignored on its
     * own — the `.env` someone actually wants to open — still arrives by name.
     *
     * Language dependency directories (`node_modules`, `vendor`, …) are dropped
     * entirely: they are never useful in the tree and would only be peeled for
     * curiosity. Build output and local excludes (`dist`, `plan`, …) stay as
     * one opaque row for [`list_directory`].
     *
     * Appended after the tracked rows and never interleaved, so the budget is
     * spent on the work first.
     */
    if !truncated {
        let ignored = run_git_bounded(
            Some(root),
            &[
                "ls-files",
                "--others",
                "-i",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
                "-z",
            ],
            MAX_TREE_BYTES,
        )?;
        let mut extra = Vec::new();
        for path in ignored.stdout.split('\0') {
            if path.is_empty() {
                continue;
            }
            if entries.len() + extra.len() >= MAX_TREE_ENTRIES {
                truncated = true;
                break;
            }
            let entry_bytes = path.len() * 6 + 128;
            if bytes + entry_bytes > MAX_TREE_BYTES {
                truncated = true;
                break;
            }
            let path = normalize_rel(path.trim_end_matches('/'));
            if path.is_empty() || path_under_dependency_dir(&path) || !seen.insert(path.clone()) {
                continue;
            }
            if let Some(entry) = index_entry(root, path, true)? {
                bytes += entry_bytes;
                extra.push(entry);
            }
        }
        extra.sort_by(|a, b| a.path.cmp(&b.path));
        entries.append(&mut extra);
    }

    Ok(FileTree { entries, truncated })
}

fn index_entry(root: &Path, path: String, ignored: bool) -> Result<Option<FileEntry>, FsError> {
    let absolute = root.join(&path);
    let meta = match absolute.symlink_metadata() {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let symlink = meta.is_symlink().then(|| symlink_target(root, &absolute));
    let kind = if meta.is_dir() || symlink == Some(SymlinkTarget::Directory) {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    Ok(Some(FileEntry {
        path,
        kind,
        ignored,
        symlink,
    }))
}

fn list_via_walk(root: &Path) -> Result<FileTree, FsError> {
    let mut entries = Vec::new();
    let mut truncated = false;
    walk(root, root, &mut entries, &mut truncated, &mut 0)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(FileTree { entries, truncated })
}

fn walk(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<FileEntry>,
    truncated: &mut bool,
    bytes: &mut usize,
) -> Result<(), FsError> {
    if *truncated {
        return Ok(());
    }
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => return Err(FsError::Io(e)),
    };
    for entry in read {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let name = name.to_string_lossy();
        if is_dependency_dir_name(&name) {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(root, &path, entries, truncated, bytes)?;
            if *truncated {
                return Ok(());
            }
        } else if ft.is_file() || ft.is_symlink() {
            if entries.len() >= MAX_TREE_ENTRIES {
                *truncated = true;
                return Ok(());
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let entry_bytes = rel.len() * 6 + 128;
            if *bytes + entry_bytes > MAX_TREE_BYTES {
                *truncated = true;
                return Ok(());
            }
            *bytes += entry_bytes;
            entries.push(FileEntry {
                path: rel,
                kind: if ft.is_symlink() && symlink_target(root, &path) == SymlinkTarget::Directory
                {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                ignored: false,
                symlink: ft.is_symlink().then(|| symlink_target(root, &path)),
            });
        }
    }
    Ok(())
}

fn search_by_name(root: &Path, query: &str, limit: usize) -> Result<SearchResults, FsError> {
    let tree = list_files(root)?;
    let mut scored: Vec<(i32, usize, SearchMatch)> = Vec::new();
    for (order, entry) in tree.entries.into_iter().enumerate() {
        // The tree lists ignored files so they can be opened; a name search is
        // for the work, and `git grep` below already excludes them.
        if entry.ignored || entry.kind != EntryKind::File {
            continue;
        }
        let Some(score) = fuzzy_score(&entry.path, query) else {
            continue;
        };
        scored.push((
            score,
            order,
            SearchMatch {
                text: entry.path.clone(),
                path: entry.path,
                line: 0,
                column: 0,
                before: Vec::new(),
                after: Vec::new(),
            },
        ));
    }
    // Best first, and ties in listing order so a query that matches a whole
    // directory does not reshuffle it on the next keystroke.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let truncated = tree.truncated || scored.len() > limit;
    scored.truncate(limit);
    Ok(SearchResults {
        matches: scored.into_iter().map(|(_, _, hit)| hit).collect(),
        truncated,
    })
}

fn search_by_content(root: &Path, query: &str, limit: usize) -> Result<SearchResults, FsError> {
    let root = canonicalize_root(root)?;
    if !is_git_repo(&root) {
        return search_content_walk(&root, query, limit);
    }
    // Fixed-string (`-F`): find-in-project takes typed text, not a regex. `-e`
    // still wraps the needle so a query that starts with `-` is not a flag.
    // `git grep` exits 1 when there are no matches — that is success.
    let out = run_git(
        Some(&root),
        &[
            "grep",
            "-n",
            "-I",
            "--no-color",
            "--untracked",
            "-z",
            "-F",
            "-C",
            CONTEXT_ARG,
            "-e",
            query,
            "--",
            ".",
        ],
    )?;
    if out.status != 0 && out.status != 1 {
        return Err(FsError::Git(GitError::CommandFailed {
            args: vec![
                "grep".into(),
                "-n".into(),
                "-I".into(),
                "--no-color".into(),
                "--untracked".into(),
                "-z".into(),
                "-F".into(),
                "-e".into(),
                query.into(),
            ],
            stderr: out.stderr,
            status: out.status,
        }));
    }
    if out.status == 1 || out.stdout.is_empty() {
        return Ok(SearchResults {
            matches: Vec::new(),
            truncated: false,
        });
    }

    Ok(parse_grep_z(out.stdout.as_bytes(), Some(query), limit))
}

/// `-C` for [`SEARCH_CONTEXT_LINES`]; a literal because `run_git` takes `&str`s.
const CONTEXT_ARG: &str = "3";

fn search_content_walk(root: &Path, query: &str, limit: usize) -> Result<SearchResults, FsError> {
    let tree = list_via_walk(root)?;
    let mut matches = Vec::new();
    let mut truncated = tree.truncated;
    for entry in tree.entries {
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        let Ok(contents) = read_file(root, &entry.path) else {
            continue;
        };
        if contents.binary || contents.too_large {
            continue;
        }
        let lines: Vec<&str> = contents.text.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if let Some(col) = line.find(query) {
                if matches.len() >= limit {
                    truncated = true;
                    break;
                }
                let above = idx.saturating_sub(SEARCH_CONTEXT_LINES);
                let below = (idx + 1 + SEARCH_CONTEXT_LINES).min(lines.len());
                matches.push(SearchMatch {
                    path: entry.path.clone(),
                    line: (idx + 1) as u32,
                    column: (col + 1) as u32,
                    text: (*line).to_string(),
                    before: lines[above..idx].iter().map(|l| context_line(l)).collect(),
                    after: lines[idx + 1..below]
                        .iter()
                        .map(|l| context_line(l))
                        .collect(),
                });
            }
        }
    }
    Ok(SearchResults { matches, truncated })
}

fn context_line(line: &str) -> String {
    if line.len() <= MAX_CONTEXT_LINE_BYTES {
        return line.to_string();
    }
    let mut cut = MAX_CONTEXT_LINE_BYTES;
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    line[..cut].to_string()
}

/// Read `git grep -z -n` output into hits, `limit` of them at most.
///
/// With `-C`, git marks a context line exactly as it marks a hit once `-z` is
/// on, so `query` is what tells them apart: the grep was `-F`, so a line is a
/// hit exactly when it contains the needle. `None` takes every record as a hit.
fn parse_grep_z(stdout: &[u8], query: Option<&str>, limit: usize) -> SearchResults {
    // Records are path\0line\0text\n; `--\n` separates non-adjacent groups.
    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut truncated = false;
    let text = String::from_utf8_lossy(stdout);
    let mut remaining = text.as_ref();
    let mut recent: VecDeque<(u32, &str)> = VecDeque::new();
    let mut recent_path = "";
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix("--\n") {
            remaining = rest;
            continue;
        }
        let Some((path, rest)) = remaining.split_once('\0') else {
            break;
        };
        let Some((line_s, rest)) = rest.split_once('\0') else {
            break;
        };
        let (line_text, rest) = rest.split_once('\n').unwrap_or((rest, ""));
        remaining = rest;
        let Ok(line) = line_s.parse::<u32>() else {
            continue;
        };
        if path != recent_path {
            recent.clear();
            recent_path = path;
        }
        let path_norm = path.to_owned();
        let hit = query.is_none_or(|needle| line_text.contains(needle));
        if let Some(last) = matches.last().filter(|_| matches.len() >= limit) {
            // Past the cap, a record is either the tail of the last hit's
            // context or proof that another hit follows.
            let trailing = !hit
                && last.path == path_norm
                && line as usize <= last.line as usize + SEARCH_CONTEXT_LINES;
            if !trailing {
                truncated = true;
                break;
            }
        }
        for earlier in matches.iter_mut().rev() {
            if earlier.path != path_norm || earlier.line >= line {
                break;
            }
            let next = earlier.line as usize + earlier.after.len() + 1;
            if next > earlier.line as usize + SEARCH_CONTEXT_LINES {
                break;
            }
            if next == line as usize {
                earlier.after.push(context_line(line_text));
            }
        }
        if hit && matches.len() < limit {
            let before = recent
                .iter()
                .filter(|(at, _)| {
                    *at < line && (*at as usize) + SEARCH_CONTEXT_LINES >= line as usize
                })
                .map(|(_, text)| context_line(text))
                .collect();
            matches.push(SearchMatch {
                path: path_norm,
                line,
                column: 0,
                text: line_text.to_string(),
                before,
                after: Vec::new(),
            });
        }
        recent.push_back((line, line_text));
        if recent.len() > SEARCH_CONTEXT_LINES {
            recent.pop_front();
        }
    }
    SearchResults { matches, truncated }
}

/* --------------------------------------------------------- definitions --- */

/// Keywords that make the word after them a declaration beyond doubt.
const DECLARING_KEYWORDS: &[&str] = &[
    "fn",
    "func",
    "function",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "impl",
    "type",
    "typedef",
    "def",
    "defn",
    "mod",
    "module",
    "namespace",
    "package",
    "union",
    "macro_rules",
    "record",
    "object",
    "protocol",
    "extension",
    "sub",
    "proc",
];

/// Keywords that bind a name to a value. A declaration, but often a local one.
const BINDING_KEYWORDS: &[&str] = &["const", "let", "var", "static", "val"];

/// Words that may stand between the start of a line and a declared name.
const MODIFIERS: &[&str] = &[
    "pub",
    "export",
    "default",
    "async",
    "public",
    "private",
    "protected",
    "internal",
    "override",
    "final",
    "abstract",
    "readonly",
    "static",
    "declare",
    "virtual",
    "inline",
    "extern",
    "unsafe",
    "get",
    "set",
];

/// Ranks, best first. A sort key, never sent over the wire.
const RANK_DECLARING: u8 = 0;
const RANK_BINDING: u8 = 1;
const RANK_HEAD: u8 = 2;

/// Search for the lines that *declare* `symbol`.
///
/// One `git grep -w -F` for the bare word, then a filter in this process. The
/// grep is asked for a fixed string rather than a pattern for two reasons: a
/// symbol that arrives from a click in the editor must never be able to become
/// a regex, and `\b` is a GNU extension that git's bundled regex engine does
/// not portably have — so the word boundary is git's own `-w` and the language
/// heuristic is [`definition_rank`], which a test can read.
fn search_by_definition(root: &Path, symbol: &str, limit: usize) -> Result<SearchResults, FsError> {
    if !is_identifier(symbol) {
        return Ok(SearchResults {
            matches: Vec::new(),
            truncated: false,
        });
    }
    let root = canonicalize_root(root)?;
    let raw = if is_git_repo(&root) {
        grep_word(&root, symbol)?
    } else {
        search_content_walk(&root, symbol, MAX_DEFINITION_SCAN)?
    };
    Ok(declarations(raw, symbol, limit))
}

/// Whether `symbol` is a name a language could have declared.
///
/// The gate that keeps the grep a fixed-string search: anything else — a
/// space, a quote, a metacharacter — is answered with no hits rather than
/// handed to git.
fn is_identifier(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if symbol.len() > 128 {
        return false;
    }
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn grep_word(root: &Path, symbol: &str) -> Result<SearchResults, FsError> {
    // `git grep` exits 1 when there are no matches — that is success.
    let out = run_git(
        Some(root),
        &[
            "grep",
            "-n",
            "-I",
            "--no-color",
            "--untracked",
            "-z",
            "-w",
            "-F",
            "-e",
            symbol,
            "--",
            ".",
        ],
    )?;
    if out.status != 0 && out.status != 1 {
        return Err(FsError::Git(GitError::CommandFailed {
            args: vec!["grep".into(), "-w".into(), "-F".into(), symbol.into()],
            stderr: out.stderr,
            status: out.status,
        }));
    }
    if out.status == 1 || out.stdout.is_empty() {
        return Ok(SearchResults {
            matches: Vec::new(),
            truncated: false,
        });
    }
    Ok(parse_grep_z(
        out.stdout.as_bytes(),
        None,
        MAX_DEFINITION_SCAN,
    ))
}

/// Keep the hits that declare `symbol`, best kind of declaration first.
fn declarations(raw: SearchResults, symbol: &str, limit: usize) -> SearchResults {
    let scanned = raw.truncated;
    let mut scored: Vec<(u8, usize, SearchMatch)> = Vec::new();
    for (order, hit) in raw.matches.into_iter().enumerate() {
        let Some((column, rank)) = definition_rank(&hit.text, symbol) else {
            continue;
        };
        scored.push((
            rank,
            order,
            SearchMatch {
                path: hit.path,
                line: hit.line,
                column,
                text: hit.text,
                before: Vec::new(),
                after: Vec::new(),
            },
        ));
    }
    // Ties keep grep's order, which is path then line: a reshuffle between two
    // equally good answers would move the one under the cursor.
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let truncated = scanned || scored.len() > limit;
    scored.truncate(limit);
    SearchResults {
        matches: scored.into_iter().map(|(_, _, hit)| hit).collect(),
        truncated,
    }
}

/// Returns the 1-based column and rank of a declaration candidate.
fn definition_rank(line: &str, symbol: &str) -> Option<(u32, u8)> {
    let mut best: Option<(u32, u8)> = None;
    for (index, _) in line.match_indices(symbol) {
        if !is_whole_word(line, index, symbol.len()) {
            continue;
        }
        let Some(rank) = rank_at(&line[..index], &line[index + symbol.len()..]) else {
            continue;
        };
        let column = line[..index].chars().count() as u32 + 1;
        if best.is_none_or(|(_, previous)| rank < previous) {
            best = Some((column, rank));
        }
    }
    best
}

fn rank_at(before: &str, after: &str) -> Option<u8> {
    let head = before.trim_end();
    let binding_head = head.strip_suffix("mut").map(str::trim_end);
    if binding_head
        .is_some_and(|prefix| matches!(prefix.split_whitespace().last(), Some("let" | "static")))
    {
        return Some(RANK_BINDING);
    }
    let previous = head.rsplit(char::is_whitespace).next().unwrap_or("");
    let previous = previous.strip_suffix(['!', '*']).unwrap_or(previous);
    let previous = previous.split('<').next().unwrap_or(previous);
    if DECLARING_KEYWORDS.contains(&previous) {
        return Some(RANK_DECLARING);
    }
    if BINDING_KEYWORDS.contains(&previous) {
        return Some(RANK_BINDING);
    }

    // The remaining two shapes are both "a name that opens a signature".
    let after = after.trim_start();
    let after = after.strip_prefix('?').unwrap_or(after);
    let signature = signature_tail(after)?;
    if !signature.starts_with('(') {
        return None;
    }
    // A head that already assigned, called or ended a statement is a body, not
    // a signature: `function outer() { return foo(` must not read as a
    // declaration of `foo`.
    if head.contains(['{', '=', '.', ';']) {
        return None;
    }
    let mut words = head.split_whitespace().map(keyword_token);
    let first = words.next().unwrap_or("");
    if DECLARING_KEYWORDS.contains(&first) {
        return Some(RANK_DECLARING);
    }
    let bare =
        first.is_empty() || MODIFIERS.contains(&first) && words.all(|w| MODIFIERS.contains(&w));
    if bare && method_declaration(signature) {
        return Some(RANK_HEAD);
    }
    None
}

// A call ends at `)`; a method continues with a body or a return type.
fn method_declaration(signature: &str) -> bool {
    let mut depth = 0usize;
    for (index, character) in signature.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
                if depth == 0 {
                    let tail = signature[index + 1..].trim_start();
                    return tail.starts_with('{') || tail.starts_with(':');
                }
            }
            _ => {}
        }
    }
    false
}

// Generic parameters can contain nested types and function constraints.
fn signature_tail(after: &str) -> Option<&str> {
    let after = after.trim_start();
    if !after.starts_with('<') {
        return Some(after);
    }
    let mut depth = 0usize;
    for (index, character) in after.char_indices() {
        match character {
            '<' => depth += 1,
            '>' if !after[..index].ends_with('=') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(after[index + 1..].trim_start());
                }
            }
            _ => {}
        }
    }
    None
}

/// The identifier inside a token: `impl<T>` is `impl`, `macro_rules!` is
/// `macro_rules`, `(` is nothing.
fn keyword_token(raw: &str) -> &str {
    let start = raw
        .find(|c: char| c.is_alphanumeric() || c == '_')
        .unwrap_or(raw.len());
    let rest = &raw[start..];
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    &rest[..end]
}

fn is_whole_word(line: &str, index: usize, len: usize) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let before = line[..index].chars().next_back();
    let after = line[index + len..].chars().next();
    !before.is_some_and(word) && !after.is_some_and(word)
}

/// A match at the start of a path segment or a camelCase word.
const WORD_START_BONUS: i32 = 8;
/// A match immediately after the previous one.
const RUN_BONUS: i32 = 4;
/// How many characters of path cost one point, so a short path wins a tie.
const LENGTH_PENALTY_PER: usize = 8;

/// How well `path` answers `query`, or `None` when it does not.
///
/// The formula is the one the palette already uses in
/// `apps/tauri/src/palette/fuzzy.ts` — a point per matched character, a bonus
/// for landing on a word boundary, a bonus for a run, and a penalty for
/// length. It is duplicated rather than shared because the two run in
/// different languages on different sides of a socket; what matters is that
/// they *agree*, so `fuzzy_score_matches_the_palette` pins the cases that
/// distinguish it from any other reasonable scorer.
fn fuzzy_score(path: &str, query: &str) -> Option<i32> {
    let needle: Vec<char> = query.to_lowercase().chars().filter(|c| *c != ' ').collect();
    if needle.is_empty() {
        return Some(0);
    }

    let raw: Vec<char> = path.chars().collect();
    let hay: Vec<char> = path.to_lowercase().chars().collect();
    // Lowercasing can expand a character; bonuses belong to its first folded scalar.
    let boundaries: Vec<bool> = raw
        .iter()
        .enumerate()
        .flat_map(|(i, c)| c.to_lowercase().enumerate().map(move |(part, _)| (i, part)))
        .map(|(i, part)| part == 0 && starts_word(&raw, i))
        .collect();

    let mut score = 0i32;
    let mut needle_index = 0usize;
    let mut previous_match: isize = -2;

    for index in 0..hay.len() {
        if needle_index >= needle.len() {
            break;
        }
        if hay[index] != needle[needle_index] {
            continue;
        }
        score += 1;
        if boundaries[index] {
            score += WORD_START_BONUS;
        }
        if previous_match == index as isize - 1 {
            score += RUN_BONUS;
        }
        previous_match = index as isize;
        needle_index += 1;
    }

    if needle_index < needle.len() {
        return None;
    }
    Some(score - (hay.len() / LENGTH_PENALTY_PER) as i32)
}

/// Whether index `i` begins a word: start of string, after a separator, or the
/// upper-case character of a camelCase hump.
fn starts_word(raw: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let before = raw[i - 1];
    if before == ' ' || before == '-' || before == '/' || before == '_' || before == '.' {
        return true;
    }
    raw[i].is_uppercase() && !raw[i - 1].is_uppercase()
}

fn is_binary(buf: &[u8]) -> bool {
    buf.iter().take(BINARY_PROBE).any(|&b| b == 0)
}

fn is_git_repo(root: &Path) -> bool {
    discover_root(root).is_ok()
}

fn canonicalize_root(root: &Path) -> Result<PathBuf, FsError> {
    fs::canonicalize(root).map_err(FsError::Io)
}

// Mutations name a directory entry, not the target of its final symlink.
fn resolve_entry_inside(root: &Path, relative: &str) -> Result<PathBuf, FsError> {
    let rel = Path::new(relative);
    let name = rel
        .file_name()
        .ok_or_else(|| FsError::EscapesWorkspace(relative.to_string()))?;
    let parent = rel.parent().and_then(Path::to_str).unwrap_or("");
    Ok(resolve_inside(root, parent)?.join(name))
}

fn resolve_inside(root: &Path, relative: &str) -> Result<PathBuf, FsError> {
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err(FsError::EscapesWorkspace(relative.to_string()));
    }
    for c in rel.components() {
        match c {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::EscapesWorkspace(relative.to_string()));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    // Walk one component at a time and canonicalize every existing prefix.
    // A missing nested path under a symlink (`vendor` → `/tmp`, then
    // `vendor/out/file`) would otherwise pass a lexical `starts_with` and
    // `create_dir_all` would write outside the checkout.
    let mut current = root.to_path_buf();
    for c in rel.components() {
        let Component::Normal(name) = c else {
            continue;
        };
        current.push(name);
        if current.symlink_metadata().is_ok() {
            let canon = fs::canonicalize(&current)
                .map_err(|_| FsError::EscapesWorkspace(relative.to_string()))?;
            if !canon.starts_with(root) {
                return Err(FsError::EscapesWorkspace(relative.to_string()));
            }
            current = canon;
        }
    }
    if !current.starts_with(root) {
        return Err(FsError::EscapesWorkspace(relative.to_string()));
    }
    Ok(current)
}

fn normalize_rel(path: &str) -> String {
    path.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn directory_root_lists_real_empty_hidden_and_ignored_children() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success());
        fs::write(root.join(".gitignore"), ".agents/\nbuild/\n*.secret\n").unwrap();
        for path in [
            ".agents",
            "empty",
            "build",
            "build/.agents",
            "node_modules",
            ".venv",
        ] {
            fs::create_dir_all(root.join(path)).unwrap();
        }
        for name in [
            "space name.secret",
            "quote\".secret",
            "line\n.secret",
            "é.secret",
            "back\\slash.secret",
        ] {
            fs::write(root.join(name), "").unwrap();
        }
        let listing = list_directory(root, "").unwrap();
        assert_eq!(listing.path, "");
        assert!(!listing.truncated);
        assert!(!listing
            .entries
            .iter()
            .any(|e| matches!(e.path.as_str(), ".git" | "node_modules" | ".venv")));
        for name in [".agents", "empty", "build"] {
            let entry = listing.entries.iter().find(|e| e.path == name).unwrap();
            assert_eq!(entry.kind, EntryKind::Directory);
            assert_eq!(entry.ignored, name != "empty");
        }
        assert!(listing
            .entries
            .iter()
            .filter(|e| e.path.ends_with(".secret"))
            .all(|e| e.ignored));
        assert!(list_directory(root, ".agents").unwrap().entries.is_empty());
        let nested = list_directory(root, "build/./").unwrap();
        assert_eq!(nested.path, "build");
        assert_eq!(nested.entries[0].path, "build/.agents");
        assert!(nested.entries[0].ignored);
        assert!(list_directory(root, "/").is_err());
        assert!(list_directory(root, "../").is_err());
        assert!(list_directory(root, ".gitignore").is_err());
        assert!(list_directory(root, "missing").is_err());
        assert!(delete_path(root, "").is_err());
        assert!(rename_path(root, "", "oops").is_err());
        assert!(rename_path(root, ".", "oops").is_err());
    }

    #[test]
    fn directory_links_report_targets_and_preserve_aliases() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let outside = tempfile::tempdir().unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success());
        fs::create_dir(root.join("real")).unwrap();
        fs::write(root.join("real/file"), "text").unwrap();
        symlink("real", root.join("alias")).unwrap();
        symlink("real/file", root.join("file-link")).unwrap();
        symlink("missing", root.join("broken")).unwrap();
        symlink(outside.path(), root.join("external")).unwrap();
        symlink("loop", root.join("loop")).unwrap();
        let listing = list_directory(root, "").unwrap();
        for (name, target, kind) in [
            ("alias", SymlinkTarget::Directory, EntryKind::Directory),
            ("file-link", SymlinkTarget::File, EntryKind::File),
            ("broken", SymlinkTarget::Broken, EntryKind::File),
            ("external", SymlinkTarget::External, EntryKind::File),
            ("loop", SymlinkTarget::Unavailable, EntryKind::File),
        ] {
            let entry = listing.entries.iter().find(|e| e.path == name).unwrap();
            assert_eq!(entry.symlink, Some(target));
            assert_eq!(entry.kind, kind);
        }
        let alias = list_directory(root, "alias").unwrap();
        assert_eq!(alias.path, "alias");
        assert_eq!(alias.entries[0].path, "alias/file");
        assert!(matches!(
            list_directory(root, "external"),
            Err(FsError::EscapesWorkspace(_))
        ));
        assert!(matches!(
            read_file(root, "external/file"),
            Err(FsError::EscapesWorkspace(_))
        ));
    }

    #[test]
    fn directory_budgets_mark_partial_before_accumulating_paths() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..=MAX_DIRECTORY_ENTRIES {
            fs::write(tmp.path().join(format!("f{i}")), "").unwrap();
        }
        let listing = list_directory(tmp.path(), "").unwrap();
        assert!(listing.truncated);
        assert_eq!(listing.entries.len(), MAX_DIRECTORY_ENTRIES);
        let large = tempfile::tempdir().unwrap();
        for i in 0..1000 {
            fs::write(large.path().join(format!("{i:04}{}", "x".repeat(240))), "").unwrap();
        }
        let listing = list_directory(large.path(), "").unwrap();
        assert!(listing.truncated);
        assert!(listing.entries.len() < 1000);
        assert!(
            listing
                .entries
                .iter()
                .map(|e| e.path.len() * 6 + 128)
                .sum::<usize>()
                + 128
                <= MAX_DIRECTORY_BYTES
        );
        assert!(matches!(
            list_directory(large.path(), &"x".repeat(MAX_DIRECTORY_PATH_BYTES + 1)),
            Err(FsError::TooLarge { .. })
        ));
    }

    #[test]
    fn directory_permission_failures_are_not_empty_successes() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
        let denied = fs::read_dir(&path).is_err();
        let result = list_directory(tmp.path(), "private");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        // Privileged test runners can bypass mode bits.
        if denied {
            assert!(
                matches!(result, Err(FsError::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied)
            );
        }
    }

    #[test]
    fn directory_listing_honors_configured_excludes_and_keeps_emptied_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let excludes = tempfile::NamedTempFile::new().unwrap();
        fs::write(excludes.path(), ".agents/\n").unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(tmp.path())
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["config", "core.excludesFile"])
            .arg(excludes.path())
            .status()
            .unwrap()
            .success());
        fs::create_dir(tmp.path().join(".agents")).unwrap();
        fs::write(tmp.path().join(".agents/last"), "").unwrap();
        fs::rename(tmp.path().join(".agents/last"), tmp.path().join("moved")).unwrap();
        let listing = list_directory(tmp.path(), "").unwrap();
        let entry = listing
            .entries
            .iter()
            .find(|e| e.path == ".agents")
            .unwrap();
        assert_eq!(entry.kind, EntryKind::Directory);
        assert!(entry.ignored);
        assert!(list_directory(tmp.path(), ".agents")
            .unwrap()
            .entries
            .is_empty());
    }

    #[test]
    fn unrepresentable_directory_names_are_reported_as_partial() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::tempdir().unwrap();
        let name = std::ffi::OsStr::from_bytes(b"bad-\xff");
        // macOS filesystems may refuse invalid UTF-8 at creation time.
        if fs::write(tmp.path().join(name), "").is_err() {
            return;
        }
        let listing = list_directory(tmp.path(), "").unwrap();
        assert!(listing.truncated);
        assert!(listing.entries.is_empty());
    }

    #[test]
    fn entry_mutations_do_not_follow_the_final_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("real"), "keep").unwrap();
        fs::create_dir(root.join("tree")).unwrap();
        fs::write(root.join("tree/child"), "keep child").unwrap();
        for (index, target) in ["real", "tree", "missing", "."].iter().enumerate() {
            let link = format!("alias-{index}");
            symlink(target, root.join(&link)).unwrap();
            rename_path(root, &link, "renamed").unwrap();
            assert_eq!(
                fs::read_link(root.join("renamed")).unwrap(),
                Path::new(target)
            );
            delete_path(root, "renamed").unwrap();
            assert!(root.join("renamed").symlink_metadata().is_err());
        }
        assert_eq!(fs::read(root.join("real")).unwrap(), b"keep");
        assert_eq!(fs::read(root.join("tree/child")).unwrap(), b"keep child");
        assert!(rename_path(root, ".", "moved-root").is_err());
        assert!(delete_path(root, ".").is_err());
        symlink("missing", root.join("occupied")).unwrap();
        assert!(matches!(
            rename_path(root, "real", "occupied"),
            Err(FsError::AlreadyExists(_))
        ));
    }

    #[test]
    fn entry_mutations_reject_an_escaping_parent() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("alias")).unwrap();
        assert!(delete_path(root.path(), "alias/keep").is_err());
        assert!(rename_path(root.path(), "alias/keep", "moved").is_err());
        assert_eq!(fs::read(outside.path().join("keep")).unwrap(), b"keep");
    }

    #[test]
    fn text_reads_refuse_non_regular_entries() {
        let dir = tempfile::tempdir().unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(dir.path().join("socket")).unwrap();
        assert!(matches!(
            read_file(dir.path(), "socket"),
            Err(FsError::NotFound(_))
        ));
        assert!(matches!(
            read_file(dir.path(), "."),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn fuzzy_boundaries_follow_case_expansion() {
        assert_eq!(fuzzy_score("İx", "x"), Some(1));
        assert_eq!(fuzzy_score("İ/x", "x"), Some(1 + WORD_START_BONUS));
        assert_eq!(fuzzy_score("İaX", "x"), Some(1 + WORD_START_BONUS));
        assert_eq!(fuzzy_score("İ.x", "x"), Some(1 + WORD_START_BONUS));
    }

    #[test]
    fn definition_rank_reads_the_shapes_a_declaration_has() {
        // A declaring keyword right before the name, whatever the language.
        assert_eq!(
            definition_rank("pub fn open_file(p: &str) {", "open_file"),
            Some((8, 0))
        );
        assert_eq!(
            definition_rank("export function openFile() {", "openFile"),
            Some((17, 0))
        );
        assert_eq!(
            definition_rank("class OpenFile {", "OpenFile"),
            Some((7, 0))
        );
        assert_eq!(
            definition_rank("impl<T> OpenFile<T> {", "OpenFile"),
            Some((9, 0))
        );
        assert_eq!(
            definition_rank("macro_rules! open_file {", "open_file"),
            Some((14, 0))
        );
        // Go puts the receiver between the keyword and the name.
        assert_eq!(
            definition_rank("func (r *Repo) openFile(p string) {", "openFile"),
            Some((16, 0))
        );
        // A binding is a declaration, but a weaker one: it is often a local.
        assert_eq!(
            definition_rank("const openFile = (p) => {", "openFile"),
            Some((7, 1))
        );
        // A method has no keyword at all — a name, a signature, an open brace.
        assert_eq!(
            definition_rank("  openFile(p: string): void {", "openFile"),
            Some((3, 2))
        );
    }

    /// The half that matters: a mention offered as a definition is worse than
    /// no answer, so every shape that merely *uses* the name has to be refused.
    #[test]
    fn definition_rank_refuses_a_mention() {
        assert_eq!(definition_rank("  openFile(path);", "openFile"), None);
        assert_eq!(
            definition_rank("  const x = openFile(path);", "openFile"),
            None
        );
        assert_eq!(
            definition_rank("import { openFile } from \"./api\";", "openFile"),
            None
        );
        assert_eq!(definition_rank("  if (openFile) {", "openFile"), None);
        // A body on one line is a body, not a signature.
        assert_eq!(
            definition_rank("function outer() { return openFile(1) }", "openFile"),
            None
        );
        // `-w` is git's word boundary; this is ours, for the walk fallback.
        assert_eq!(definition_rank("fn reopenFile() {", "openFile"), None);
        assert_eq!(definition_rank("fn openFileTwice() {", "openFile"), None);
    }

    #[test]
    fn declarations_rank_keywords_over_bare_heads_and_keep_grep_order() {
        let raw = SearchResults {
            matches: vec![
                hit("a.ts", 1, "  openFile(p: string) {"),
                hit("b.ts", 2, "  openFile(p);"),
                hit("c.rs", 3, "pub fn openFile() {"),
                hit("d.ts", 4, "const openFile = () => {"),
            ],
            truncated: false,
        };
        let kept = declarations(raw, "openFile", 10);
        assert_eq!(
            kept.matches
                .iter()
                .map(|m| m.path.as_str())
                .collect::<Vec<_>>(),
            ["c.rs", "d.ts", "a.ts"]
        );
        assert!(!kept.truncated);
    }

    #[test]
    fn declarations_report_truncation_of_the_kept_hits_not_the_raw_ones() {
        let raw = SearchResults {
            matches: vec![
                hit("a.rs", 1, "fn openFile() {"),
                hit("b.rs", 2, "fn openFile() {"),
            ],
            truncated: false,
        };
        assert!(declarations(raw, "openFile", 1).truncated);
    }

    /// The gate that keeps the grep a fixed-string search.
    #[test]
    fn only_an_identifier_reaches_git() {
        assert!(is_identifier("openFile"));
        assert!(is_identifier("_private"));
        assert!(is_identifier("$el"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("open file"));
        assert!(!is_identifier(".*"));
        assert!(!is_identifier("2fast"));
    }

    #[test]
    fn definition_rank_accepts_mutable_bindings_and_generic_methods() {
        for (line, symbol) in [
            ("let mut cache = Cache::new();", "cache"),
            ("static mut CACHE: usize = 0;", "CACHE"),
            ("  static find<T>(key: T): T {", "find"),
            ("  find<T extends Map<string, V>>(key: T) {", "find"),
            ("  find<T extends () => void>(key: T) {", "find"),
        ] {
            assert!(definition_rank(line, symbol).is_some(), "{line}");
        }
        for line in [
            "find<T>(key);",
            "return find<T>(key);",
            "const result = find<T>(key);",
            "object.find<T>(key);",
            "find<T(key) {",
        ] {
            assert_eq!(definition_rank(line, "find"), None, "{line}");
        }
    }

    #[test]
    fn grep_records_preserve_paths_lines_and_limits() {
        let raw = b"first.ts\x002\0lookup();\nsecond\nfile.ts\x009\0lookup<T>(): T;\n";
        let all = parse_grep_z(raw, None, 10);
        assert_eq!(all.matches.len(), 2);
        assert_eq!(all.matches[0].text, "lookup();");
        assert_eq!(all.matches[1].path, "second\nfile.ts");
        assert_eq!(all.matches[1].line, 9);
        assert_eq!(all.matches[1].text, "lookup<T>(): T;");
        assert!(!all.truncated);
        let limited = parse_grep_z(raw, None, 1);
        assert_eq!(limited.matches.len(), 1);
        assert!(limited.truncated);
    }

    #[test]
    fn grep_context_lines_attach_to_the_hits_they_surround() {
        // Hits on 3 and 5 share line 4; `--` opens a second group at 20.
        let raw = b"a.ts\x001\0one\na.ts\x002\0two\na.ts\x003\0hit three\na.ts\x004\0four\n\
a.ts\x005\0hit five\na.ts\x006\0six\na.ts\x007\0seven\n--\n\
a.ts\x0019\0nineteen\na.ts\x0020\0hit twenty\nb.ts\x001\0hit b\nb.ts\x002\0b two\n";
        let all = parse_grep_z(raw, Some("hit"), 10);
        let shape: Vec<_> = all
            .matches
            .iter()
            .map(|m| (m.path.as_str(), m.line, m.before.clone(), m.after.clone()))
            .collect();
        assert_eq!(
            shape,
            [
                (
                    "a.ts",
                    3,
                    vec!["one".into(), "two".into()],
                    vec!["four".into(), "hit five".into(), "six".into()]
                ),
                (
                    "a.ts",
                    5,
                    vec!["two".into(), "hit three".into(), "four".into()],
                    vec!["six".into(), "seven".into()]
                ),
                ("a.ts", 20, vec!["nineteen".into()], vec![]),
                ("b.ts", 1, vec![], vec!["b two".into()]),
            ]
        );
        assert!(!all.truncated);

        // The cap still reads the last kept hit's trailing context.
        let capped = parse_grep_z(raw, Some("hit"), 2);
        assert_eq!(capped.matches.len(), 2);
        assert_eq!(capped.matches[1].after, ["six", "seven"]);
        assert!(capped.truncated);
    }

    #[test]
    fn context_arg_matches_the_context_constant() {
        assert_eq!(CONTEXT_ARG.parse::<usize>().unwrap(), SEARCH_CONTEXT_LINES);
    }

    #[test]
    fn context_lines_are_clipped_on_a_char_boundary() {
        let long = "é".repeat(MAX_CONTEXT_LINE_BYTES);
        let clipped = context_line(&long);
        assert!(clipped.len() <= MAX_CONTEXT_LINE_BYTES);
        assert!(clipped.chars().all(|c| c == 'é'));
    }

    #[test]
    fn typescript_declaration_matrix() {
        for line in [
            "export async function lookup<T>(value: T): Promise<T> {",
            "export function* lookup() { yield 1; }",
            "export const lookup = <T>(value: T) => value;",
            "export type lookup<T> = { value: T };",
            "export interface lookup<T> { value: T; }",
            "  lookup(value: string): void;",
            "  lookup?(value: string): void;",
            "  abstract lookup<T>(value: T): T;",
            "  lookup(value: string) { return value; }",
            "  public async lookup<T>(value: T): Promise<T> { return value; }",
            "  get lookup(): string { return this.value; }",
        ] {
            assert!(definition_rank(line, "lookup").is_some(), "{line}");
        }
        for line in [
            "lookup(value);",
            "await lookup(value);",
            "return lookup(value);",
            "object.lookup(value);",
            "const value = lookup(value);",
            "import { lookup } from './service';",
            "export { lookup };",
            "const value = condition ? lookup(value) : other;",
        ] {
            assert_eq!(definition_rank(line, "lookup"), None, "{line}");
        }
    }

    #[test]
    fn typescript_definition_search_uses_real_git() {
        let tmp = git_repo();
        fs::write(
            tmp.path().join("service.ts"),
            "export interface Service {\n  lookup<T>(value: T): T;\n}\nexport class Client {\n  lookup<T>(value: T): T { return value; }\n}\nlookup(value);\nobject.lookup(value);\n",
        )
        .unwrap();
        let results = search_files(tmp.path(), "lookup", SearchKind::Definition, 50).unwrap();
        assert_eq!(
            results
                .matches
                .iter()
                .map(|hit| hit.line)
                .collect::<Vec<_>>(),
            [2, 5]
        );
        assert!(!results.truncated);
    }

    fn hit(path: &str, line: u32, text: &str) -> SearchMatch {
        SearchMatch {
            path: path.to_string(),
            line,
            column: 0,
            text: text.to_string(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    fn git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());
        tmp
    }

    #[test]
    fn lists_tracked_and_untracked() {
        let tmp = git_repo();
        fs::write(tmp.path().join("a.rs"), "fn main() {}").unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/lib.rs"), "").unwrap();
        assert!(Command::new("git")
            .args(["add", "a.rs"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());

        let tree = list_files(tmp.path()).unwrap();
        let paths: Vec<_> = tree.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"a.rs"));
        assert!(paths.contains(&"src/lib.rs"));
        assert!(tree
            .entries
            .iter()
            .all(|entry| entry.kind == EntryKind::File));
    }

    #[test]
    fn drops_paths_deleted_from_the_worktree() {
        let tmp = git_repo();
        fs::write(tmp.path().join("plan.md"), "notes").unwrap();
        fs::create_dir_all(tmp.path().join("docs/orca")).unwrap();
        fs::write(tmp.path().join("docs/orca/worktrees.md"), "notes").unwrap();
        fs::write(tmp.path().join("keep.rs"), "fn main() {}").unwrap();
        assert!(Command::new("git")
            .args(["add", "-A"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());

        // Deleted on disk, still in the index — which is what `delete_path`,
        // a `rm`, and an agent all leave behind.
        delete_path(tmp.path(), "plan.md").unwrap();
        delete_path(tmp.path(), "docs").unwrap();

        let tree = list_files(tmp.path()).unwrap();
        let paths: Vec<_> = tree.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["keep.rs"]);
    }

    #[test]
    fn lists_an_ignored_file_but_flags_it() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".gitignore"), "secret.txt\ndist/\n").unwrap();
        fs::write(tmp.path().join("secret.txt"), "nope").unwrap();
        fs::create_dir(tmp.path().join("dist")).unwrap();
        fs::write(tmp.path().join("dist/app.js"), "built").unwrap();
        fs::write(tmp.path().join("ok.txt"), "yes").unwrap();

        let tree = list_files(tmp.path()).unwrap();
        let flagged: Vec<_> = tree
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.ignored))
            .collect();

        // The tracked group comes first, so the budget is spent on the work
        // before a build directory gets any of it — and `dist` arrives as
        // itself rather than as its contents.
        assert_eq!(
            flagged,
            vec![
                (".gitignore", false),
                ("ok.txt", false),
                ("dist", true),
                ("secret.txt", true),
            ]
        );
        assert_eq!(
            tree.entries
                .iter()
                .find(|e| e.path == "dist")
                .map(|e| e.kind),
            Some(EntryKind::Directory)
        );
    }

    /// An ignored directory costs one row however much is under it.
    ///
    /// Without `--directory` this repository's own listing answers with
    /// 385 751 ignored paths against a budget of 10 000, so the tree truncates
    /// on build output before it reaches any source. The collapse is what
    /// makes listing ignored paths affordable at all.
    #[test]
    fn an_ignored_directory_never_lists_its_contents() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".gitignore"), "dist/\n").unwrap();
        fs::create_dir_all(tmp.path().join("dist/assets/deep")).unwrap();
        for name in ["a.js", "b.js", "c.js"] {
            fs::write(tmp.path().join("dist/assets/deep").join(name), "").unwrap();
        }
        fs::write(tmp.path().join("app.js"), "source").unwrap();

        let tree = list_files(tmp.path()).unwrap();
        let dist: Vec<_> = tree
            .entries
            .iter()
            .filter(|e| e.path.starts_with("dist"))
            .collect();
        assert_eq!(dist.len(), 1, "got {dist:?}");
        assert_eq!(dist[0].path, "dist");
        assert_eq!(dist[0].kind, EntryKind::Directory);
        assert!(dist[0].ignored);
        assert!(!tree.truncated);
    }

    /// Language dependency directories are omitted from the root listing entirely.
    #[test]
    fn dependency_directories_are_not_listed() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".gitignore"), "node_modules/\nvendor/\n").unwrap();
        fs::create_dir_all(tmp.path().join("node_modules/left-pad")).unwrap();
        fs::write(tmp.path().join("node_modules/left-pad/index.js"), "").unwrap();
        fs::create_dir_all(tmp.path().join("vendor/pkg")).unwrap();
        fs::write(tmp.path().join("vendor/pkg/lib.go"), "").unwrap();
        fs::write(tmp.path().join("app.js"), "source").unwrap();

        let tree = list_files(tmp.path()).unwrap();
        assert!(
            tree.entries
                .iter()
                .all(|e| !e.path.starts_with("node_modules") && !e.path.starts_with("vendor")),
            "got {tree:?}"
        );
    }

    #[test]
    fn list_directory_peels_one_ignored_folder() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".gitignore"), "plan/\n").unwrap();
        fs::create_dir_all(tmp.path().join("plan/nested")).unwrap();
        fs::write(tmp.path().join("plan/notes.md"), "x").unwrap();
        fs::write(tmp.path().join("plan/nested/more.md"), "y").unwrap();

        let peeled = list_directory(tmp.path(), "plan").unwrap();
        let paths: Vec<_> = peeled
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.kind, e.ignored))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("plan/nested", EntryKind::Directory, true),
                ("plan/notes.md", EntryKind::File, true),
            ]
        );
        assert!(!peeled.truncated);

        let nested = list_directory(tmp.path(), "plan/nested").unwrap();
        assert_eq!(nested.entries.len(), 1);
        assert_eq!(nested.entries[0].path, "plan/nested/more.md");
        assert!(nested.entries[0].ignored);
    }

    #[test]
    fn list_directory_skips_dependency_children() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".gitignore"), "build/\n").unwrap();
        fs::create_dir_all(tmp.path().join("build/node_modules/x")).unwrap();
        fs::write(tmp.path().join("build/out.js"), "").unwrap();

        let peeled = list_directory(tmp.path(), "build").unwrap();
        assert_eq!(
            peeled
                .entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec!["build/out.js"]
        );
    }

    #[test]
    fn a_name_search_does_not_offer_ignored_files() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".gitignore"), "build/\n").unwrap();
        fs::create_dir(tmp.path().join("build")).unwrap();
        fs::write(tmp.path().join("build/widget.rs"), "built").unwrap();
        fs::write(tmp.path().join("widget.rs"), "source").unwrap();

        let hits = search_files(tmp.path(), "widget", SearchKind::Name, 10).unwrap();
        let paths: Vec<_> = hits.matches.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, vec!["widget.rs"]);
    }

    #[test]
    fn read_write_round_trip_and_revision() {
        let tmp = git_repo();
        fs::write(tmp.path().join("f.rs"), "one").unwrap();
        let first = read_file(tmp.path(), "f.rs").unwrap();
        assert_eq!(first.text, "one");
        assert_eq!(first.language, "rust");
        write_file(tmp.path(), "f.rs", "two", &first.revision).unwrap();
        let second = read_file(tmp.path(), "f.rs").unwrap();
        assert_eq!(second.text, "two");
        assert_ne!(first.revision, second.revision);
    }

    #[test]
    fn recognizes_python_source_extensions() {
        for path in ["src/main.py", "types/models.pyi", "desktop/app.pyw"] {
            assert_eq!(language_for(path), "python", "{path}");
        }
    }

    #[test]
    fn write_rejects_stale_revision() {
        let tmp = git_repo();
        fs::write(tmp.path().join("f.rs"), "one").unwrap();
        let first = read_file(tmp.path(), "f.rs").unwrap();
        fs::write(tmp.path().join("f.rs"), "agent").unwrap();
        let err = write_file(tmp.path(), "f.rs", "mine", &first.revision).unwrap_err();
        match err {
            FsError::RevisionMismatch { current } => assert_eq!(current.text, "agent"),
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn rejects_path_escape() {
        let tmp = git_repo();
        let err = read_file(tmp.path(), "../outside").unwrap_err();
        assert!(matches!(err, FsError::EscapesWorkspace(_)));
    }

    #[test]
    fn create_refuses_a_symlink_prefix_that_leaves_the_workspace() {
        let tmp = git_repo();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("vendor")).unwrap();
        let err = create_path(tmp.path(), "vendor/out/file.rs", PathKind::File).unwrap_err();
        assert!(matches!(err, FsError::EscapesWorkspace(_)));
        assert!(!outside.path().join("out").exists());
    }

    #[test]
    fn rejects_binary() {
        let tmp = git_repo();
        fs::write(tmp.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        let c = read_file(tmp.path(), "b.bin").unwrap();
        assert!(c.binary);
        assert!(c.text.is_empty());
    }

    #[test]
    fn a_lossy_decode_is_reported_as_binary_instead_of_repaired() {
        let tmp = git_repo();
        // Latin-1 "café": no NUL, so the probe passes, but it is not UTF-8.
        fs::write(tmp.path().join("latin.txt"), [b'c', b'a', b'f', 0xe9]).unwrap();
        let contents = read_file(tmp.path(), "latin.txt").unwrap();
        assert!(contents.binary, "a lossy decode must not look like text");
        assert!(contents.text.is_empty());

        // And the write path refuses it, so the bytes cannot be replaced by
        // their replacement characters.
        let err = write_file(tmp.path(), "latin.txt", "cafe", &contents.revision).unwrap_err();
        assert!(matches!(err, FsError::RevisionMismatch { .. }));
        assert_eq!(
            fs::read(tmp.path().join("latin.txt")).unwrap(),
            [b'c', b'a', b'f', 0xe9]
        );
    }

    #[test]
    fn a_file_that_grows_past_the_budget_after_metadata_is_reported_too_large() {
        let tmp = git_repo();
        let path = tmp.path().join("big.txt");
        fs::write(&path, vec![b'a'; MAX_FILE_BYTES + 1]).unwrap();
        let contents = read_file(tmp.path(), "big.txt").unwrap();
        assert!(contents.too_large);
        assert!(contents.text.is_empty());
    }

    #[test]
    fn two_names_sharing_a_stem_do_not_share_a_temp_file() {
        let tmp = git_repo();
        assert_ne!(
            temp_path(&tmp.path().join("a.rs")).unwrap(),
            temp_path(&tmp.path().join("a.md")).unwrap()
        );
    }

    #[test]
    fn writing_leaves_no_temp_file_behind() {
        let tmp = git_repo();
        fs::write(tmp.path().join("note.txt"), "old").unwrap();
        let contents = read_file(tmp.path(), "note.txt").unwrap();
        write_file(tmp.path(), "note.txt", "new", &contents.revision).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("note.txt")).unwrap(),
            "new"
        );
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("forge-tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn reads_an_image_whole_with_its_media_type() {
        let tmp = git_repo();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let png = [0x89, b'P', b'N', b'G', 0, 0, 0, 0];
        fs::write(tmp.path().join("docs/shot.PNG"), png).unwrap();
        let image = read_image(tmp.path(), "docs/shot.PNG").unwrap();
        assert_eq!(image.mime, "image/png");
        assert_eq!(image.path, "docs/shot.PNG");
        assert_eq!(image.bytes, png);
    }

    #[test]
    fn image_read_refuses_what_is_not_named_as_an_image() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".env"), "SECRET=1").unwrap();
        // Refused on the name alone, before the path is touched.
        assert!(matches!(
            read_image(tmp.path(), ".env"),
            Err(FsError::NotAnImage(_))
        ));
        assert!(matches!(
            read_image(tmp.path(), "../outside.png"),
            Err(FsError::EscapesWorkspace(_))
        ));
        assert!(matches!(
            read_image(tmp.path(), "missing.png"),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn image_read_refuses_an_oversize_file_instead_of_cutting_it() {
        let tmp = git_repo();
        let file = File::create(tmp.path().join("huge.jpg")).unwrap();
        file.set_len(MAX_IMAGE_BYTES as u64 + 1).unwrap();
        assert!(matches!(
            read_image(tmp.path(), "huge.jpg"),
            Err(FsError::TooLarge { .. })
        ));
    }

    #[test]
    fn name_search_is_fuzzy() {
        let tmp = git_repo();
        fs::create_dir_all(tmp.path().join("apps/tauri/src")).unwrap();
        fs::write(tmp.path().join("apps/tauri/src/app_shell.rs"), "").unwrap();
        let r = search_files(tmp.path(), "appshell", SearchKind::Name, 50).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert!(r.matches[0].path.ends_with("app_shell.rs"));
    }

    #[test]
    fn content_search_keeps_three_context_lines_in_git_and_plain_directories() {
        for tmp in [git_repo(), tempfile::tempdir().unwrap()] {
            fs::write(
                tmp.path().join("context.txt"),
                "outside before\none\ntwo\nthree\nneedle\nfive\nsix\nseven\noutside after\n",
            )
            .unwrap();
            let results = search_files(tmp.path(), "needle", SearchKind::Content, 50).unwrap();
            assert!(!results.truncated);
            assert_eq!(results.matches.len(), 1);
            let found = &results.matches[0];
            assert_eq!(found.path, "context.txt");
            assert_eq!(found.line, 5);
            assert_eq!(found.before, ["one", "two", "three"]);
            assert_eq!(found.after, ["five", "six", "seven"]);
        }
    }

    #[test]
    fn content_search_finds_line() {
        let tmp = git_repo();
        fs::write(tmp.path().join("a.rs"), "hello\nworld\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "a.rs"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());
        let r = search_files(tmp.path(), "world", SearchKind::Content, 50).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].line, 2);
        assert!(r.matches[0].text.contains("world"));
        assert_eq!(r.matches[0].before, ["hello"]);
        assert!(r.matches[0].after.is_empty());
    }

    /// Metacharacters stay literal: Content is find-in-project text, not a regex.
    #[test]
    fn content_search_is_fixed_string() {
        let tmp = git_repo();
        fs::write(tmp.path().join("a.rs"), "hello.*world\nother line\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "a.rs"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());
        let r = search_files(tmp.path(), ".*", SearchKind::Content, 50).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].line, 1);
        assert!(r.matches[0].text.contains("hello.*world"));
    }

    /// The whole path, against a real `git grep`: the word boundary is git's,
    /// the shape filter is ours, and the call that merely uses the name is not
    /// in the answer.
    #[test]
    fn definition_search_finds_the_declaration_and_not_the_call() {
        let tmp = git_repo();
        fs::write(
            tmp.path().join("lib.rs"),
            "pub fn open_file(p: &str) {}\nfn main() { open_file(\"a\"); }\n",
        )
        .unwrap();
        fs::write(tmp.path().join("other.rs"), "// reopen_file is not it\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success());
        let r = search_files(tmp.path(), "open_file", SearchKind::Definition, 50).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].path, "lib.rs");
        assert_eq!(r.matches[0].line, 1);
        assert_eq!(r.matches[0].column, 8);
    }

    #[test]
    fn definition_search_never_hands_git_a_pattern() {
        let tmp = git_repo();
        fs::write(tmp.path().join("a.rs"), "pub fn anything() {}\n").unwrap();
        let r = search_files(tmp.path(), ".*", SearchKind::Definition, 50).unwrap();
        assert!(r.matches.is_empty());
    }

    #[test]
    fn creates_a_file_and_the_directories_above_it() {
        let tmp = git_repo();
        create_path(tmp.path(), "a/b/c.rs", PathKind::File).unwrap();
        assert!(tmp.path().join("a/b/c.rs").is_file());
    }

    #[test]
    fn creates_a_directory() {
        let tmp = git_repo();
        create_path(tmp.path(), "a/b", PathKind::Directory).unwrap();
        assert!(tmp.path().join("a/b").is_dir());
    }

    #[test]
    fn create_refuses_to_overwrite() {
        // "Create" and "replace what is there" are different acts, and only one
        // of them was asked for. There is no revision to condition on here, so
        // refusing is the only thing that cannot lose an agent's work.
        let tmp = git_repo();
        fs::write(tmp.path().join("a.rs"), "keep me").unwrap();
        let err = create_path(tmp.path(), "a.rs", PathKind::File).unwrap_err();
        assert!(matches!(err, FsError::AlreadyExists(_)));
        assert_eq!(
            fs::read_to_string(tmp.path().join("a.rs")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn create_refuses_to_escape_the_workspace() {
        let tmp = git_repo();
        let err = create_path(tmp.path(), "../outside.rs", PathKind::File).unwrap_err();
        assert!(matches!(err, FsError::EscapesWorkspace(_)));
    }

    #[test]
    fn renames_a_file_into_a_directory_that_does_not_exist_yet() {
        let tmp = git_repo();
        fs::write(tmp.path().join("a.rs"), "x").unwrap();
        rename_path(tmp.path(), "a.rs", "src/b.rs").unwrap();
        assert!(!tmp.path().join("a.rs").exists());
        assert_eq!(
            fs::read_to_string(tmp.path().join("src/b.rs")).unwrap(),
            "x"
        );
    }

    #[test]
    fn rename_refuses_an_occupied_destination() {
        let tmp = git_repo();
        fs::write(tmp.path().join("a.rs"), "a").unwrap();
        fs::write(tmp.path().join("b.rs"), "b").unwrap();
        let err = rename_path(tmp.path(), "a.rs", "b.rs").unwrap_err();
        assert!(matches!(err, FsError::AlreadyExists(_)));
        assert_eq!(fs::read_to_string(tmp.path().join("b.rs")).unwrap(), "b");
    }

    #[test]
    fn rename_destination_created_after_preparation_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        for symlink in [false, true] {
            fs::write(tmp.path().join("source"), "source").unwrap();
            let movement = prepare_rename(tmp.path(), "source", "target").unwrap();
            if symlink {
                std::os::unix::fs::symlink("missing", tmp.path().join("target")).unwrap();
            } else {
                fs::write(tmp.path().join("target"), "competitor").unwrap();
            }
            assert!(matches!(movement.apply(), Err(FsError::AlreadyExists(_))));
            assert_eq!(
                fs::read_to_string(tmp.path().join("source")).unwrap(),
                "source"
            );
            if symlink {
                assert_eq!(
                    fs::read_link(tmp.path().join("target")).unwrap(),
                    Path::new("missing")
                );
            } else {
                assert_eq!(
                    fs::read_to_string(tmp.path().join("target")).unwrap(),
                    "competitor"
                );
            }
            fs::remove_file(tmp.path().join("target")).unwrap();
        }
    }

    #[test]
    fn prepared_moves_refuse_replaced_parent_symlinks() {
        for swap_source in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::create_dir(tmp.path().join("src")).unwrap();
            fs::create_dir(tmp.path().join("dst")).unwrap();
            fs::write(tmp.path().join("src/file"), "inside").unwrap();
            fs::write(outside.path().join("file"), "outside").unwrap();
            let movement = prepare_rename(tmp.path(), "src/file", "dst/file").unwrap();
            let parent = if swap_source { "src" } else { "dst" };
            fs::rename(tmp.path().join(parent), tmp.path().join("original")).unwrap();
            std::os::unix::fs::symlink(outside.path(), tmp.path().join(parent)).unwrap();
            assert!(movement.apply().is_err());
            assert_eq!(
                fs::read_to_string(outside.path().join("file")).unwrap(),
                "outside"
            );
            let source = if swap_source {
                "original/file"
            } else {
                "src/file"
            };
            assert_eq!(
                fs::read_to_string(tmp.path().join(source)).unwrap(),
                "inside"
            );
        }
    }

    #[test]
    fn unrelated_broken_editor_alias_cannot_veto_a_move() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("other"), "keep").unwrap();
        std::os::unix::fs::symlink("missing", tmp.path().join("alias")).unwrap();
        let movement = prepare_rename(tmp.path(), "other", "renamed").unwrap();
        assert_eq!(movement.retarget("alias/file").unwrap(), None);
        movement.apply().unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("renamed")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn literal_unix_names_round_trip_through_directory_index_read_and_write() {
        let tmp = git_repo();
        fs::create_dir(tmp.path().join("nested")).unwrap();
        fs::create_dir(tmp.path().join("back")).unwrap();
        fs::write(tmp.path().join("back/slash.secret"), "other").unwrap();
        for parent in ["", "nested"] {
            for name in [r"back\slash.secret", ":(glob)*", ":(literal)name"] {
                let path = if parent.is_empty() {
                    name.to_owned()
                } else {
                    format!("{parent}/{name}")
                };
                fs::write(tmp.path().join(&path), "original").unwrap();
                let listing = list_directory(tmp.path(), parent).unwrap();
                assert!(listing.entries.iter().any(|entry| entry.path == path));
                assert!(list_files(tmp.path())
                    .unwrap()
                    .entries
                    .iter()
                    .any(|entry| entry.path == path));
                let contents = read_file(tmp.path(), &path).unwrap();
                assert_eq!(contents.path, path);
                write_file(tmp.path(), &contents.path, "changed", &contents.revision).unwrap();
                assert_eq!(
                    fs::read_to_string(tmp.path().join(&path)).unwrap(),
                    "changed"
                );
            }
        }
        assert_eq!(
            fs::read_to_string(tmp.path().join("back/slash.secret")).unwrap(),
            "other"
        );
    }

    #[test]
    fn navigation_index_preserves_directory_link_types() {
        for git in [false, true] {
            let tmp = if git {
                git_repo()
            } else {
                tempfile::tempdir().unwrap()
            };
            fs::create_dir(tmp.path().join("real")).unwrap();
            fs::write(tmp.path().join("real/file"), "text").unwrap();
            std::os::unix::fs::symlink("real", tmp.path().join("alias")).unwrap();
            let index = list_files(tmp.path()).unwrap();
            let entry = index
                .entries
                .iter()
                .find(|entry| entry.path == "alias")
                .unwrap();
            assert_eq!(entry.kind, EntryKind::Directory);
            assert_eq!(entry.symlink, Some(SymlinkTarget::Directory));
        }
    }

    #[test]
    fn name_search_carries_source_index_incompleteness_even_without_hits() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..8000 {
            fs::write(tmp.path().join(format!("file-{i:05}")), "").unwrap();
        }
        let result = search_files(tmp.path(), "missing", SearchKind::Name, 20).unwrap();
        assert!(result.matches.is_empty());
        assert!(result.truncated);
    }

    #[test]
    fn rename_case_only_and_invalid_moves() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("Folder")).unwrap();
        fs::write(tmp.path().join("Folder/file"), "keep").unwrap();
        for (from, to) in [
            ("", "x"),
            (".", "x"),
            ("Folder", "."),
            ("Folder", "Folder"),
            ("Folder", "Folder/new/child"),
        ] {
            assert!(rename_path(tmp.path(), from, to).is_err(), "{from} -> {to}");
        }
        assert!(!tmp.path().join("Folder/new").exists());
        if tmp.path().join("folder").exists() {
            assert!(rename_path(tmp.path(), "folder", "folder/new/child").is_err());
            assert!(!tmp.path().join("Folder/new").exists());
        }
        rename_path(tmp.path(), "Folder", "folder").unwrap();
        let names: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("folder")]);
        rename_path(tmp.path(), "folder/file", "folder/FILE").unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("folder/FILE")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn rename_external_symlink_moves_only_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        rename_path(tmp.path(), "link", "moved").unwrap();
        assert_eq!(
            fs::read_link(tmp.path().join("moved")).unwrap(),
            outside.path()
        );
        assert_eq!(fs::read_to_string(outside.path()).unwrap(), "outside");
    }

    #[test]
    fn rename_retargets_by_components_and_does_not_overwrite_a_hardlink() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::create_dir(tmp.path().join("src-other")).unwrap();
        fs::write(tmp.path().join("src/file"), "keep").unwrap();
        std::os::unix::fs::symlink("src", tmp.path().join("alias")).unwrap();
        let movement = prepare_rename(tmp.path(), "src", "moved").unwrap();
        assert_eq!(
            movement.retarget("src/file").unwrap().as_deref(),
            Some("moved/file")
        );
        assert_eq!(
            movement.retarget("alias/file").unwrap().as_deref(),
            Some("moved/file")
        );
        assert_eq!(movement.retarget("src-other/file").unwrap(), None);
        fs::hard_link(tmp.path().join("src/file"), tmp.path().join("other")).unwrap();
        assert!(matches!(
            rename_path(tmp.path(), "src/file", "other"),
            Err(FsError::AlreadyExists(_))
        ));
        assert_eq!(
            fs::read_to_string(tmp.path().join("src/file")).unwrap(),
            "keep"
        );
        assert!(matches!(
            prepare_rename(tmp.path(), "src", &"x".repeat(4097)),
            Err(FsError::TooLarge { .. })
        ));
    }

    #[test]
    fn rename_refuses_a_source_that_is_not_there() {
        let tmp = git_repo();
        let err = rename_path(tmp.path(), "gone.rs", "b.rs").unwrap_err();
        assert!(matches!(err, FsError::NotFound(_)));
    }

    #[test]
    fn deletes_a_file_and_a_directory() {
        let tmp = git_repo();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/a.rs"), "").unwrap();
        delete_path(tmp.path(), "src/a.rs").unwrap();
        assert!(!tmp.path().join("src/a.rs").exists());
        delete_path(tmp.path(), "src").unwrap();
        assert!(!tmp.path().join("src").exists());
    }

    #[test]
    fn delete_refuses_the_workspace_root_itself() {
        // The root is not a path *inside* the workspace, and `resolve_inside`
        // would happily hand it back for an empty relative path.
        let tmp = git_repo();
        for relative in ["", "/", "."] {
            assert!(delete_path(tmp.path(), relative).is_err(), "{relative:?}");
        }
        assert!(tmp.path().exists());
    }

    #[test]
    fn delete_refuses_a_path_that_is_not_there() {
        let tmp = git_repo();
        let err = delete_path(tmp.path(), "gone.rs").unwrap_err();
        assert!(matches!(err, FsError::NotFound(_)));
    }

    #[test]
    fn fuzzy_subsequence() {
        // A match is still a subsequence test; the score only orders the ones
        // that matched.
        assert!(fuzzy_score("src/app_shell.rs", "appshell").is_some());
        assert!(fuzzy_score("FooBar", "fb").is_some());
        assert!(fuzzy_score("abc", "acx").is_none());
    }

    #[test]
    fn fuzzy_score_prefers_word_starts_over_incidental_letters() {
        // "fb" landing on two word starts must beat the same two letters
        // buried mid-word, whatever order the walk produced them in.
        let hump = fuzzy_score("FooBar", "fb").expect("camelCase hump matches");
        let buried = fuzzy_score("offbeat", "fb").expect("buried letters match");
        assert!(hump > buried, "{hump} should beat {buried}");
    }

    #[test]
    fn fuzzy_score_prefers_the_shorter_path_on_an_equal_match() {
        let short = fuzzy_score("src/a.rs", "ars").expect("matches");
        let long = fuzzy_score("src/a.rs/very/long/tail/indeed/here", "ars").expect("matches");
        assert!(short > long, "{short} should beat {long}");
    }

    #[test]
    fn fuzzy_score_rewards_a_run_of_consecutive_characters() {
        let run = fuzzy_score("zshell", "she").expect("matches");
        let scattered = fuzzy_score("zsxhxe", "she").expect("matches");
        assert!(run > scattered, "{run} should beat {scattered}");
    }

    #[test]
    fn fuzzy_score_has_nothing_to_say_about_an_empty_query() {
        assert_eq!(fuzzy_score("anything", ""), Some(0));
    }

    #[test]
    fn name_search_returns_the_best_hit_even_past_the_limit() {
        // The defect this replaced: hits were kept in walk order and cut at
        // `limit`, so the best answer could be the one dropped.
        let tmp = git_repo();
        let root = tmp.path();
        for index in 0..20 {
            fs::write(root.join(format!("zzz_{index}_target.rs")), "").unwrap();
        }
        fs::write(root.join("target.rs"), "").unwrap();

        let found = search_files(root, "target", SearchKind::Name, 1).expect("search");
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].path, "target.rs");
        assert!(
            found.truncated,
            "twenty-one hits capped at one is truncated"
        );
    }
}
