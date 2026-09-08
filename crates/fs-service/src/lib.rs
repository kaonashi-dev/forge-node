//! # fs-service
//!
//! Workspace filesystem operations for Forge (ADR-012): list, read, write and
//! search, with every path kept inside the checkout. The daemon is the only
//! caller; the GUI never opens a file itself.
//!
//! Listing and content search prefer the system `git` CLI (ADR-008) so
//! `.gitignore` comes for free. A non-repo checkout falls back to a bounded
//! directory walk. Writes are atomic (`temp` + `rename`) and conditioned on a
//! content revision so an agent editing the same path cannot be overwritten in
//! silence.

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use git_service::{discover_root, run_git, GitError};
use thiserror::Error;

/// Soft ceiling for one file's contents on the wire.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Soft ceiling for how many paths a tree listing returns.
pub const MAX_TREE_ENTRIES: usize = 10_000;

/// Soft ceiling for search hits.
pub const MAX_SEARCH_RESULTS: usize = 200;

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
    /// True when a `NUL` was found in the probe window.
    pub binary: bool,
    /// True when the file exceeded [`MAX_FILE_BYTES`] and was not opened.
    pub too_large: bool,
}

/// Name match or content match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchKind {
    /// Subsequence match against the relative path.
    Name,
    /// Line match against file contents (`git grep`).
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
/// not inside a git repository.
pub fn list_files(root: &Path) -> Result<FileTree, FsError> {
    let root = canonicalize_root(root)?;
    if is_git_repo(&root) {
        list_via_git(&root)
    } else {
        list_via_walk(&root)
    }
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
    let mut file = File::open(&path)?;
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut buf)?;
    if is_binary(&buf) {
        return Ok(FileContents {
            path: normalize_rel(relative),
            text: String::new(),
            revision: revision_of(&buf),
            language: language_for(relative).to_string(),
            binary: true,
            too_large: false,
        });
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    Ok(FileContents {
        path: normalize_rel(relative),
        revision: revision_of(&buf),
        language: language_for(relative).to_string(),
        text,
        binary: false,
        too_large: false,
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

    let tmp = path.with_extension(format!("forge-tmp-{}", std::process::id()));
    {
        let mut out = File::create(&tmp)?;
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
    }
    if let Some(perms) = mode {
        let _ = fs::set_permissions(&tmp, perms);
    }
    fs::rename(&tmp, &path)?;
    Ok(())
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

/// Move `from` to `to`, both workspace-relative (A11).
///
/// Refuses an existing destination for the same reason `create_path` does. The
/// rename is not atomic across filesystems and is not asked to be: both paths
/// are inside one checkout.
pub fn rename_path(root: &Path, from: &str, to: &str) -> Result<(), FsError> {
    let root = canonicalize_root(root)?;
    let source = resolve_inside(&root, from)?;
    let target = resolve_inside(&root, to)?;
    if !source.exists() {
        return Err(FsError::NotFound(normalize_rel(from)));
    }
    if target.exists() {
        return Err(FsError::AlreadyExists(normalize_rel(to)));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&source, &target)?;
    Ok(())
}

/// Delete `relative`, recursively for a directory (A11).
///
/// No trash and no undo: this is the checkout, and the checkout is under git —
/// which is the real undo, and a far better one than a bespoke bin the daemon
/// would have to keep, garbage-collect and explain. The GUI asks first.
pub fn delete_path(root: &Path, relative: &str) -> Result<(), FsError> {
    let root = canonicalize_root(root)?;
    let path = resolve_inside(&root, relative)?;
    // The workspace root is not a path *inside* the workspace. `resolve_inside`
    // rejects `..` but is perfectly happy with `""`, `"/"` and `"."`, all of
    // which resolve to the root — and this function removes directories
    // recursively, so that is the whole checkout.
    if path == root {
        return Err(FsError::EscapesWorkspace(relative.to_string()));
    }
    if !path.exists() {
        return Err(FsError::NotFound(normalize_rel(relative)));
    }
    if path.is_dir() {
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
    let out = run_git(
        Some(root),
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for path in out.stdout.split('\0') {
        if path.is_empty() {
            continue;
        }
        if entries.len() >= MAX_TREE_ENTRIES {
            truncated = true;
            break;
        }
        if path.ends_with('/') {
            continue;
        }
        entries.push(FileEntry {
            path: normalize_rel(path),
            kind: EntryKind::File,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(FileTree { entries, truncated })
}

fn list_via_walk(root: &Path) -> Result<FileTree, FsError> {
    let mut entries = Vec::new();
    let mut truncated = false;
    walk(root, root, &mut entries, &mut truncated)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(FileTree { entries, truncated })
}

fn walk(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<FileEntry>,
    truncated: &mut bool,
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
        if name == ".git" || name == "node_modules" || name == "target" {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(root, &path, entries, truncated)?;
        } else if ft.is_file() || ft.is_symlink() {
            if entries.len() >= MAX_TREE_ENTRIES {
                *truncated = true;
                return Ok(());
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            entries.push(FileEntry {
                path: rel,
                kind: EntryKind::File,
            });
        }
    }
    Ok(())
}

fn search_by_name(root: &Path, query: &str, limit: usize) -> Result<SearchResults, FsError> {
    let tree = list_files(root)?;
    let mut scored: Vec<(i32, usize, SearchMatch)> = Vec::new();
    for (order, entry) in tree.entries.into_iter().enumerate() {
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
            },
        ));
    }
    // Best first, and ties in listing order so a query that matches a whole
    // directory does not reshuffle it on the next keystroke.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let truncated = scored.len() > limit;
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

    parse_grep_z(out.stdout.as_bytes(), limit)
}

fn search_content_walk(root: &Path, query: &str, limit: usize) -> Result<SearchResults, FsError> {
    let tree = list_via_walk(root)?;
    let mut matches = Vec::new();
    let mut truncated = false;
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
        for (idx, line) in contents.text.lines().enumerate() {
            if let Some(col) = line.find(query) {
                if matches.len() >= limit {
                    truncated = true;
                    break;
                }
                matches.push(SearchMatch {
                    path: entry.path.clone(),
                    line: (idx + 1) as u32,
                    column: (col + 1) as u32,
                    text: line.to_string(),
                });
            }
        }
    }
    Ok(SearchResults { matches, truncated })
}

fn parse_grep_z(stdout: &[u8], limit: usize) -> Result<SearchResults, FsError> {
    // `git grep -z -n` emits path\0line\0text\n per hit.
    let mut matches = Vec::new();
    let mut truncated = false;
    let text = String::from_utf8_lossy(stdout);
    let mut remaining = text.as_ref();
    while !remaining.is_empty() {
        let Some((path, rest)) = remaining.split_once('\0') else {
            break;
        };
        let Some((line_s, rest)) = rest.split_once('\0') else {
            break;
        };
        let (line_text, rest) = rest.split_once('\n').unwrap_or((rest, ""));
        remaining = rest;
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        let Ok(line) = line_s.parse::<u32>() else {
            continue;
        };
        matches.push(SearchMatch {
            path: path.replace('\\', "/"),
            line,
            column: 0,
            text: line_text.to_string(),
        });
    }
    Ok(SearchResults { matches, truncated })
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
    parse_grep_z(out.stdout.as_bytes(), MAX_DEFINITION_SCAN)
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
///
/// The scoring is the point of the change: `search_by_name` used to keep the
/// first `limit` subsequence hits in directory-walk order, so a query whose
/// best answer sorted late was answered with `truncated: true` and without it.
fn fuzzy_score(path: &str, query: &str) -> Option<i32> {
    let needle: Vec<char> = query.to_lowercase().chars().filter(|c| *c != ' ').collect();
    if needle.is_empty() {
        return Some(0);
    }

    let raw: Vec<char> = path.chars().collect();
    let hay: Vec<char> = path.to_lowercase().chars().collect();

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
        if starts_word(&raw, &hay, index) {
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
fn starts_word(raw: &[char], hay: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let before = hay[i - 1];
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
    let joined = root.join(rel);
    // For writes to new files the path may not exist yet — canonicalize the
    // parent and re-join the file name.
    if joined.exists() {
        let canon = fs::canonicalize(&joined)?;
        if !canon.starts_with(root) {
            return Err(FsError::EscapesWorkspace(relative.to_string()));
        }
        return Ok(canon);
    }
    if let Some(parent) = joined.parent() {
        if parent.exists() {
            let parent = fs::canonicalize(parent)?;
            if !parent.starts_with(root) {
                return Err(FsError::EscapesWorkspace(relative.to_string()));
            }
            return Ok(parent.join(joined.file_name().unwrap_or_default()));
        }
    }
    if !joined.starts_with(root) {
        return Err(FsError::EscapesWorkspace(relative.to_string()));
    }
    Ok(joined)
}

fn normalize_rel(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

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
        let all = parse_grep_z(raw, 10).unwrap();
        assert_eq!(all.matches.len(), 2);
        assert_eq!(all.matches[0].text, "lookup();");
        assert_eq!(all.matches[1].path, "second\nfile.ts");
        assert_eq!(all.matches[1].line, 9);
        assert_eq!(all.matches[1].text, "lookup<T>(): T;");
        assert!(!all.truncated);
        let limited = parse_grep_z(raw, 1).unwrap();
        assert_eq!(limited.matches.len(), 1);
        assert!(limited.truncated);
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
    fn respects_gitignore() {
        let tmp = git_repo();
        fs::write(tmp.path().join(".gitignore"), "secret.txt\n").unwrap();
        fs::write(tmp.path().join("secret.txt"), "nope").unwrap();
        fs::write(tmp.path().join("ok.txt"), "yes").unwrap();
        let tree = list_files(tmp.path()).unwrap();
        let paths: Vec<_> = tree.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"ok.txt"));
        assert!(!paths.contains(&"secret.txt"));
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
    fn rejects_binary() {
        let tmp = git_repo();
        fs::write(tmp.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        let c = read_file(tmp.path(), "b.bin").unwrap();
        assert!(c.binary);
        assert!(c.text.is_empty());
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
