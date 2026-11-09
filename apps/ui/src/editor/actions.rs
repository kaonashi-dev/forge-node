//! What the editor's chrome asks of the app: its own chords, the find panel,
//! a refused save, the editor preferences, and the Code strip's steps.

use domain::{
    EditorFindCommand, EditorInputEvent, EditorKey, SessionId, SessionKind, editor_modifiers,
};
use rune::{KeyEvent, Tx};

use super::{SCROLL_KEY, conflict, find, tabs};
use crate::app::AppState;
use crate::keymap::{self, Action};
use crate::settings::prefs;
use crate::workbench::{CenterMode, Panel};

/// The editor's own keys for the two navigation actions: F12, and Alt-O
/// rather than Alt-D, which Option turns into `∂` on macOS.
const DEFINITION: EditorInputEvent = EditorInputEvent::Key {
    key: EditorKey::Function(12),
    modifiers: 0,
};
const BACK: EditorInputEvent = EditorInputEvent::Key {
    key: EditorKey::Char('o'),
    modifiers: editor_modifiers::ALT,
};
const COPY: EditorInputEvent = EditorInputEvent::Key {
    key: EditorKey::Char('c'),
    modifiers: editor_modifiers::CONTROL,
};

/// Every element of the editor pane is keyed under this prefix, so a focused
/// key says whether the keyboard is inside it.
pub const KEY_PREFIX: &str = "editor-";

impl AppState {
    /// The editor context's chords, asked before the app's: a line editor is
    /// shown and the keyboard is in its pane, or nowhere in particular.
    pub fn editor_chord(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let inside = tx
            .focused()
            .is_none_or(|focused| focused.starts_with(KEY_PREFIX));
        if !inside || self.active_line_editor().is_none() || !self.session_shown() {
            return false;
        }
        let Some(action) = keymap::editor_action_for(ev) else {
            return false;
        };
        let handled = self.editor_action(action);
        if let Some(offset) = self.editor_reveal.take() {
            tx.scroll_to_instant(SCROLL_KEY, 0.0, offset);
        }
        handled
    }

    pub fn editor_action(&mut self, action: Action) -> bool {
        match action {
            Action::GoToDefinition => self.send_editor_input(DEFINITION),
            Action::GoBack => self.send_editor_input(BACK),
            Action::EditorZoomIn => self.zoom_editor(Some(true)),
            Action::EditorZoomOut => self.zoom_editor(Some(false)),
            Action::EditorZoomReset => self.zoom_editor(None),
            _ => return false,
        }
        true
    }

    fn send_editor_input(&mut self, event: EditorInputEvent) {
        if self.active_line_editor().is_none() {
            return;
        }
        self.editor.queue(event);
        let out = self.editor.take_requests();
        if let (Some(connection), Some((session, events))) = (&self.connection, out.input) {
            let _ = connection.editor_input(self.generation, session, events);
        }
        if let (Some(connection), Some((session, first, count))) = (&self.connection, out.view) {
            let _ = connection.editor_view(self.generation, session, first, count);
        }
    }

    /// One step of `ui.editor.font_size`, or back to its default; a no-op at
    /// either end, so a held chord does not keep writing the same bound.
    fn zoom_editor(&mut self, up: Option<bool>) {
        let range = prefs::EDITOR_FONT_SIZE;
        let current = prefs::read_scale(self.pref(prefs::EDITOR_FONT_SIZE_KEY), range);
        let next = match up {
            Some(up) => prefs::step_value(current, up, range),
            None => range.fallback,
        };
        if next != current {
            self.write_pref(prefs::EDITOR_FONT_SIZE_KEY, &prefs::number_value(next));
        }
    }

    /// Takes the editor preferences from the replica: the type, and autosave
    /// for every live editor. Called when the replica or a preference changes,
    /// never per frame.
    pub fn apply_editor_prefs(&mut self) {
        let font = prefs::read_scale(
            self.pref(prefs::EDITOR_FONT_SIZE_KEY),
            prefs::EDITOR_FONT_SIZE,
        );
        let line_height = prefs::read_scale(
            self.pref(prefs::EDITOR_LINE_HEIGHT_KEY),
            prefs::EDITOR_LINE_HEIGHT,
        );
        if let Some(offset) = self.editor.set_type(font as f32, line_height as f32) {
            self.editor_reveal = Some(offset);
        }
        let (Some(connection), Some(store)) = (&self.connection, &self.store) else {
            return;
        };
        let enabled = prefs::read_flag(store.app_state_value(prefs::AUTOSAVE_KEY), false);
        let live: Vec<SessionId> = store
            .sessions
            .iter()
            .filter(|session| session.kind == SessionKind::Editor && session.state.is_active())
            .map(|session| session.id)
            .collect();
        for session in self.editor.autosave.due(&live, enabled) {
            let _ = connection.set_editor_autosave(session, enabled);
        }
    }

    /// The replica changed: the find field follows its panel, a conflict that
    /// cleared drops what was fetched for it, and the preferences apply.
    pub fn editor_store_changed(&mut self) {
        self.follow_find();
        if let Some(store) = &self.store {
            self.editor.conflicts.retain(|id, _| {
                store.sessions.iter().any(|session| {
                    session.id == *id && session.editor.as_ref().is_some_and(|state| state.conflict)
                })
            });
        }
        self.apply_editor_prefs();
    }

    pub fn follow_find(&mut self) {
        let session = self.active_line_editor();
        let find = self
            .active_editor_state()
            .and_then(|state| state.find.as_deref())
            .cloned();
        self.editor.find.follow(session, find.as_ref());
    }

    /// A find-panel gesture; the answer arrives as the session's `find`.
    pub fn send_editor_find(&mut self, command: EditorFindCommand) {
        if let EditorFindCommand::Set { pattern, .. } = &command
            && find::too_long(pattern)
        {
            return;
        }
        if let (Some(connection), Some(session)) = (&self.connection, self.active_line_editor()) {
            let _ = connection.editor_find(session, command);
        }
    }

    pub fn find_typed(&mut self) {
        let Some(open) = self
            .active_editor_state()
            .and_then(|state| state.find.as_deref())
            .cloned()
        else {
            return;
        };
        let command = find::set(&open, &self.editor.find.edit.text, None);
        self.send_editor_find(command);
    }

    pub fn find_toggle(&mut self, flag: find::Flag) {
        let Some(open) = self
            .active_editor_state()
            .and_then(|state| state.find.as_deref())
            .cloned()
        else {
            return;
        };
        let command = find::set(&open, &self.editor.find.edit.text, Some(flag));
        self.send_editor_find(command);
    }

    /// The field's own keys, then the editor's chords: the panel is inside
    /// the editor's context, so ⌘= still sizes the text from there.
    pub fn find_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(command) = find::key_command(ev) else {
            let Some(action) = keymap::editor_action_for(ev) else {
                return false;
            };
            let handled = self.editor_action(action);
            if let Some(offset) = self.editor_reveal.take() {
                tx.scroll_to_instant(SCROLL_KEY, 0.0, offset);
            }
            return handled;
        };
        if command == EditorFindCommand::Close {
            self.editor.find.edit.focused = false;
        }
        self.send_editor_find(command);
        true
    }

    /// Keep mine: write the draft over what is on disk now.
    pub fn keep_mine(&mut self, session: SessionId) {
        if let Some(connection) = &self.connection {
            let _ = connection.overwrite_editor_buffer(session);
        }
    }

    /// Take disk: replace the buffer with what is on disk now.
    pub fn take_disk(&mut self, session: SessionId) {
        if let Some(connection) = &self.connection {
            let _ = connection.reload_editor_buffer(session);
        }
    }

    /// Shows or hides the comparison, reading both sides again on each show.
    pub fn toggle_compare(&mut self, session: SessionId) {
        let entry = self.editor.conflicts.entry(session).or_default();
        entry.comparing = !entry.comparing;
        if !entry.comparing {
            return;
        }
        entry.loading = true;
        entry.error = None;
        let sent = match &self.connection {
            Some(connection) => connection.editor_conflict(self.generation, session),
            None => Err("client worker is still starting".to_string()),
        };
        if let Err(error) = sent {
            entry.loading = false;
            entry.error = Some(error);
        }
    }

    pub fn apply_editor_conflict(
        &mut self,
        session: SessionId,
        result: Result<Box<(String, String)>, String>,
    ) {
        let Some(entry) = self.editor.conflicts.get_mut(&session) else {
            return;
        };
        entry.loading = false;
        match result {
            Ok(sides) => {
                entry.rows = Some(conflict::line_diff(&sides.0, &sides.1).into());
                entry.error = None;
            }
            Err(error) => {
                entry.rows = None;
                entry.error = Some(error);
            }
        }
    }

    /// A definition jump landed in `session`: show its tab, as a file this
    /// window opened itself would be shown.
    pub fn editor_focus_requested(&mut self, session: SessionId) {
        self.workbench.show(CenterMode::Session);
        if self.rows.iter().any(|row| row.id == session) {
            self.select(session);
        } else {
            self.select_when_listed = Some(session);
        }
    }

    /// Remembers an editor's file as it closes, for Reopen Closed Tab. A
    /// read-only editor is skipped: it may hold a place outside the checkout,
    /// which only a definition jump can open.
    pub fn remember_closed_editor(&mut self, session: SessionId) {
        let Some(found) = self.store.as_ref().and_then(|store| {
            store
                .sessions
                .iter()
                .find(|candidate| candidate.id == session)
        }) else {
            return;
        };
        if let Some(state) = found.editor.as_ref().filter(|state| !state.read_only) {
            let workspace = found.workspace_id;
            let path = state.path.clone();
            self.editor.closed.remember(workspace, &path);
        }
    }

    /// Brings back the last file closed in this window's checkout.
    pub fn reopen_closed_editor(&mut self) {
        self.sync_workbench_workspace();
        let Some(workspace) = self.workbench.workspace else {
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        if let Some(path) = self.editor.closed.take(workspace)
            && connection.open_editor(workspace, path).is_ok()
        {
            self.workbench.show(CenterMode::Session);
        }
    }

    /// One step along the strip's editors, wrapping.
    pub fn step_code_view(&mut self, delta: isize) {
        let editors: Vec<SessionId> = self
            .tab_rows()
            .into_iter()
            .filter(|row| row.kind == SessionKind::Editor)
            .map(|row| row.id)
            .collect();
        if let Some(id) = tabs::step(&editors, self.active, delta) {
            self.workbench.show(CenterMode::Session);
            self.select(id);
        }
    }

    /// The pane menu's Copy: the editor owns the selection, so it is asked to
    /// copy it, and the clipboard is written from what it sends back.
    pub fn editor_copy(&mut self, session: SessionId) {
        if self.active_line_editor() == Some(session) {
            self.send_editor_input(COPY);
        }
    }

    pub fn editor_paste(&mut self, session: SessionId, tx: &mut Tx) {
        if self.active_line_editor() != Some(session) {
            return;
        }
        tx.read_clipboard(move |s: &mut AppState, content, tx| {
            if s.active_line_editor() == Some(session) && !content.text().is_empty() {
                s.paste(content.text(), tx);
            }
        });
    }

    /// Shows `path` in the Files tree, opening each folder above it as its
    /// listing arrives.
    pub fn reveal_in_files(&mut self, path: String, tx: &mut Tx) {
        self.show_panel(Panel::Files);
        self.editor.reveal = Some(path);
        self.advance_reveal(tx);
    }

    /// One step of a reveal: every listed folder above the path opens, which
    /// asks for the next listing, until the path itself is in the tree.
    pub fn advance_reveal(&mut self, tx: &mut Tx) {
        let Some(path) = self.editor.reveal.clone() else {
            return;
        };
        if self.workbench.panel != Panel::Files {
            self.editor.reveal = None;
            return;
        }
        let tree = &mut self.workbench.files.tree;
        if tree.reveal(&path, tx) {
            self.editor.reveal = None;
            return;
        }
        for (slash, _) in path.match_indices('/') {
            let folder = &path[..slash];
            if !tree.contains(folder) {
                break;
            }
            tree.set_expanded(folder, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::{EditorFind, EditorState};
    use flume::Receiver;
    use rune::Key;

    use super::*;
    use crate::connection::{Command, ConnectionHandle, Metrics, fixture_store};
    use crate::terminal::SharedTerminal;

    /// The fixture with its first session turned into a line editor on
    /// `src/lib.rs`, shown, and a connection whose requests can be read.
    fn editor_app(state: EditorState) -> (AppState, Receiver<Command>, SessionId) {
        let mut store = fixture_store(&crate::bench::fixture_path()).expect("fixture decodes");
        let session = &mut store.sessions[0];
        session.kind = SessionKind::Editor;
        session.editor = Some(state);
        let id = session.id;
        let mut s = AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        s.editor_surface = protocol::EditorSurface::Dom;
        s.select(id);
        s.editor.set_session(Some(id));
        let (handle, rx) = ConnectionHandle::test_handle(Arc::new(Metrics::default()));
        s.connection = Some(handle);
        (s, rx, id)
    }

    fn file() -> EditorState {
        EditorState {
            path: "src/lib.rs".into(),
            ..EditorState::default()
        }
    }

    fn cmd(ch: char) -> KeyEvent {
        KeyEvent {
            key: Key::Command(ch),
            mods: rune::Mods {
                cmd: true,
                ..rune::Mods::default()
            },
        }
    }

    fn sent(rx: &Receiver<Command>) -> Vec<Command> {
        rx.try_iter().collect()
    }

    #[test]
    fn command_b_is_go_to_definition_in_the_editor_and_the_sidebar_elsewhere() {
        let (mut s, rx, id) = editor_app(file());
        let open = s.sidebar.is_open();
        assert!(s.editor_key(&cmd('b'), &mut Tx::default()));
        assert_eq!(s.sidebar.is_open(), open, "the sidebar did not move");
        assert!(sent(&rx).iter().any(|command| matches!(
            command,
            Command::EditorInput { session_id, events, .. }
                if *session_id == id && events == &[DEFINITION]
        )));
        s.workbench.show(CenterMode::Changes);
        assert!(s.editor_key(&cmd('b'), &mut Tx::default()));
        assert_ne!(s.sidebar.is_open(), open, "under Changes ⌘B is the app's");
    }

    #[test]
    fn control_minus_goes_back_and_command_minus_zooms_the_editor() {
        let (mut s, rx, _) = editor_app(file());
        let back = KeyEvent {
            key: Key::Text("-".into()),
            mods: rune::Mods {
                ctrl: true,
                ..rune::Mods::default()
            },
        };
        assert!(s.editor_key(&back, &mut Tx::default()));
        assert!(sent(&rx).iter().any(|command| matches!(
            command,
            Command::EditorInput { events, .. } if events == &[BACK]
        )));
        assert!(s.editor_key(&cmd('-'), &mut Tx::default()));
        assert_eq!(s.pref(prefs::EDITOR_FONT_SIZE_KEY), Some("12"));
        assert_eq!(s.editor.font_size, 12.0);
        assert!(sent(&rx).iter().any(|command| matches!(
            command,
            Command::SetAppState { key, value } if key == prefs::EDITOR_FONT_SIZE_KEY && value == "12"
        )));
        assert!(s.editor_key(&cmd('0'), &mut Tx::default()));
        assert_eq!(s.editor.font_size, 13.0);
    }

    #[test]
    fn line_spacing_and_autosave_reach_the_editor_when_they_change() {
        let (mut s, rx, id) = editor_app(file());
        s.write_pref(prefs::EDITOR_LINE_HEIGHT_KEY, "1.8");
        assert_eq!(s.editor.row_h, (13.0f32 * 1.8).round());
        s.write_pref(prefs::AUTOSAVE_KEY, "true");
        let told: Vec<(SessionId, bool)> = sent(&rx)
            .into_iter()
            .filter_map(|command| match command {
                Command::SetEditorAutosave {
                    session_id,
                    autosave,
                } => Some((session_id, autosave)),
                _ => None,
            })
            .collect();
        assert_eq!(told.last(), Some(&(id, true)));
        s.write_pref(prefs::AUTOSAVE_KEY, "true");
        assert!(
            !sent(&rx)
                .iter()
                .any(|command| matches!(command, Command::SetEditorAutosave { .. })),
            "told once per change"
        );
    }

    #[test]
    fn the_find_field_sends_what_is_typed_with_the_panels_flags() {
        let (mut s, rx, id) = editor_app(EditorState {
            find: Some(Box::new(EditorFind {
                focus: 1,
                pattern: "old".into(),
                regex: true,
                ..EditorFind::default()
            })),
            ..file()
        });
        s.follow_find();
        assert!(s.editor.find.edit.focused);
        s.editor.find.edit.text = "new".into();
        s.find_typed();
        s.find_toggle(find::Flag::Case);
        let enter = KeyEvent {
            key: Key::Enter,
            mods: rune::Mods::default(),
        };
        assert!(s.find_key(&enter, &mut Tx::default()));
        let finds: Vec<EditorFindCommand> = sent(&rx)
            .into_iter()
            .filter_map(|command| match command {
                Command::EditorFind {
                    session_id,
                    command,
                } if session_id == id => Some(command),
                _ => None,
            })
            .collect();
        assert_eq!(
            finds,
            vec![
                EditorFindCommand::Set {
                    pattern: "new".into(),
                    case_sensitive: false,
                    whole_word: false,
                    regex: true
                },
                EditorFindCommand::Set {
                    pattern: "new".into(),
                    case_sensitive: true,
                    whole_word: false,
                    regex: true
                },
                EditorFindCommand::Next,
            ]
        );
    }

    #[test]
    fn compare_fetches_both_sides_and_a_cleared_conflict_drops_them() {
        let (mut s, rx, id) = editor_app(EditorState {
            conflict: true,
            ..file()
        });
        s.toggle_compare(id);
        assert!(sent(&rx).iter().any(|command| matches!(
            command,
            Command::EditorConflict { session_id, .. } if *session_id == id
        )));
        s.apply_editor_conflict(id, Ok(Box::new(("a\nb\n".into(), "a\nc\n".into()))));
        let held = s.editor.conflicts.get(&id).expect("held");
        assert_eq!(held.rows.as_ref().map(|rows| rows.len()), Some(3));
        s.editor_store_changed();
        assert!(
            s.editor.conflicts.contains_key(&id),
            "kept while the flag stands"
        );
        if let Some(session) = s
            .store
            .as_mut()
            .and_then(|store| store.sessions.iter_mut().find(|session| session.id == id))
        {
            session.editor = Some(file());
        }
        s.editor_store_changed();
        assert!(s.editor.conflicts.is_empty());
    }

    #[test]
    fn a_closed_editor_reopens_in_its_checkout() {
        let (mut s, rx, id) = editor_app(file());
        s.close_now(id);
        let _ = sent(&rx);
        s.reopen_closed_editor();
        assert!(sent(&rx).iter().any(|command| matches!(
            command,
            Command::OpenEditor { path, .. } if path == "src/lib.rs"
        )));
        s.reopen_closed_editor();
        assert!(sent(&rx).is_empty(), "each close reopens once");
    }

    #[test]
    fn a_definition_jump_into_another_editor_shows_its_tab() {
        let (mut s, _rx, id) = editor_app(file());
        let other = s
            .rows
            .iter()
            .find(|row| row.id != id)
            .expect("a second row")
            .id;
        s.workbench.show(CenterMode::Changes);
        s.editor_focus_requested(other);
        assert_eq!(s.active, Some(other));
        assert_eq!(s.workbench.mode, CenterMode::Session);
    }
}
