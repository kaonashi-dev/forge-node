//! Workspace filesystem views for the in-app editor (ADR-012).
//!
//! Runtime-only state, like [`crate::diff::WorkspaceDiff`] and
//! [`crate::pull_request::PullRequest`]: computed on demand from the checkout
//! on disk, never stored and never authoritative. A file that came out of a
//! database would describe a path that no longer exists.

use serde::{Deserialize, Serialize};

use crate::ids::WorkspaceId;

/// Whether a listed path is a file or a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
}

/// One path relative to the workspace root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Workspace-relative path, forward-slashed.
    pub path: String,
    /// File or directory.
    pub kind: FileKind,
    /// Excluded by `.gitignore`. Listed so a build directory is reachable, and
    /// flagged so the tree can grey it and keep it collapsed: an ignored path
    /// is not part of the work, but it is still a file someone opens.
    pub ignored: bool,
}

/// A bounded listing of paths under a workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTree {
    /// Which checkout this describes.
    pub workspace_id: WorkspaceId,
    /// Relative paths. From `git ls-files` these are files only; the GUI
    /// synthesises directory rows when it builds the tree. Tracked paths come
    /// first, then the ignored ones, each group sorted.
    pub entries: Vec<FileEntry>,
    /// Whether entries were dropped for exceeding the service budget.
    pub truncated: bool,
}

/// The text of one file, plus the revision a later write must present.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContents {
    /// Which checkout this was read from.
    pub workspace_id: WorkspaceId,
    /// Workspace-relative path.
    pub path: String,
    /// UTF-8 text. Empty when [`FileContents::binary`] or
    /// [`FileContents::too_large`].
    pub text: String,
    /// Hash of the on-disk bytes; required by `WriteFile`.
    pub revision: String,
    /// Language hint for the editor (`"rust"`, `"typescript"`, …), or empty.
    pub language: String,
    /// True when a `NUL` was found in the probe window.
    pub binary: bool,
    /// True when the file exceeded the byte budget and was not opened.
    pub too_large: bool,
}

/// One image from a checkout, for a Markdown preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContents {
    /// Which checkout this was read from.
    pub workspace_id: WorkspaceId,
    /// Workspace-relative path, as asked for.
    pub path: String,
    /// Media type from the extension (`image/png`, `image/svg+xml`, …).
    pub mime: String,
    /// The file's bytes as base64: the last hop is JSON into a WebView, where a
    /// byte array would serialize as a list of numbers three times the size.
    pub data: String,
}

/// Name match or content match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SearchKind {
    /// Subsequence match against the relative path.
    Name,
    /// Line match against file contents.
    Content,
    /// Lines that *declare* the queried symbol, not every line that mentions it.
    ///
    /// Its own kind rather than a crafted `Content` query because the pattern
    /// is not the caller's to write: the service greps for the bare word and
    /// decides what a declaration looks like, so the GUI never ships a regex
    /// that has to be right in eleven languages at once.
    Definition,
}

/// One hit from a workspace search.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    /// Which checkout was searched.
    pub workspace_id: WorkspaceId,
    /// What was searched for. Answers carry no request id, so this is what
    /// tells a caller whether the hits on screen are the ones it asked for.
    pub query: String,
    /// Hits, capped by the service.
    pub matches: Vec<SearchMatch>,
    /// Whether more hits existed beyond the cap.
    pub truncated: bool,
}
