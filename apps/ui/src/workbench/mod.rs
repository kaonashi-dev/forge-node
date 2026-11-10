//! Read-only views of a workspace beside its sessions: its uncommitted changes
//! and its files. Every read is answered by the daemon on the client worker
//! (`GetWorkspaceDiff`, `ListDirectory`, `ReadFile`, `ReadImage`,
//! `SearchFiles`, `WatchFiles`); this side only keeps the answers and never
//! touches the checkout (ADR-012).

pub mod compare;
pub mod conflict;
pub mod csv;
pub mod decorations;
pub mod hooks;
pub mod patch;
pub mod recent;
pub mod search;
pub mod search_view;
pub mod view;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use domain::{DirectoryListing, FileContents, FileEntry, FileKind, WorkspaceDiff, WorkspaceId};
use rune_kit::diff::DiffState;
use rune_kit::syntax::Highlight;
use rune_kit::{FileNode, TreeModel, natural_cmp};

use crate::file_icons::FileIcon;
use crate::pr_detail;

/// What the centre shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CenterMode {
    #[default]
    Session,
    Projects,
    Changes,
    Files,
    Settings,
    /// Project content search, ⇧⌘F.
    Search,
    /// One session's changes since its baseline.
    SessionChanges,
    /// A structured Forge Review of the focused checkout.
    Review,
    /// A branch against the branch a pull request would target.
    Compare,
    /// One pull request from the last refresh.
    PrDetail,
    /// Preparing a pull request with an agent.
    PrCompose,
}

/// The rendered surface a file opens on instead of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    Markdown,
    Csv,
    Svg,
    Image,
}

impl PreviewKind {
    /// A raster has no text the editor would open; the others are a
    /// rendering of a file that still has a source.
    #[must_use]
    pub fn has_source(self) -> bool {
        self != PreviewKind::Image
    }
}

/// Which preview a path opens in, as the original client's `previewKindFor`: by extension,
/// and a dotfile's name is not an extension. Images are the extensions the
/// daemon's `ReadImage` serves, so a preview never asks for bytes it will be
/// refused.
#[must_use]
pub fn preview_kind(path: &str) -> Option<PreviewKind> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let dot = name.rfind('.').filter(|dot| *dot > 0)?;
    Some(match name[dot + 1..].to_ascii_lowercase().as_str() {
        "md" | "markdown" | "mdx" => PreviewKind::Markdown,
        "csv" | "tsv" => PreviewKind::Csv,
        "svg" => PreviewKind::Svg,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" => PreviewKind::Image,
        _ => return None,
    })
}

#[must_use]
pub fn opens_rendered(path: &str) -> bool {
    preview_kind(path).is_some()
}

/// The folder `path` is listed in; `""` is the root.
#[must_use]
pub fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..slash])
}

/// One visit to a checkout. A reply carries the scope it was asked in, so
/// after leaving a checkout and coming back (A → B → A) an answer to the
/// first visit is dropped rather than landing over the second's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub workspace: WorkspaceId,
    pub visit: u64,
}

/// What a checkout's status said last: the oid HEAD names and whether the
/// tree is dirty, as the original client's `checkoutWatch` remembers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutMark {
    pub head: Option<String>,
    pub dirty: bool,
}

/// Whether git moved under a checkout between two marks. An unmeasured
/// status (`head: None`) says nothing, and neither does the first look,
/// where there is nothing old to throw away. A `git pull` keeps the branch
/// name and rewrites HEAD, which is why the oid is what is compared.
#[must_use]
pub fn checkout_moved(before: Option<&CheckoutMark>, after: Option<&CheckoutMark>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) if after.head.is_some() => {
            before.head != after.head || before.dirty != after.dirty
        }
        _ => false,
    }
}

/// What the sidebar shows, as the original client's sidebar views. The centre keeps the
/// session, a diff picked from Git, or Settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Panel {
    #[default]
    Projects,
    Files,
    History,
    PullRequests,
    Git,
    Processes,
}

/// A read the worker should make for the current workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    Diff,
    Directory(String),
    File(String),
    /// An image's bytes, for a preview.
    Image(String),
    /// Paths ranked against a palette query.
    Search(String),
    /// Declaration candidates for a typed symbol, from `SearchFiles` kind
    /// `Definition`; the daemon validates the word and never sees a regex.
    Definition(String),
    /// Lines that contain the needle, for project search.
    Content(String),
    /// Whether a rebase, merge or cherry-pick is stopped, read from git.
    Rebase,
    /// A branch against `base`, or the daemon's default base when `None`.
    Compare(Option<String>),
}

/// A step on a stopped operation; each answers with the state git is in now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebaseStep {
    Resolve(Vec<String>),
    Continue,
    Abort,
}

/// A child id that marks a folder whose listing has not arrived, so the tree
/// draws a chevron for it (a row's chevron comes from having children). The
/// NUL cannot occur in a path.
const PENDING: &str = "\u{0}loading";
/// Lines a preview shows; past this a file is a download, not a read.
const MAX_PREVIEW_LINES: usize = 20_000;
/// Folders one connection may watch and the longest path, as the daemon
/// refuses a set past either whole.
pub const MAX_WATCHED: usize = 128;
const MAX_WATCH_PATH_BYTES: usize = 4096;
/// A file change re-reads the diff after this, so a burst costs one read.
const DIFF_COALESCE: Duration = Duration::from_millis(150);
/// Git subprocesses cost seconds on a large checkout, so a watched change
/// re-reads the diff at most this often; a gesture does not wait.
const DIFF_WATCH_FLOOR: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct Workbench {
    pub mode: CenterMode,
    pub panel: Panel,
    visit: u64,
    pub workspace: Option<WorkspaceId>,
    pub changes: Changes,
    pub files: Files,
    pub search: search::ProjectSearch,
    pub compare: compare::Compare,
    /// Branches per project, read on demand for the compare base picker.
    pub branches: HashMap<domain::ProjectId, client::Branches>,
    pub branches_pending: HashSet<domain::ProjectId>,
    /// The pull request the centre shows, by `pr_filters::key`.
    pub pr_detail: Option<String>,
    pub pr_tab: pr_detail::Tab,
    /// The last read's searchable pull-request rows.
    pub pr_index: crate::pr_filters::Index,
    reads: Vec<Read>,
    watch: Watch,
}

/// What this connection has asked the daemon to watch. Connection-scoped:
/// a new connection starts with nothing watched.
#[derive(Default)]
struct Watch {
    generation: u64,
    sent: Option<(WorkspaceId, Vec<String>)>,
    ticket: u64,
    /// Folders listed while nothing was watched, listed again once this set
    /// is armed: a change in between was seen by no one.
    rescan: Vec<String>,
    armed: bool,
    /// The checkout, whether Files showed and the shown preview's folder
    /// when the set was last built.
    shown: Option<(Option<WorkspaceId>, bool, Option<String>)>,
    /// Ask again even for an unchanged set: a refresh re-arms it.
    stale: bool,
}

/// A `WatchFiles` to send: the folders, and the ticket its answer carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchSet {
    pub directories: Vec<String>,
    pub ticket: u64,
}

/// One changed path as git reported it, with its patch already parsed.
pub struct ChangedFile {
    pub path: String,
    pub icon: FileIcon,
    pub status: domain::DiffStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub truncated: bool,
    pub patch: String,
    diffs: Vec<rune_kit::diff::FileDiff>,
}

#[derive(Default)]
pub struct Changes {
    pub state: DiffState,
    pub files: Vec<ChangedFile>,
    /// The checkout this diff belongs to.
    pub workspace: Option<WorkspaceId>,
    /// The file whose patch `state` shows.
    pub selected: Option<usize>,
    /// Read on every load, never remembered: staging a path is what takes it
    /// off the list, and only git knows.
    pub rebase: Option<domain::RebaseState>,
    pub rebase_error: Option<String>,
    pub branch: Option<String>,
    pub file_count: usize,
    /// Paths git reported with no patch to draw: binary, or over the budget.
    pub without_patch: Vec<String>,
    pub truncated: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub decorations: Rc<decorations::Decorations>,
    /// The conflicted path the three-way view is showing.
    pub conflict_focus: Option<String>,
    /// Read off the focused path's patch, so the view never parses per frame.
    pub conflict_blocks: Vec<conflict::Block>,
    /// The focused path's patch as rows, for the proposed-resolution view.
    pub conflict_rows: Vec<patch::Row>,
    pub conflict_pairs: Vec<patch::Pair>,
    /// Whether the focused path has a patch to read at all.
    pub conflict_readable: bool,
    /// The region "Next conflict" last jumped to.
    pub conflict_region: usize,
    last_read: Option<Instant>,
    /// When a watched change re-reads the diff.
    due: Option<Instant>,
}

impl Changes {
    /// The working tree as the prompt composer and the diff view need it.
    #[must_use]
    pub fn diff(&self) -> Option<WorkspaceDiff> {
        if !self.loaded || self.error.is_some() {
            return None;
        }
        Some(WorkspaceDiff {
            workspace_id: self.workspace?,
            branch: self.branch.clone(),
            files: self
                .files
                .iter()
                .map(|file| domain::DiffFile {
                    path: file.path.clone(),
                    status: file.status,
                    additions: file.additions,
                    deletions: file.deletions,
                    patch: file.patch.clone(),
                    binary: file.binary,
                    truncated: file.truncated,
                })
                .collect(),
            truncated: self.truncated,
        })
    }

    /// Picks the conflicted path the block view shows and reads its markers.
    pub fn focus_conflict(&mut self, path: String) {
        self.conflict_focus = Some(path);
        self.conflict_region = 0;
        self.read_conflict();
    }

    /// The next (`by = 1`) or previous (`by = -1`) conflicted path.
    pub fn step_conflict(&mut self, by: isize) {
        let paths: Vec<String> = self
            .rebase
            .as_ref()
            .map(|state| {
                state
                    .conflicts
                    .iter()
                    .map(|file| file.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        let at = self
            .conflict_focus
            .as_ref()
            .and_then(|path| paths.iter().position(|candidate| candidate == path))
            .unwrap_or(0);
        let next = (at as isize + by).rem_euclid(paths.len() as isize) as usize;
        self.focus_conflict(paths[next].clone());
    }

    /// Wraps the region cursor; the view scrolls to the returned index.
    pub fn next_conflict_region(&mut self) -> Option<usize> {
        if self.conflict_blocks.is_empty() {
            return None;
        }
        self.conflict_region = (self.conflict_region + 1) % self.conflict_blocks.len();
        Some(self.conflict_region)
    }

    fn read_conflict(&mut self) {
        self.conflict_blocks.clear();
        self.conflict_rows.clear();
        self.conflict_pairs.clear();
        self.conflict_readable = false;
        let Some(path) = &self.conflict_focus else {
            return;
        };
        let Some(file) = self.files.iter().find(|file| &file.path == path) else {
            return;
        };
        self.conflict_readable = !file.binary && !file.truncated && !file.patch.is_empty();
        if !self.conflict_readable {
            return;
        }
        self.conflict_blocks = conflict::blocks(&file.patch);
        self.conflict_rows = patch::rows(&file.patch);
        self.conflict_pairs = patch::pairs(&self.conflict_rows);
    }

    fn sync_conflict(&mut self) {
        let paths: Vec<String> = self
            .rebase
            .as_ref()
            .filter(|state| !state.conflicts.is_empty())
            .map(|state| {
                state
                    .conflicts
                    .iter()
                    .map(|file| file.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if paths.is_empty() {
            self.conflict_focus = None;
            self.conflict_blocks.clear();
            self.conflict_rows.clear();
            self.conflict_pairs.clear();
            self.conflict_readable = false;
            return;
        }
        let keep = self
            .conflict_focus
            .as_ref()
            .is_some_and(|path| paths.contains(path));
        if !keep {
            self.conflict_focus = Some(paths[0].clone());
            self.conflict_region = 0;
        }
        self.read_conflict();
    }
}

pub const FILES_TREE_KEY: &str = "files-tree";

pub struct Files {
    pub tree: TreeModel<FileNode<FileIcon>>,
    pub preview: Option<Preview>,
    pub error: Option<String>,
    pub filter: rune::TextEdit,
    /// Why some folders are not live, or why none are.
    pub watch_notice: Option<String>,
    /// Each folder's entries as listed, folders first; `""` is the root.
    listings: HashMap<String, Vec<FileEntry>>,
    requested: HashSet<String>,
    root_requested: bool,
    /// The path the preview shows or was asked for; answers for any other drop.
    previewing: Option<String>,
    /// Every open folder, as of the last `take_reads`.
    open: Vec<String>,
    open_changed: bool,
    /// The active editor's path, and whether the tree has shown it yet.
    follow: Option<String>,
    followed: bool,
    /// A preview image had not decoded when the view was last built.
    image_pending: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            tree: TreeModel::new().key(FILES_TREE_KEY),
            preview: None,
            error: None,
            filter: rune::TextEdit::new(""),
            watch_notice: None,
            listings: HashMap::new(),
            requested: HashSet::new(),
            root_requested: false,
            previewing: None,
            open: Vec::new(),
            open_changed: false,
            follow: None,
            followed: false,
            image_pending: false,
        }
    }
}

pub struct Preview {
    pub path: String,
    pub kind: Option<PreviewKind>,
    /// The source as text rather than its rendering.
    pub source: bool,
    pub table: Option<csv::CsvTable>,
    pub image: Option<Result<rune::Image, String>>,
    pub language: String,
    pub text: Rc<str>,
    /// Byte range of each shown line in `text`.
    pub lines: Vec<Range<usize>>,
    /// The file has more lines than are shown.
    pub cut: bool,
    pub binary: bool,
    /// The daemon refused to send it whole; nothing is shown.
    pub too_large: bool,
    pub highlights: Rc<[Highlight]>,
}

impl Workbench {
    /// Points the views at `workspace`; a different one drops what was read.
    /// A project search keeps its query and asks it again there.
    pub fn set_workspace(&mut self, workspace: Option<WorkspaceId>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        self.visit += 1;
        self.changes = Changes::default();
        self.files = Files::default();
        self.compare = compare::Compare::default();
        self.reads.clear();
        self.search.rescope(Instant::now());
        if self.shows_diff() {
            self.read_diff();
        }
        if self.mode == CenterMode::Compare {
            self.compare.request(None);
        }
    }

    #[must_use]
    pub fn scope(&self) -> Option<Scope> {
        self.workspace.map(|workspace| Scope {
            workspace,
            visit: self.visit,
        })
    }

    /// Projects, Files and Pull Requests are sidebar panels and Git is both:
    /// asking for one shows its panel, and only Changes, Search and Settings
    /// (or the session) take the centre.
    pub fn show(&mut self, mode: CenterMode) {
        match mode {
            CenterMode::Projects => self.show_panel(Panel::Projects),
            CenterMode::Files => self.show_panel(Panel::Files),
            CenterMode::Changes => {
                self.show_panel(Panel::Git);
                self.mode = CenterMode::Changes;
            }
            CenterMode::Search => {
                self.search.edit.focused = true;
                self.mode = CenterMode::Search;
            }
            CenterMode::SessionChanges | CenterMode::Review => self.mode = mode,
            CenterMode::Compare => {
                self.mode = CenterMode::Compare;
                if self.compare.data.is_none() && !self.compare.loading() {
                    self.compare.request(None);
                }
            }
            CenterMode::PrDetail => self.mode = CenterMode::PrDetail,
            CenterMode::PrCompose => {
                self.mode = CenterMode::PrCompose;
                if !self.changes.loaded {
                    self.read_diff();
                }
            }
            CenterMode::Session | CenterMode::Settings => self.mode = mode,
        }
    }

    /// Shows `path` rendered in the centre.
    pub fn preview(&mut self, path: &str) {
        self.files.preview = None;
        self.files.error = None;
        self.files.previewing = Some(path.to_string());
        self.read_preview(path);
        self.mode = CenterMode::Files;
    }

    fn read_preview(&mut self, path: &str) {
        let kind = preview_kind(path);
        if kind != Some(PreviewKind::Image) {
            self.queue(Read::File(path.to_string()));
        }
        if matches!(kind, Some(PreviewKind::Image | PreviewKind::Svg)) {
            self.queue(Read::Image(path.to_string()));
        }
    }

    /// Reads the shown preview again, keeping it on screen until the answer.
    fn reread_preview(&mut self) {
        if let Some(path) = self.files.previewing.clone() {
            self.read_preview(&path);
        }
    }

    pub fn toggle_source(&mut self) {
        if let Some(preview) = &mut self.files.preview
            && preview.kind.is_some_and(PreviewKind::has_source)
        {
            preview.source = !preview.source;
        }
    }

    /// Where git's view of the checkout shows: the Git panel and its diff,
    /// the Files tree's decorations, or the compose view's change summary.
    fn shows_diff(&self) -> bool {
        matches!(self.panel, Panel::Git | Panel::Files)
            || matches!(self.mode, CenterMode::Changes | CenterMode::PrCompose)
    }

    fn read_diff(&mut self) {
        self.queue(Read::Diff);
        if self.panel == Panel::Git || self.mode == CenterMode::Changes {
            self.queue(Read::Rebase);
        }
    }

    /// Git moved under the checkout: re-read what git shows now, or forget
    /// it so the next look reads again. The file tree is left as opened.
    pub fn checkout_moved(&mut self) {
        if self.shows_diff() {
            self.read_diff();
        } else {
            self.changes.loaded = false;
        }
        if self.mode == CenterMode::Compare {
            self.compare.reload();
        }
    }

    pub fn show_panel(&mut self, panel: Panel) {
        self.panel = panel;
        if self.shows_diff() && !self.changes.loaded {
            self.read_diff();
        }
    }

    /// Reads what shows again. The tree keeps what was open: every listed
    /// folder is listed again and the watch is armed anew.
    pub fn refresh(&mut self) {
        if self.shows_diff() {
            self.read_diff();
        }
        if self.mode == CenterMode::Compare {
            self.compare.reload();
        }
        if self.panel == Panel::Files {
            self.rescan();
            self.watch.stale = true;
        }
        if self.mode == CenterMode::Files {
            self.reread_preview();
            self.watch.stale = true;
        }
    }

    fn rescan(&mut self) {
        let mut listed: Vec<String> = self.files.listings.keys().cloned().collect();
        listed.sort();
        for dir in listed {
            self.queue(Read::Directory(dir));
        }
    }

    /// Lists `dir` again, after a write the daemon does not announce.
    pub fn reread(&mut self, dir: &str) {
        self.queue(Read::Directory(dir.to_string()));
    }

    fn queue(&mut self, read: Read) {
        if !self.reads.contains(&read) {
            self.reads.push(read);
        }
    }

    /// The reads to send now: what was asked for, the listing of every
    /// folder the person opened since the last call, and the diff a watched
    /// change made due.
    pub fn take_reads(&mut self, now: Instant) -> Vec<Read> {
        if self.workspace.is_none() {
            self.reads.clear();
            self.files.open.clear();
            return Vec::new();
        }
        if self.panel == Panel::Files {
            if !self.files.root_requested {
                self.files.root_requested = true;
                self.queue(Read::Directory(String::new()));
            }
            let open = self.files.expanded();
            for dir in &open {
                if !self.files.listings.contains_key(dir)
                    && self.files.requested.insert(dir.clone())
                {
                    self.queue(Read::Directory(dir.clone()));
                }
            }
            if open != self.files.open {
                self.files.open = open;
                self.files.open_changed = true;
            }
        } else if !self.files.open.is_empty() {
            self.files.open.clear();
            self.files.open_changed = true;
        }
        if self.shows_diff() && self.changes.due.is_some_and(|due| due <= now) {
            self.changes.due = None;
            self.read_diff();
        }
        if self.mode == CenterMode::Compare
            && let Some(base) = self.compare.take_request()
        {
            self.queue(Read::Compare(base));
        }
        let reads = std::mem::take(&mut self.reads);
        if reads.contains(&Read::Diff) {
            self.changes.last_read = Some(now);
        }
        reads
    }

    /// The folders to watch now, when they differ from what this connection
    /// last asked for: the root and every open folder while Files shows, and
    /// the shown preview's folder whatever the sidebar shows. Sent before the
    /// reads `take_reads` returned, so a folder is watched before it is read
    /// and no change falls between.
    pub fn take_watch(&mut self, generation: u64) -> Option<(WorkspaceId, WatchSet)> {
        if self.watch.generation != generation {
            self.watch = Watch {
                generation,
                ..Watch::default()
            };
        }
        let preview_dir = (self.mode == CenterMode::Files)
            .then(|| self.files.previewing.as_deref().map(parent_of))
            .flatten()
            .map(str::to_string);
        let shown = (self.workspace, self.panel == Panel::Files, preview_dir);
        let open_changed = std::mem::take(&mut self.files.open_changed);
        let stale = std::mem::take(&mut self.watch.stale);
        if !open_changed && !stale && self.watch.shown.as_ref() == Some(&shown) {
            return None;
        }
        let (_, tree, preview_dir) = &shown;
        let wanted = self.workspace.and_then(|workspace| {
            let mut dirs: Vec<String> = Vec::new();
            if *tree {
                dirs.push(String::new());
                dirs.extend(self.files.open.iter().cloned());
            }
            dirs.extend(preview_dir.iter().cloned());
            dirs.sort();
            dirs.dedup();
            (!dirs.is_empty()).then_some((workspace, dirs))
        });
        self.watch.shown = Some(shown);
        if stale {
            self.watch.sent = None;
        }
        let (workspace, mut dirs) = match wanted {
            Some(wanted) => wanted,
            None => (self.watch.sent.as_ref()?.0, Vec::new()),
        };
        let total = dirs.len();
        dirs.retain(|dir| dir.len() <= MAX_WATCH_PATH_BYTES);
        dirs.truncate(MAX_WATCHED);
        if self
            .watch
            .sent
            .as_ref()
            .is_some_and(|(sent, sent_dirs)| *sent == workspace && *sent_dirs == dirs)
        {
            return None;
        }
        let first = !self.watch.armed
            || self
                .watch
                .sent
                .as_ref()
                .is_none_or(|(sent, _)| *sent != workspace);
        self.watch.rescan = if first && !dirs.is_empty() {
            self.files.listings.keys().cloned().collect()
        } else {
            Vec::new()
        };
        self.files.watch_notice = (dirs.len() != total).then(|| {
            format!(
                "Live updates cover at most {MAX_WATCHED} folders. Refresh to update the others."
            )
        });
        self.watch.armed = false;
        self.watch.sent = (!dirs.is_empty()).then(|| (workspace, dirs.clone()));
        self.watch.ticket += 1;
        Some((
            workspace,
            WatchSet {
                directories: dirs,
                ticket: self.watch.ticket,
            },
        ))
    }

    /// The daemon's answer to a `WatchFiles`; one for a set since replaced
    /// is dropped.
    pub fn apply_watch(&mut self, ticket: u64, result: Result<(), String>) {
        if ticket != self.watch.ticket {
            return;
        }
        match result {
            Ok(()) => {
                self.watch.armed = self.watch.sent.is_some();
                for dir in std::mem::take(&mut self.watch.rescan) {
                    if self.files.listings.contains_key(&dir) {
                        self.queue(Read::Directory(dir));
                    }
                }
            }
            Err(error) => {
                self.watch.armed = false;
                self.files.watch_notice = Some(format!("Live updates unavailable: {error}"));
            }
        }
    }

    /// A watched path changed on disk (`FileChanged`). Its folder is listed
    /// again, and an empty path, sent after the daemon lost native events,
    /// lists every folder again. The diff behind the decorations is re-read
    /// once the burst settles, at most every [`DIFF_WATCH_FLOOR`].
    pub fn file_changed(&mut self, workspace: WorkspaceId, path: &str, now: Instant) {
        if self.workspace != Some(workspace) {
            return;
        }
        self.search.files_changed();
        if self.changes.due.is_none() {
            let floor = self
                .changes
                .last_read
                .map_or(now, |read| read + DIFF_WATCH_FLOOR);
            self.changes.due = Some((now + DIFF_COALESCE).max(floor));
        }
        if path.is_empty() {
            self.rescan();
            self.reread_preview();
            return;
        }
        for dir in [parent_of(path), path] {
            if self.files.listings.contains_key(dir) {
                self.queue(Read::Directory(dir.to_string()));
            }
        }
        if self.files.previewing.as_deref() == Some(path) {
            self.reread_preview();
        }
    }

    pub fn apply_rebase(&mut self, scope: Scope, state: Result<domain::RebaseState, String>) {
        if self.scope() != Some(scope) {
            return;
        }
        match state {
            Ok(state) => {
                self.changes.rebase_error = None;
                self.changes.rebase = state.operation.is_some().then_some(state);
            }
            Err(error) => self.changes.rebase_error = Some(error),
        }
        self.changes.sync_conflict();
    }

    pub fn apply_diff(&mut self, scope: Scope, diff: Result<WorkspaceDiff, String>) {
        if self.scope() != Some(scope) {
            return;
        }
        let changes = &mut self.changes;
        changes.loaded = true;
        changes.workspace = Some(scope.workspace);
        match diff {
            Ok(diff) => {
                changes.error = None;
                changes.branch = diff.branch;
                changes.truncated = diff.truncated;
                changes.file_count = diff.files.len();
                changes.decorations = Rc::new(decorations::Decorations::from_diff(&diff.files));
                changes.without_patch = diff
                    .files
                    .iter()
                    .filter(|file| file.patch.is_empty())
                    .map(|file| file.path.clone())
                    .collect();
                let kept = changes
                    .selected
                    .and_then(|index| changes.files.get(index))
                    .map(|file| file.path.clone());
                changes.files = diff
                    .files
                    .into_iter()
                    .map(|file| ChangedFile {
                        diffs: rune_kit::diff::parse(&file.patch),
                        icon: crate::file_icons::for_path(&file.path),
                        path: file.path,
                        status: file.status,
                        additions: file.additions,
                        deletions: file.deletions,
                        binary: file.binary,
                        truncated: file.truncated,
                        patch: file.patch,
                    })
                    .collect();
                let again = kept.and_then(|path| changes.files.iter().position(|f| f.path == path));
                self.select_change(again.unwrap_or(0));
                self.changes.sync_conflict();
            }
            Err(error) => {
                changes.error = Some(error);
                self.changes.sync_conflict();
            }
        }
    }

    /// Accepts the daemon's branch compare for the request it answers. A late
    /// answer to an older base is dropped rather than painted under the newer
    /// one.
    pub fn apply_compare(
        &mut self,
        scope: Scope,
        requested: Option<String>,
        result: Result<domain::BranchCompare, String>,
    ) {
        if self.scope() != Some(scope) {
            return;
        }
        self.compare.apply(scope, requested, result);
    }

    /// A layout toggle is a redraw, never another git read.
    pub fn set_split(&mut self, split: bool) {
        self.changes.state.set_mode(if split {
            rune_kit::diff::DiffMode::Split
        } else {
            rune_kit::diff::DiffMode::Unified
        });
        self.compare.set_split(split);
    }

    /// Shows one changed file's patch, as the original client opens one file at a time.
    pub fn select_change(&mut self, index: usize) {
        let changes = &mut self.changes;
        match changes.files.get(index) {
            Some(file) => {
                changes.selected = Some(index);
                changes.state.set_files(file.diffs.clone());
                let conflicted = changes.rebase.as_ref().is_some_and(|state| {
                    state
                        .conflicts
                        .iter()
                        .any(|conflict| conflict.path == file.path)
                });
                if conflicted {
                    changes.conflict_focus = Some(file.path.clone());
                    changes.conflict_region = 0;
                    changes.read_conflict();
                }
            }
            None => {
                changes.selected = None;
                changes.state.set_files(Vec::new());
            }
        }
    }

    /// Reads a project's branches for the compare base picker, unless they are
    /// already here or in flight.
    pub fn request_branches(&mut self, project: domain::ProjectId) -> bool {
        if self.branches.contains_key(&project) || self.branches_pending.contains(&project) {
            return false;
        }
        self.branches_pending.insert(project);
        true
    }

    pub fn apply_branches(
        &mut self,
        project: domain::ProjectId,
        branches: Result<client::Branches, String>,
    ) {
        self.branches_pending.remove(&project);
        if let Ok(branches) = branches {
            self.branches.insert(project, branches);
        }
    }

    /// The branches the base picker can offer for `project`.
    #[must_use]
    pub fn branches_for(&self, project: domain::ProjectId) -> Option<&client::Branches> {
        self.branches.get(&project)
    }

    pub fn open_pr_detail(&mut self, key: String) {
        self.pr_detail = Some(key);
        self.pr_tab = pr_detail::Tab::Description;
        self.mode = CenterMode::PrDetail;
    }

    /// A folder's entries. A folder that can no longer be listed (deleted
    /// since it was opened) leaves the tree; only the root's failure is said.
    pub fn apply_directory(
        &mut self,
        scope: Scope,
        path: &str,
        listing: Result<DirectoryListing, String>,
    ) {
        if self.scope() != Some(scope) {
            return;
        }
        let files = &mut self.files;
        let mut entries = match listing {
            Ok(listing) => listing.entries,
            Err(error) if path.is_empty() => {
                files.error = Some(error);
                return;
            }
            Err(_) => {
                files.requested.remove(path);
                if files.listings.remove(path).is_some() {
                    files.rebuild();
                }
                return;
            }
        };
        entries.sort_by(|a, b| {
            let file = |kind: &FileKind| *kind != FileKind::Directory;
            file(&a.kind)
                .cmp(&file(&b.kind))
                .then_with(|| natural_cmp(&a.path, &b.path))
        });
        if files.listings.get(path) == Some(&entries) {
            return;
        }
        files.listings.insert(path.to_string(), entries);
        files.rebuild();
    }

    pub fn apply_file(&mut self, scope: Scope, file: Result<FileContents, String>) {
        if self.scope() != Some(scope) {
            return;
        }
        match file {
            Ok(file) => {
                if self.files.previewing.as_deref() != Some(file.path.as_str()) {
                    return;
                }
                self.files.error = None;
                let mut preview = Preview::new(file);
                if let Some(old) = self.files.preview.take()
                    && old.path == preview.path
                {
                    preview.image = old.image;
                    preview.source = old.source;
                }
                self.files.preview = Some(preview);
            }
            Err(error) => self.files.error = Some(error),
        }
    }

    /// An image for the preview, already handed to the decoder.
    pub fn apply_image(&mut self, scope: Scope, path: &str, image: Result<rune::Image, String>) {
        if self.scope() != Some(scope) || self.files.previewing.as_deref() != Some(path) {
            return;
        }
        match &mut self.files.preview {
            Some(preview) if preview.path == path => preview.image = Some(image),
            _ => self.files.preview = Some(Preview::image(path, image)),
        }
        self.files.image_pending = true;
    }

    /// Whether the preview's image finished decoding since the last call,
    /// so the view is built again to show it (or its error).
    pub fn image_landed(&mut self) -> bool {
        if !self.files.image_pending {
            return false;
        }
        let waiting =
            self.files.preview.as_ref().is_some_and(
                |preview| matches!(&preview.image, Some(Ok(image)) if !image.is_ready()),
            );
        self.files.image_pending = waiting;
        !waiting
    }

    pub fn filter_changed(&mut self) {
        self.files.apply_filter();
    }

    /// Keeps the tree on the active editor's file, as the original client's `treeFollow`.
    /// Only a new path is followed, so a re-listing does not yank the list
    /// back from wherever the person scrolled.
    pub fn follow(&mut self, path: Option<&str>) {
        if self.files.follow.as_deref() != path {
            self.files.follow = path.map(str::to_string);
            self.files.followed = false;
        }
    }

    #[must_use]
    pub fn following(&self) -> Option<&str> {
        self.files.follow.as_deref()
    }

    /// Opens the folders down to the followed file as their listings land,
    /// then selects it and scrolls it into view, once.
    pub fn reveal_followed(&mut self, tx: &mut rune::Tx) -> bool {
        if self.files.followed || self.panel != Panel::Files {
            return false;
        }
        let Some(path) = self.files.follow.clone() else {
            return false;
        };
        let tree = &mut self.files.tree;
        let mut changed = false;
        let mut end = 0;
        while let Some(slash) = path[end..].find('/') {
            end += slash;
            let dir = &path[..end];
            if tree.contains(dir) && !tree.is_expanded(dir) {
                tree.set_expanded(dir, true);
                changed = true;
            }
            end += 1;
        }
        if tree.reveal(&path, tx) {
            self.files.followed = true;
            changed = true;
        }
        changed
    }
}

impl Files {
    /// Every open folder, depth first.
    fn expanded(&self) -> Vec<String> {
        let mut open = Vec::new();
        let mut stack: Vec<String> = self.tree.children(None).map(str::to_string).collect();
        while let Some(id) = stack.pop() {
            if self.tree.is_expanded(&id) {
                stack.extend(self.tree.children(Some(&id)).map(str::to_string));
                open.push(id);
            }
        }
        open
    }

    #[must_use]
    pub fn filter_query(&self) -> &str {
        self.filter.text.trim()
    }

    /// Shows the loaded rows whose name holds the filter, with the folders
    /// that lead to them. Only what was listed is searched; ⌘P asks the
    /// daemon about the rest.
    fn apply_filter(&mut self) {
        let query = self.filter_query().to_lowercase();
        if query.is_empty() {
            self.tree.clear_filter();
        } else {
            self.tree
                .filter(|node| !node.path.is_empty() && node.name.to_lowercase().contains(&query));
        }
    }

    /// Builds the tree again from the listings, keeping what was open and
    /// selected: the model cannot drop a folder's placeholder in place.
    fn rebuild(&mut self) {
        let open = self.expanded();
        let selected: Vec<String> = self.tree.selected().map(str::to_string).collect();
        let mut tree = TreeModel::new().key(FILES_TREE_KEY);
        let mut stack: Vec<(Option<String>, &str)> = vec![(None, "")];
        while let Some((parent, dir)) = stack.pop() {
            let Some(entries) = self.listings.get(dir) else {
                continue;
            };
            for entry in entries {
                let folder = entry.kind == FileKind::Directory;
                let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
                tree.insert(
                    parent.as_deref(),
                    entry.path.clone(),
                    FileNode {
                        name: name.to_string(),
                        path: entry.path.clone(),
                        dir: folder,
                        data: (!folder).then(|| crate::file_icons::for_path(name)),
                    },
                );
                if !folder {
                    continue;
                }
                if self.listings.contains_key(&entry.path) {
                    stack.push((Some(entry.path.clone()), &entry.path));
                } else {
                    tree.insert(
                        Some(&entry.path),
                        format!("{}/{PENDING}", entry.path),
                        FileNode {
                            name: "Loading…".to_string(),
                            path: String::new(),
                            dir: false,
                            data: None,
                        },
                    );
                }
            }
        }
        for id in &open {
            tree.set_expanded(id, true);
        }
        for id in &selected {
            tree.select(id);
        }
        self.tree = tree;
        if !self.filter_query().is_empty() {
            self.apply_filter();
        }
    }
}

impl Preview {
    fn new(file: FileContents) -> Self {
        let kind = preview_kind(&file.path);
        let text: Rc<str> = Rc::from(file.text);
        let mut lines = Vec::new();
        let mut start = 0;
        for (index, _) in text.match_indices('\n') {
            lines.push(start..index);
            start = index + 1;
            if lines.len() == MAX_PREVIEW_LINES {
                break;
            }
        }
        let cut = lines.len() == MAX_PREVIEW_LINES && start < text.len();
        if !cut && start < text.len() {
            lines.push(start..text.len());
        }
        let readable = !file.binary && !file.too_large;
        let table = (readable && kind == Some(PreviewKind::Csv))
            .then(|| csv::parse(&text, csv::delimiter(&file.path)));
        let language = file.language;
        let highlights = if !readable
            || language == "markdown"
            || matches!(kind, Some(PreviewKind::Markdown | PreviewKind::Csv))
        {
            Rc::from(Vec::new())
        } else {
            rune_kit::syntax::highlight((!language.is_empty()).then_some(&language), &text)
        };
        Self {
            path: file.path,
            kind,
            source: false,
            table,
            image: None,
            language,
            text,
            lines,
            cut,
            binary: file.binary,
            too_large: file.too_large,
            highlights,
        }
    }

    /// A picture with no text read (yet): a raster, or an SVG whose source
    /// is still on its way.
    fn image(path: &str, image: Result<rune::Image, String>) -> Self {
        Self {
            path: path.to_string(),
            kind: preview_kind(path),
            source: false,
            table: None,
            image: Some(image),
            language: String::new(),
            text: Rc::from(""),
            lines: Vec::new(),
            cut: false,
            binary: false,
            too_large: false,
            highlights: Rc::from(Vec::new()),
        }
    }

    #[must_use]
    pub fn is_markdown(&self) -> bool {
        self.kind == Some(PreviewKind::Markdown) || self.language == "markdown"
    }

    /// The highlights inside `line`, shifted to start at the line.
    #[must_use]
    pub fn line_highlights(&self, line: &Range<usize>) -> Vec<Highlight> {
        let first = self
            .highlights
            .partition_point(|(range, _)| range.end <= line.start);
        self.highlights[first..]
            .iter()
            .take_while(|(range, _)| range.start < line.end)
            .map(|(range, paint)| {
                let start = range.start.max(line.start) - line.start;
                let end = range.end.min(line.end) - line.start;
                (start..end, *paint)
            })
            .filter(|(range, _)| range.start < range.end)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{DiffFile, DiffStatus};

    fn bench() -> (Workbench, WorkspaceId) {
        let workspace = WorkspaceId::new();
        let mut bench = Workbench::default();
        bench.set_workspace(Some(workspace));
        (bench, workspace)
    }

    fn entry(path: &str, kind: FileKind) -> FileEntry {
        FileEntry {
            path: path.into(),
            kind,
            ignored: false,
            symlink: None,
        }
    }

    fn listing(path: &str, entries: Vec<FileEntry>) -> DirectoryListing {
        DirectoryListing {
            path: path.into(),
            entries,
            truncated: false,
        }
    }

    #[test]
    fn the_files_view_lists_the_root_then_each_folder_once_it_is_opened() {
        let (mut bench, _) = bench();
        let scope = bench.scope().expect("a scope");
        bench.show(CenterMode::Files);
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![Read::Diff, Read::Directory(String::new())],
            "the diff decorates the tree"
        );
        bench.apply_directory(
            scope,
            "",
            Ok(listing(
                "",
                vec![
                    entry("README.md", FileKind::File),
                    entry("src", FileKind::Directory),
                ],
            )),
        );
        let roots: Vec<&str> = bench.files.tree.children(None).collect();
        assert_eq!(roots, vec!["src", "README.md"], "folders first");
        assert!(
            bench.files.tree.has_children("src"),
            "an unread folder still opens"
        );
        assert_eq!(
            bench.take_reads(Instant::now()),
            Vec::new(),
            "nothing until it is opened"
        );
        bench.files.tree.set_expanded("src", true);
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![Read::Directory("src".into())]
        );
        assert_eq!(bench.take_reads(Instant::now()), Vec::new(), "asked once");
        bench.apply_directory(
            scope,
            "src",
            Ok(listing("src", vec![entry("src/main.rs", FileKind::File)])),
        );
        let children: Vec<&str> = bench.files.tree.children(Some("src")).collect();
        assert_eq!(children, vec!["src/main.rs"], "the placeholder is gone");
    }

    #[test]
    fn an_answer_for_another_workspace_is_dropped() {
        let (mut bench, _) = bench();
        bench.apply_directory(
            Scope {
                workspace: WorkspaceId::new(),
                visit: 0,
            },
            "",
            Ok(listing("", vec![entry("a", FileKind::File)])),
        );
        assert!(bench.files.tree.is_empty());
    }

    #[test]
    fn the_changes_view_parses_every_patch_and_names_what_has_none() {
        let (mut bench, workspace) = bench();
        let scope = bench.scope().expect("a scope");
        bench.show(CenterMode::Changes);
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![Read::Diff, Read::Rebase]
        );
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let file = |path: &str, patch: &str| DiffFile {
            path: path.into(),
            status: DiffStatus::Modified,
            additions: 1,
            deletions: 1,
            patch: patch.into(),
            binary: patch.is_empty(),
            truncated: false,
        };
        bench.apply_diff(
            scope,
            Ok(WorkspaceDiff {
                workspace_id: workspace,
                branch: Some("main".into()),
                files: vec![file("a.rs", patch), file("logo.png", "")],
                truncated: false,
            }),
        );
        assert_eq!(bench.changes.files.len(), 2);
        assert_eq!(bench.changes.selected, Some(0));
        assert_eq!(
            bench.changes.state.files().len(),
            1,
            "one file's patch at a time"
        );
        bench.select_change(1);
        assert!(
            bench.changes.state.files().is_empty(),
            "a binary file has no patch"
        );
        assert_eq!(bench.changes.without_patch, vec!["logo.png".to_string()]);
        assert_eq!(bench.changes.file_count, 2);
    }

    #[test]
    fn a_preview_splits_lines_and_slices_highlights_per_line() {
        let preview = Preview::new(FileContents {
            workspace_id: WorkspaceId::new(),
            path: "a.rs".into(),
            text: "fn a() {}\nlet x = 1;\n".into(),
            revision: String::new(),
            language: "rust".into(),
            binary: false,
            too_large: false,
        });
        assert_eq!(preview.lines.len(), 2);
        assert_eq!(&preview.text[preview.lines[1].clone()], "let x = 1;");
        let second = preview.line_highlights(&preview.lines[1]);
        assert!(!second.is_empty());
        assert!(
            second
                .iter()
                .all(|(range, _)| range.end <= "let x = 1;".len())
        );
    }

    #[test]
    fn markdown_opens_rendered_and_a_dotfile_is_not_an_extension() {
        assert!(opens_rendered("docs/README.md"));
        assert!(
            opens_rendered("NOTES.MD"),
            "the extension is read case-blind"
        );
        assert!(opens_rendered("page.mdx"));
        assert!(!opens_rendered(".md"));
        assert!(!opens_rendered("a/.md"));
        assert!(!opens_rendered("src/main.rs"));
        assert!(!opens_rendered("Makefile"));
    }

    #[test]
    fn a_reply_to_an_earlier_visit_is_dropped_after_coming_back() {
        let (mut bench, a) = bench();
        let first = bench.scope().expect("a scope");
        bench.set_workspace(Some(WorkspaceId::new()));
        bench.set_workspace(Some(a));
        let second = bench.scope().expect("a scope");
        assert_ne!(first, second, "returning to A is a new visit");
        bench.apply_directory(
            first,
            "",
            Ok(listing("", vec![entry("stale", FileKind::File)])),
        );
        assert!(
            bench.files.tree.is_empty(),
            "the first visit's answer is dropped"
        );
        bench.apply_directory(
            second,
            "",
            Ok(listing("", vec![entry("fresh", FileKind::File)])),
        );
        let roots: Vec<&str> = bench.files.tree.children(None).collect();
        assert_eq!(roots, vec!["fresh"]);
    }

    #[test]
    fn a_checkout_moves_on_a_new_head_or_a_dirtiness_flip_only() {
        let mark = |head: Option<&str>, dirty| CheckoutMark {
            head: head.map(str::to_string),
            dirty,
        };
        let seen = mark(Some("abc123"), false);
        assert!(
            !checkout_moved(Some(&seen), Some(&mark(None, false))),
            "unmeasured"
        );
        assert!(!checkout_moved(Some(&seen), None), "no answer");
        assert!(!checkout_moved(None, Some(&seen)), "the first look");
        let dirty = mark(Some("abc123"), true);
        assert!(!checkout_moved(Some(&dirty), Some(&dirty)), "unchanged");
        assert!(checkout_moved(
            Some(&seen),
            Some(&mark(Some("def456"), false))
        ));
        assert!(checkout_moved(Some(&seen), Some(&dirty)));
    }

    #[test]
    fn a_moved_checkout_rereads_git_only_where_it_shows() {
        let (mut bench, _) = bench();
        bench.show_panel(Panel::Git);
        bench.take_reads(Instant::now());
        bench.checkout_moved();
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![Read::Diff, Read::Rebase]
        );
        bench.changes.loaded = true;
        bench.show_panel(Panel::Projects);
        bench.checkout_moved();
        assert_eq!(
            bench.take_reads(Instant::now()),
            Vec::new(),
            "nothing read for a hidden panel"
        );
        assert!(!bench.changes.loaded, "but the next look reads again");
    }

    /// A Files panel with the root and `src` listed and `src` open.
    fn listed() -> (Workbench, WorkspaceId, Scope) {
        let (mut bench, workspace) = bench();
        let scope = bench.scope().expect("a scope");
        bench.show(CenterMode::Files);
        bench.apply_directory(
            scope,
            "",
            Ok(listing(
                "",
                vec![
                    entry("src", FileKind::Directory),
                    entry("README.md", FileKind::File),
                ],
            )),
        );
        bench.files.tree.set_expanded("src", true);
        bench.apply_directory(
            scope,
            "src",
            Ok(listing(
                "src",
                vec![
                    entry("src/main.rs", FileKind::File),
                    entry("src/lib.rs", FileKind::File),
                ],
            )),
        );
        bench.take_reads(Instant::now());
        (bench, workspace, scope)
    }

    #[test]
    fn a_shown_preview_keeps_its_folder_watched_whatever_the_sidebar_shows() {
        let (mut bench, workspace) = bench();
        bench.preview("docs/guide/intro.md");
        bench.take_reads(Instant::now());
        let (to, set) = bench
            .take_watch(1)
            .expect("the preview's folder is watched");
        assert_eq!(to, workspace);
        assert_eq!(set.directories, vec!["docs/guide".to_string()]);
        bench.show_panel(Panel::Files);
        bench.take_reads(Instant::now());
        let (_, both) = bench.take_watch(1).expect("the tree joins the preview");
        assert_eq!(both.directories, vec![String::new(), "docs/guide".into()]);
        bench.show_panel(Panel::Projects);
        bench.take_reads(Instant::now());
        let (_, kept) = bench
            .take_watch(1)
            .expect("leaving Files keeps the preview");
        assert_eq!(kept.directories, vec!["docs/guide".to_string()]);
        bench.show(CenterMode::Session);
        bench.take_reads(Instant::now());
        let (_, cleared) = bench
            .take_watch(1)
            .expect("a closed preview stops watching");
        assert!(cleared.directories.is_empty());
    }

    #[test]
    fn the_watch_follows_the_open_folders_and_is_asked_again_on_a_new_connection() {
        let (mut bench, workspace) = bench();
        let scope = bench.scope().expect("a scope");
        assert_eq!(bench.take_watch(1), None, "nothing to watch, nothing sent");
        bench.show(CenterMode::Files);
        bench.take_reads(Instant::now());
        let (to, first) = bench.take_watch(1).expect("the root is watched");
        assert_eq!(to, workspace);
        assert_eq!(first.directories, vec![String::new()]);
        assert_eq!(bench.take_watch(1), None, "unchanged");
        bench.apply_watch(first.ticket, Ok(()));
        assert_eq!(
            bench.take_reads(Instant::now()),
            Vec::new(),
            "nothing was listed before the first watch"
        );
        bench.apply_directory(
            scope,
            "",
            Ok(listing("", vec![entry("src", FileKind::Directory)])),
        );
        bench.files.tree.set_expanded("src", true);
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![Read::Directory("src".into())]
        );
        let (_, opened) = bench.take_watch(1).expect("src is watched");
        assert_eq!(opened.directories, vec![String::new(), "src".into()]);
        bench.show_panel(Panel::Projects);
        bench.take_reads(Instant::now());
        let (_, cleared) = bench.take_watch(1).expect("a hidden tree stops watching");
        assert!(cleared.directories.is_empty());
        assert_eq!(bench.take_watch(1), None);
        bench.show_panel(Panel::Files);
        bench.take_reads(Instant::now());
        let (_, again) = bench.take_watch(2).expect("a new connection watches anew");
        bench.apply_watch(opened.ticket, Ok(()));
        assert_eq!(
            bench.take_reads(Instant::now()),
            Vec::new(),
            "an answer for a replaced set is dropped"
        );
        bench.apply_watch(again.ticket, Ok(()));
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![Read::Directory(String::new())],
            "what was listed while unwatched is listed again"
        );
        bench.apply_watch(again.ticket, Err("refused".into()));
        assert!(
            bench
                .files
                .watch_notice
                .as_deref()
                .is_some_and(|notice| notice.contains("refused"))
        );
    }

    #[test]
    fn the_watch_is_bounded_like_the_daemon() {
        let (mut bench, _) = bench();
        bench.show(CenterMode::Files);
        bench.files.open = (0..MAX_WATCHED + 5).map(|i| format!("d{i}")).collect();
        bench.files.open.push("x".repeat(MAX_WATCH_PATH_BYTES + 1));
        let (_, set) = bench.take_watch(1).expect("a set");
        assert_eq!(set.directories.len(), MAX_WATCHED);
        assert!(bench.files.watch_notice.is_some());
    }

    #[test]
    fn a_changed_path_relists_its_folder_and_an_empty_one_relists_all() {
        let (mut bench, workspace, _) = listed();
        let now = Instant::now();
        bench.file_changed(WorkspaceId::new(), "src/main.rs", now);
        assert_eq!(bench.take_reads(now), Vec::new(), "another checkout");
        bench.file_changed(workspace, "src/new.rs", now);
        assert_eq!(bench.take_reads(now), vec![Read::Directory("src".into())]);
        bench.file_changed(workspace, "src", now);
        assert_eq!(
            bench.take_reads(now),
            vec![
                Read::Directory(String::new()),
                Read::Directory("src".into())
            ],
            "a folder is listed in its parent and lists itself"
        );
        bench.file_changed(workspace, "", now);
        assert_eq!(
            bench.take_reads(now),
            vec![
                Read::Directory(String::new()),
                Read::Directory("src".into())
            ]
        );
    }

    #[test]
    fn watched_changes_reread_the_diff_after_the_floor() {
        let (mut bench, workspace, _) = listed();
        let start = Instant::now();
        bench.changes.last_read = Some(start);
        let diff = |reads: Vec<Read>| reads.contains(&Read::Diff);
        bench.file_changed(workspace, "README.md", start);
        bench.file_changed(workspace, "src/main.rs", start);
        assert!(!diff(bench.take_reads(start + DIFF_COALESCE)));
        assert!(
            diff(bench.take_reads(start + DIFF_WATCH_FLOOR)),
            "one read for the burst, once the floor passed"
        );
        assert!(!diff(bench.take_reads(start + DIFF_WATCH_FLOOR * 2)));
        let later = start + DIFF_WATCH_FLOOR * 3;
        bench.file_changed(workspace, "README.md", later);
        assert!(!diff(bench.take_reads(later)), "coalesced");
        assert!(diff(bench.take_reads(later + DIFF_COALESCE)));
    }

    #[test]
    fn the_filter_shows_matching_rows_and_survives_a_relisting() {
        let (mut bench, _, scope) = listed();
        bench.files.filter = rune::TextEdit::new("MAIN");
        bench.filter_changed();
        let shown = |bench: &Workbench| -> Vec<String> {
            (0..bench.files.tree.visible_len())
                .filter_map(|row| bench.files.tree.visible_id(row).map(str::to_string))
                .collect()
        };
        assert_eq!(shown(&bench), vec!["src", "src/main.rs"]);
        bench.apply_directory(
            scope,
            "src",
            Ok(listing(
                "src",
                vec![
                    entry("src/main.rs", FileKind::File),
                    entry("src/domain.rs", FileKind::File),
                ],
            )),
        );
        assert_eq!(shown(&bench), vec!["src", "src/domain.rs", "src/main.rs"]);
        bench.files.filter = rune::TextEdit::new("");
        bench.filter_changed();
        assert_eq!(shown(&bench).len(), 4);
    }

    #[test]
    fn a_folder_that_vanished_leaves_the_tree() {
        let (mut bench, _, scope) = listed();
        bench.apply_directory(scope, "src", Err("gone".into()));
        assert!(
            bench.files.error.is_none(),
            "only the root's failure is said"
        );
        assert!(
            bench.files.tree.has_children("src"),
            "it waits to be listed again"
        );
        assert!(!bench.files.tree.contains("src/main.rs"));
    }

    #[test]
    fn previews_route_by_extension_and_late_answers_drop() {
        assert_eq!(preview_kind("a/b.CSV"), Some(PreviewKind::Csv));
        assert_eq!(preview_kind("data.tsv"), Some(PreviewKind::Csv));
        assert_eq!(preview_kind("logo.svg"), Some(PreviewKind::Svg));
        assert_eq!(preview_kind("shot.PNG"), Some(PreviewKind::Image));
        assert_eq!(preview_kind(".png"), None);
        assert_eq!(preview_kind("main.rs"), None);
        assert!(!PreviewKind::Image.has_source());
        let (mut bench, workspace) = bench();
        let scope = bench.scope().expect("a scope");
        bench.preview("logo.svg");
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![
                Read::File("logo.svg".into()),
                Read::Image("logo.svg".into())
            ],
            "an SVG is a picture with a source"
        );
        bench.preview("shot.png");
        assert_eq!(
            bench.take_reads(Instant::now()),
            vec![Read::Image("shot.png".into())]
        );
        bench.apply_file(
            scope,
            Ok(FileContents {
                workspace_id: workspace,
                path: "logo.svg".into(),
                text: "<svg/>".into(),
                revision: String::new(),
                language: String::new(),
                binary: false,
                too_large: false,
            }),
        );
        assert!(bench.files.preview.is_none(), "an earlier preview's answer");
        bench.apply_image(scope, "shot.png", Err("bad".into()));
        assert!(
            bench
                .files
                .preview
                .as_ref()
                .is_some_and(|preview| preview.kind == Some(PreviewKind::Image))
        );
    }
}
