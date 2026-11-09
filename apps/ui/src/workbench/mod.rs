//! Read-only views of a workspace beside its sessions: its uncommitted changes
//! and its files. Every read is answered by the daemon on the client worker
//! (`GetWorkspaceDiff`, `ListDirectory`, `ReadFile`); this side only keeps the
//! answers and never touches the checkout (ADR-012).

pub mod view;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use domain::{DirectoryListing, FileContents, FileEntry, FileKind, WorkspaceDiff, WorkspaceId};
use rune_kit::diff::DiffState;
use rune_kit::syntax::Highlight;
use rune_kit::{FileNode, TreeModel, natural_cmp};

use crate::file_icons::FileIcon;

/// What the centre shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CenterMode {
    #[default]
    Session,
    Projects,
    Changes,
    Files,
    Settings,
}

/// Whether a path opens rendered rather than in the editor, as Tauri's
/// `previewKindFor` decides for Markdown: by extension, and a dotfile's name
/// is not an extension.
#[must_use]
pub fn opens_rendered(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => {
            let extension = name[dot + 1..].to_ascii_lowercase();
            matches!(extension.as_str(), "md" | "markdown" | "mdx")
        }
        _ => false,
    }
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
/// tree is dirty, as Tauri's `checkoutWatch` remembers it.
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

/// What the sidebar shows, as Tauri's sidebar views. The centre keeps the
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
    /// Paths ranked against a palette query.
    Search(String),
    /// Whether a rebase, merge or cherry-pick is stopped, read from git.
    Rebase,
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

#[derive(Default)]
pub struct Workbench {
    pub mode: CenterMode,
    pub panel: Panel,
    visit: u64,
    pub workspace: Option<WorkspaceId>,
    pub changes: Changes,
    pub files: Files,
    reads: Vec<Read>,
}

/// One changed path as git reported it, with its patch already parsed.
pub struct ChangedFile {
    pub path: String,
    pub status: domain::DiffStatus,
    pub additions: u32,
    pub deletions: u32,
    diffs: Vec<rune_kit::diff::FileDiff>,
}

#[derive(Default)]
pub struct Changes {
    pub state: DiffState,
    pub files: Vec<ChangedFile>,
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
}

pub struct Files {
    pub tree: TreeModel<FileNode<FileIcon>>,
    pub preview: Option<Preview>,
    pub error: Option<String>,
    /// Each folder's entries as listed, folders first; `""` is the root.
    listings: HashMap<String, Vec<FileEntry>>,
    requested: HashSet<String>,
    root_requested: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            tree: TreeModel::new().key("files-tree"),
            preview: None,
            error: None,
            listings: HashMap::new(),
            requested: HashSet::new(),
            root_requested: false,
        }
    }
}

pub struct Preview {
    pub path: String,
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
    pub fn set_workspace(&mut self, workspace: Option<WorkspaceId>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        self.visit += 1;
        self.changes = Changes::default();
        self.files = Files::default();
        self.reads.clear();
    }

    #[must_use]
    pub fn scope(&self) -> Option<Scope> {
        self.workspace.map(|workspace| Scope {
            workspace,
            visit: self.visit,
        })
    }

    /// Projects, Files and Pull Requests are sidebar panels and Git is both:
    /// asking for one shows its panel, and only Changes and Settings (or the
    /// session) take the centre.
    pub fn show(&mut self, mode: CenterMode) {
        match mode {
            CenterMode::Projects => self.show_panel(Panel::Projects),
            CenterMode::Files => self.show_panel(Panel::Files),
            CenterMode::Changes => {
                self.show_panel(Panel::Git);
                self.mode = CenterMode::Changes;
            }
            CenterMode::Session | CenterMode::Settings => self.mode = mode,
        }
    }

    /// Shows `path` rendered in the centre.
    pub fn preview(&mut self, path: &str) {
        self.files.preview = None;
        self.files.error = None;
        self.queue(Read::File(path.to_string()));
        self.mode = CenterMode::Files;
    }

    /// Git moved under the checkout: re-read what Git shows now, or forget
    /// it so the next look reads again. The file tree is left as opened.
    pub fn checkout_moved(&mut self) {
        if self.panel == Panel::Git || self.mode == CenterMode::Changes {
            self.queue(Read::Diff);
            self.queue(Read::Rebase);
        } else {
            self.changes.loaded = false;
        }
    }

    pub fn show_panel(&mut self, panel: Panel) {
        self.panel = panel;
        if panel == Panel::Git && !self.changes.loaded {
            self.queue(Read::Diff);
            self.queue(Read::Rebase);
        }
    }

    pub fn refresh(&mut self) {
        if self.panel == Panel::Git || self.mode == CenterMode::Changes {
            self.queue(Read::Diff);
            self.queue(Read::Rebase);
        }
        match self.panel {
            Panel::Files => {
                let open = self
                    .files
                    .preview
                    .as_ref()
                    .map(|preview| preview.path.clone());
                self.files = Files::default();
                if let Some(path) = open {
                    self.queue(Read::File(path));
                }
            }
            Panel::Projects
            | Panel::History
            | Panel::PullRequests
            | Panel::Git
            | Panel::Processes => {}
        }
    }

    fn queue(&mut self, read: Read) {
        if !self.reads.contains(&read) {
            self.reads.push(read);
        }
    }

    /// The reads to send now: what was asked for, plus the listing of every
    /// folder the person opened since the last call.
    pub fn take_reads(&mut self) -> Vec<Read> {
        if self.workspace.is_none() {
            self.reads.clear();
            return Vec::new();
        }
        if self.panel == Panel::Files {
            if !self.files.root_requested {
                self.files.root_requested = true;
                self.queue(Read::Directory(String::new()));
            }
            let opened: Vec<String> = self
                .files
                .expanded()
                .into_iter()
                .filter(|dir| {
                    !self.files.listings.contains_key(dir) && !self.files.requested.contains(dir)
                })
                .collect();
            for dir in opened {
                self.files.requested.insert(dir.clone());
                self.queue(Read::Directory(dir));
            }
        }
        std::mem::take(&mut self.reads)
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
    }

    pub fn apply_diff(&mut self, scope: Scope, diff: Result<WorkspaceDiff, String>) {
        if self.scope() != Some(scope) {
            return;
        }
        let changes = &mut self.changes;
        changes.loaded = true;
        match diff {
            Ok(diff) => {
                changes.error = None;
                changes.branch = diff.branch;
                changes.truncated = diff.truncated;
                changes.file_count = diff.files.len();
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
                        path: file.path,
                        status: file.status,
                        additions: file.additions,
                        deletions: file.deletions,
                    })
                    .collect();
                let again = kept.and_then(|path| changes.files.iter().position(|f| f.path == path));
                self.select_change(again.unwrap_or(0));
            }
            Err(error) => changes.error = Some(error),
        }
    }

    /// Shows one changed file's patch, as Tauri opens one file at a time.
    pub fn select_change(&mut self, index: usize) {
        let changes = &mut self.changes;
        match changes.files.get(index) {
            Some(file) => {
                changes.selected = Some(index);
                changes.state.set_files(file.diffs.clone());
            }
            None => {
                changes.selected = None;
                changes.state.set_files(Vec::new());
            }
        }
    }

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
            Err(error) => {
                files.error = Some(error);
                return;
            }
        };
        entries.sort_by(|a, b| {
            let file = |kind: &FileKind| *kind != FileKind::Directory;
            file(&a.kind)
                .cmp(&file(&b.kind))
                .then_with(|| natural_cmp(&a.path, &b.path))
        });
        files.listings.insert(path.to_string(), entries);
        files.rebuild();
    }

    pub fn apply_file(&mut self, scope: Scope, file: Result<FileContents, String>) {
        if self.scope() != Some(scope) {
            return;
        }
        match file {
            Ok(file) => {
                self.files.error = None;
                self.files.preview = Some(Preview::new(file));
            }
            Err(error) => self.files.error = Some(error),
        }
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

    /// Builds the tree again from the listings, keeping what was open and
    /// selected: the model cannot drop a folder's placeholder in place.
    fn rebuild(&mut self) {
        let open = self.expanded();
        let selected: Vec<String> = self.tree.selected().map(str::to_string).collect();
        let mut tree = TreeModel::new().key("files-tree");
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
    }
}

impl Preview {
    fn new(file: FileContents) -> Self {
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
        let language = file.language;
        let highlights = if file.binary || language == "markdown" {
            Rc::from(Vec::new())
        } else {
            rune_kit::syntax::highlight((!language.is_empty()).then_some(&language), &text)
        };
        Self {
            path: file.path,
            language,
            text,
            lines,
            cut,
            binary: file.binary,
            too_large: file.too_large,
            highlights,
        }
    }

    #[must_use]
    pub fn is_markdown(&self) -> bool {
        self.language == "markdown"
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
        assert_eq!(bench.take_reads(), vec![Read::Directory(String::new())]);
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
        assert_eq!(bench.take_reads(), Vec::new(), "nothing until it is opened");
        bench.files.tree.set_expanded("src", true);
        assert_eq!(bench.take_reads(), vec![Read::Directory("src".into())]);
        assert_eq!(bench.take_reads(), Vec::new(), "asked once");
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
        assert_eq!(bench.take_reads(), vec![Read::Diff, Read::Rebase]);
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
        bench.take_reads();
        bench.checkout_moved();
        assert_eq!(bench.take_reads(), vec![Read::Diff, Read::Rebase]);
        bench.changes.loaded = true;
        bench.show_panel(Panel::Projects);
        bench.checkout_moved();
        assert_eq!(
            bench.take_reads(),
            Vec::new(),
            "nothing read for a hidden panel"
        );
        assert!(!bench.changes.loaded, "but the next look reads again");
    }
}
