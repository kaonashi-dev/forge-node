//! The file workbench's side of `AppState`: opening a file where it belongs,
//! project search, the watch, the tree following the active editor, and the
//! recent-files list. `app.rs` only calls in.

use std::time::Instant;

use rune::{KeyEvent, Tx};

use super::preview_find::{self, FieldKey, Hits, PreviewFind};
use super::{CenterMode, Panel, preview_kind, recent};
use crate::app::AppState;

pub const PREVIEW_SCROLL_KEY: &str = "preview-scroll";

/// Points an SVG is rasterized to fit; the decoder caps the side anyway.
const SVG_PT: f32 = 512.0;
const SVG_SCALE: f32 = 2.0;

impl AppState {
    /// Opens `path` of the window's checkout where it belongs: a Markdown,
    /// CSV or image file rendered in the centre, anything else in an editor
    /// (at `line`, when given). A line into a rendered file is a caret
    /// request, so it goes to the editor.
    pub fn open_file(&mut self, path: &str, line: Option<u32>) {
        self.sync_workbench_workspace();
        let Some(workspace) = self.workbench.workspace else {
            return;
        };
        self.note_opened(workspace, path);
        let rendered = preview_kind(path);
        if line.is_none() && rendered.is_some() {
            self.workbench.preview(path);
            return;
        }
        if rendered == Some(super::PreviewKind::Image) {
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        let sent = match line {
            Some(line) => connection.open_editor_at(workspace, path.to_string(), line),
            None => connection.open_editor(workspace, path.to_string()),
        };
        if sent.is_ok() {
            self.workbench.show(CenterMode::Session);
        }
    }

    /// `path` rendered in the centre, as the file is on disk: unsaved edits
    /// live only in the editor process.
    pub fn preview_file(&mut self, path: &str) {
        self.sync_workbench_workspace();
        if self.workbench.workspace.is_some() && preview_kind(path).is_some() {
            self.workbench.preview(path);
        }
    }

    /// ⌘F over a preview that shows text: the find bar, or the keyboard back
    /// in it. False when the centre shows no such preview.
    pub fn open_preview_find(&mut self, tx: &mut Tx) -> bool {
        let files = &mut self.workbench.files;
        if self.workbench.mode != CenterMode::Files
            || !files
                .preview
                .as_ref()
                .is_some_and(|preview| preview.shown().searchable())
        {
            return false;
        }
        match &mut files.find {
            Some(find) => find.refocus(),
            None => files.find = Some(PreviewFind::new()),
        }
        self.workbench.refresh_find();
        self.reveal_preview_find(tx);
        true
    }

    /// ⌘F is unbound in the keymap because the editor takes it; over a
    /// preview no editor is shown, so the preview answers it.
    pub fn preview_find_chord(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.modal_open() || self.workbench.mode != CenterMode::Files {
            return false;
        }
        if ev.key == rune::Key::Escape && self.workbench.files.find.is_some() {
            self.close_preview_find();
            return true;
        }
        crate::editor::find::is_reopen(ev) && self.open_preview_find(tx)
    }

    pub fn preview_find_typed(&mut self, tx: &mut Tx) {
        if let Some(find) = &mut self.workbench.files.find {
            find.current = 0;
        }
        self.workbench.refresh_find();
        self.reveal_preview_find(tx);
    }

    pub fn step_preview_find(&mut self, delta: isize, tx: &mut Tx) {
        if let Some(find) = &mut self.workbench.files.find {
            find.step(delta);
        }
        self.reveal_preview_find(tx);
    }

    pub fn close_preview_find(&mut self) {
        self.workbench.files.find = None;
    }

    pub fn preview_find_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if crate::editor::find::is_reopen(ev) {
            return self.open_preview_find(tx);
        }
        match preview_find::field_key(ev) {
            Some(FieldKey::Step(delta)) => self.step_preview_find(delta, tx),
            Some(FieldKey::Close) => self.close_preview_find(),
            None => return false,
        }
        true
    }

    /// Centres the current match: a virtual source row is mounted first.
    pub fn reveal_preview_find(&self, tx: &mut Tx) {
        let Some(find) = &self.workbench.files.find else {
            return;
        };
        match &find.hits {
            Hits::Rendered(hits) if !hits.is_empty() => {
                tx.scroll_center_in(PREVIEW_SCROLL_KEY, preview_find::CURRENT_KEY);
            }
            Hits::Lines(_) => {
                if let Some(line) = find.current_line() {
                    tx.scroll_center_in(
                        PREVIEW_SCROLL_KEY,
                        format!("l{line}/{}", preview_find::CURRENT_KEY),
                    );
                }
            }
            _ => {}
        }
    }

    /// The source of the file the preview renders, in an editor.
    pub fn edit_previewed(&mut self) {
        let Some(path) = self.workbench.files.preview.as_ref().and_then(|preview| {
            preview
                .kind
                .is_some_and(super::PreviewKind::has_source)
                .then(|| preview.path.clone())
        }) else {
            return;
        };
        if let (Some(connection), Some(workspace)) = (&self.connection, self.workbench.workspace)
            && connection.open_editor(workspace, path).is_ok()
        {
            self.workbench.show(CenterMode::Session);
        }
    }

    /// ⇧⌘F: the centre's project search, its field focused.
    pub fn find_in_files(&mut self) {
        self.sync_workbench_workspace();
        self.workbench.show(CenterMode::Search);
    }

    fn note_opened(&mut self, workspace: domain::WorkspaceId, path: &str) {
        let Some(store) = &self.store else {
            return;
        };
        let current = recent::parse(store.app_state_value(recent::RECENT_FILES_KEY));
        if let Some(next) = recent::with_opened(current, &workspace.to_string(), path)
            && let Ok(value) = serde_json::to_string(&next)
        {
            self.write_pref(recent::RECENT_FILES_KEY, &value);
        }
    }

    /// What ⌘P lists before anything is typed: files opened here lately,
    /// then the checkout's uncommitted changes. Read when the palette opens.
    #[must_use]
    pub fn recent_files(&self) -> Vec<String> {
        let Some(workspace) = self.workbench.workspace else {
            return Vec::new();
        };
        let lists = recent::parse(
            self.store
                .as_ref()
                .and_then(|store| store.app_state_value(recent::RECENT_FILES_KEY)),
        );
        let opened = lists.get(&workspace.to_string());
        recent::initial_files(
            opened.into_iter().flatten().map(String::as_str),
            self.workbench
                .changes
                .files
                .iter()
                .filter(|file| file.status != domain::DiffStatus::Deleted)
                .map(|file| file.path.as_str()),
            recent::RECENT_FILE_LIMIT,
        )
    }

    /// Sends the watch, the project search and the panels' reads, in that
    /// order: a folder is watched before it is listed.
    pub(crate) fn send_file_reads(&mut self, now: Instant) {
        let reads = self.workbench.take_reads(now);
        let watch = self.workbench.take_watch(self.generation);
        let content = self.workbench.search.take(now, self.workbench.scope());
        let Some(connection) = &self.connection else {
            return;
        };
        if let Some((workspace, set)) = watch {
            let _ = connection.watch_files(self.generation, workspace, set);
        }
        let Some(scope) = self.workbench.scope() else {
            return;
        };
        if let Some(needle) = content {
            let _ = connection.read(self.generation, scope, super::Read::Content(needle));
        }
        for read in reads {
            let _ = connection.read(self.generation, scope, read);
        }
    }

    /// Hands an image's bytes to the decoder, which works on its own thread.
    pub(crate) fn apply_image(
        &mut self,
        scope: super::Scope,
        path: &str,
        bytes: Result<(String, std::sync::Arc<[u8]>), String>,
    ) {
        let image = bytes.and_then(|(mime, bytes)| match &self.images {
            Some(cache) => Ok(cache.load_mime(bytes, &mime, SVG_PT, SVG_SCALE)),
            None => Err("This window has no image decoder.".to_string()),
        });
        self.workbench.apply_image(scope, path, image);
    }

    /// Keeps the Files tree on the active editor's file, and says whether a
    /// preview image landed; both run on the poll.
    pub(crate) fn poll_files(&mut self, tx: &mut rune::Tx) -> bool {
        let active = self.active_editor_state().and_then(|editor| {
            let session = self.active?;
            let workspace = self
                .store
                .as_ref()?
                .sessions
                .iter()
                .find(|candidate| candidate.id == session)?
                .workspace_id;
            (Some(workspace) == self.workbench.workspace).then_some(editor.path.as_str())
        });
        if active != self.workbench.following() {
            let active = active.map(str::to_string);
            self.workbench.follow(active.as_deref());
        }
        let mut changed = self.workbench.image_landed();
        if self.workbench.panel == Panel::Files {
            changed |= self.workbench.reveal_followed(tx);
        }
        changed
    }
}
