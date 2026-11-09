//! The editor surface for a `dom` editor session: the daemon's headless editor
//! publishes windows of lines, and this side scrolls, paints and forwards input.
//!
//! The editor process is the only authority on text, caret and selection. Keys
//! are forwarded, never applied here, so the two can never disagree about what
//! a key did.

pub mod aria;
pub mod metrics;
pub mod theme;
pub mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use domain::{
    EditorFrame, EditorInputEvent, EditorKey, EditorPlace, MAX_EDITOR_INPUT_EVENTS, SessionId,
    editor_modifiers,
};
use rune::{Ime, Key, KeyEvent};

use metrics::MetricsCache;

pub const ROW_H: f32 = 20.0;
pub const GUTTER_W: f32 = 88.0;
/// The gutter's right edge holds the fold chevron; a press there toggles it.
pub const FOLD_HIT_W: f32 = 20.0;
pub const TEXT_PAD: f32 = 4.0;
pub const SCROLL_KEY: &str = "editor-scroll";
/// Rows mounted above and below the visible ones.
pub const OVERSCAN: u32 = 16;
/// The editor's own `MAX_VIEW_ROWS`; asking for more is clamped there anyway.
const MAX_WINDOW: u32 = 512;
/// Bounds what a stalled worker can make this side hold.
const MAX_PENDING: usize = 1024;
/// Longest composition drawn; an input method never needs more on one line.
const PREEDIT_MAX_CHARS: usize = 256;
/// The window's text as a screen reader's value, cut on a char boundary: a
/// mounted window is a few screens, and a reader is not a file viewer.
const A11Y_TEXT_MAX_BYTES: usize = 16 * 1024;

pub struct EditorUi {
    pub session: Option<SessionId>,
    pub frame: Option<EditorFrame>,
    /// `[offset, viewport height]` of the scroll container, written by its
    /// scroll callback, which has no access to the state.
    pub scroll: Rc<Cell<[f32; 2]>>,
    pub metrics: RefCell<MetricsCache>,
    pub row_builds: Cell<u64>,
    /// The input method's composition, drawn at the caret. Never sent: it is
    /// not an edit until the input method commits it.
    pub preedit: String,
    /// The window's rows as one string, rebuilt only when a row changes.
    pub a11y_text: Rc<str>,
    a11y_rows: Vec<(u32, u64)>,
    requested: Option<(SessionId, u32, u32)>,
    pending: Vec<EditorInputEvent>,
    last_caret: Option<EditorPlace>,
}

/// What [`EditorUi::take_requests`] wants sent.
#[derive(Debug, Default, PartialEq)]
pub struct Outgoing {
    pub view: Option<(SessionId, u32, u32)>,
    pub input: Option<(SessionId, Vec<EditorInputEvent>)>,
}

impl Default for EditorUi {
    fn default() -> Self {
        Self {
            session: None,
            frame: None,
            scroll: Rc::new(Cell::new([0.0, 0.0])),
            metrics: RefCell::new(MetricsCache::new()),
            row_builds: Cell::new(0),
            preedit: String::new(),
            a11y_text: Rc::from(""),
            a11y_rows: Vec::new(),
            requested: None,
            pending: Vec::new(),
            last_caret: None,
        }
    }
}

impl EditorUi {
    /// Points the surface at `session`, dropping everything about the previous one.
    /// Points the editor at `session`. Input queued for the one it leaves is
    /// handed back with that session, to be sent there rather than dropped,
    /// as Tauri's editor flushes its batch to the original surface.
    pub fn set_session(
        &mut self,
        session: Option<SessionId>,
    ) -> Option<(SessionId, Vec<EditorInputEvent>)> {
        if self.session == session {
            return None;
        }
        let left = self
            .session
            .filter(|_| !self.pending.is_empty())
            .map(|old| (old, std::mem::take(&mut self.pending)));
        self.session = session;
        self.frame = None;
        self.requested = None;
        self.preedit.clear();
        self.a11y_text = Rc::from("");
        self.a11y_rows.clear();
        self.last_caret = None;
        self.scroll.set([0.0, self.scroll.get()[1]]);
        left
    }

    /// Forgets the last window request, so the next poll asks again (after a
    /// reconnect, when the daemon's editor has no idea what is mounted).
    pub fn reset_requests(&mut self) {
        self.requested = None;
    }

    /// Takes a frame for the current session and returns a scroll offset that
    /// would bring a moved caret into view.
    ///
    /// A frame older than the one held is dropped: a coalesced input burst can
    /// put two frames on the socket out of the order they were built in.
    pub fn apply_frame(&mut self, session: SessionId, frame: EditorFrame) -> Option<f32> {
        if self.session != Some(session) {
            return None;
        }
        if let Some(held) = &self.frame
            && held.buffer_id == frame.buffer_id
            && frame.doc_version < held.doc_version
        {
            return None;
        }
        let moved = self.last_caret != Some(frame.caret);
        self.last_caret = Some(frame.caret);
        self.refresh_a11y_text(&frame);
        self.frame = Some(frame);
        if moved { self.reveal_caret() } else { None }
    }

    fn refresh_a11y_text(&mut self, frame: &EditorFrame) {
        let same = self.a11y_rows.len() == frame.rows.len()
            && self
                .a11y_rows
                .iter()
                .zip(frame.rows.iter())
                .all(|(held, row)| *held == (row.line, row.fingerprint) && row.fingerprint != 0);
        if same {
            return;
        }
        self.a11y_rows = frame
            .rows
            .iter()
            .map(|row| (row.line, row.fingerprint))
            .collect();
        let mut text = String::new();
        for row in frame.rows.iter() {
            for span in row.spans.iter() {
                text.push_str(&span.text);
            }
            text.push('\n');
            if text.len() >= A11Y_TEXT_MAX_BYTES {
                let mut end = A11Y_TEXT_MAX_BYTES;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                break;
            }
        }
        self.a11y_text = Rc::from(text);
    }

    /// The offset that shows the caret's row, or `None` when it is already
    /// visible or is not in the window.
    fn reveal_caret(&self) -> Option<f32> {
        let frame = self.frame.as_ref()?;
        let [offset, viewport] = self.scroll.get();
        if viewport <= 0.0 {
            return None;
        }
        let y = caret_row(frame)? as f32 * ROW_H;
        if y < offset {
            Some(y)
        } else if y + ROW_H > offset + viewport {
            Some(y + ROW_H - viewport)
        } else {
            None
        }
    }

    pub fn queue(&mut self, event: EditorInputEvent) {
        if self.pending.len() < MAX_PENDING {
            self.pending.push(event);
        }
    }

    /// The window to ask for, when the mounted one no longer covers the
    /// viewport, and the input queued since the last call.
    pub fn take_requests(&mut self) -> Outgoing {
        let Some(session) = self.session else {
            return Outgoing::default();
        };
        let mut out = Outgoing::default();
        if let Some((first, count)) = self.wanted_window() {
            let request = (session, first, count);
            if self.requested != Some(request) {
                self.requested = Some(request);
                out.view = Some(request);
            }
        }
        if !self.pending.is_empty() {
            let take = self.pending.len().min(MAX_EDITOR_INPUT_EVENTS);
            out.input = Some((session, self.pending.drain(..take).collect()));
        }
        out
    }

    fn wanted_window(&self) -> Option<(u32, u32)> {
        let [offset, viewport] = self.scroll.get();
        if viewport <= 0.0 {
            return None;
        }
        let visible = (viewport / ROW_H).ceil().max(1.0) as u32;
        let top = (offset.max(0.0) / ROW_H).floor() as u32;
        let count = (visible + 2 * OVERSCAN).min(MAX_WINDOW);
        let first_visual = top.saturating_sub(OVERSCAN);
        let Some(frame) = &self.frame else {
            return Some((first_visual, count));
        };
        let base = visual_base(frame);
        let end = base + frame.rows.len() as u32;
        let rows = content_rows(frame);
        let need_from = top.saturating_sub(OVERSCAN / 2);
        let need_to = (top + visible + OVERSCAN / 2).min(rows);
        let short = (frame.rows.len() as u32) < count.min(rows) && !frame.clipped;
        if need_from >= base && need_to <= end && !short {
            return None;
        }
        Some((document_line(frame, first_visual), count))
    }

    /// Takes an input method event: a composition is drawn, a commit is sent
    /// as committed text, the way a paste is.
    pub fn ime(&mut self, event: &Ime) {
        match event {
            Ime::Preedit(text, _) => {
                self.preedit = text.chars().take(PREEDIT_MAX_CHARS).collect();
            }
            Ime::Commit(text) => {
                self.preedit.clear();
                let clean: String = text.chars().filter(|ch| !ch.is_control()).collect();
                if !clean.is_empty() {
                    self.queue(EditorInputEvent::Text(clean));
                }
            }
            Ime::Enabled | Ime::Disabled => {}
        }
    }

    /// Turns a key into editor input; `false` leaves it for the app.
    pub fn key(&mut self, event: &KeyEvent) -> bool {
        let Some(event) = key_input(event) else {
            return false;
        };
        self.queue(event);
        true
    }
}

/// Where the window's first row sits among the rows the document takes.
#[must_use]
pub fn visual_base(frame: &EditorFrame) -> u32 {
    frame.first_line.saturating_sub(frame.hidden_above)
}

/// Rows the whole document takes once folds are closed.
#[must_use]
pub fn content_rows(frame: &EditorFrame) -> u32 {
    frame.total_lines.saturating_sub(frame.hidden_lines).max(1)
}

/// The caret's row among the rows the document takes, when it is in the window.
#[must_use]
pub fn caret_row(frame: &EditorFrame) -> Option<u32> {
    frame
        .rows
        .iter()
        .position(|row| row.line == frame.caret.line)
        .map(|index| visual_base(frame) + index as u32)
}

/// The document line shown at row `visual`. Exact inside the window; outside
/// it, assumes no fold between the row and the window, which a window request
/// may get slightly wrong and the next frame corrects.
#[must_use]
pub fn document_line(frame: &EditorFrame, visual: u32) -> u32 {
    let base = visual_base(frame);
    if visual < base {
        return frame.first_line.saturating_sub(base - visual);
    }
    let index = (visual - base) as usize;
    if let Some(row) = frame.rows.get(index) {
        return row.line;
    }
    match frame.rows.last() {
        Some(last) => last.line + (index + 1 - frame.rows.len()) as u32,
        None => frame.first_line + (visual - base),
    }
}

fn key_input(event: &KeyEvent) -> Option<EditorInputEvent> {
    let modifiers = modifier_bits(event.mods);
    if let Some(comment) = alt_slash(event) {
        return Some(comment);
    }
    let key = match &event.key {
        Key::Text(value) if event.mods.ctrl || event.mods.alt || event.mods.cmd => {
            let mut chars = value.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => EditorKey::Char(ch),
                _ => return None,
            }
        }
        Key::Text(value) => {
            let mut chars = value.chars();
            return Some(match (chars.next(), chars.next()) {
                (Some(ch), None) => EditorInputEvent::Key {
                    key: EditorKey::Char(ch),
                    modifiers,
                },
                (Some(_), Some(_)) => EditorInputEvent::Text(value.clone()),
                (None, _) => return None,
            });
        }
        Key::Command(ch) => {
            return command_chord(*ch, event.mods).map(|ch| EditorInputEvent::Key {
                key: EditorKey::Char(ch),
                modifiers: editor_modifiers::CONTROL,
            });
        }
        Key::Left => EditorKey::Left,
        Key::Right => EditorKey::Right,
        Key::Up => EditorKey::Up,
        Key::Down => EditorKey::Down,
        Key::Home => EditorKey::Home,
        Key::End => EditorKey::End,
        Key::PageUp => EditorKey::PageUp,
        Key::PageDown => EditorKey::PageDown,
        Key::Enter => EditorKey::Enter,
        Key::Tab => EditorKey::Tab,
        Key::Backspace => EditorKey::Backspace,
        Key::Delete => EditorKey::Delete,
        Key::Escape => EditorKey::Escape,
        Key::Insert => EditorKey::Insert,
        Key::Function(number) => EditorKey::Function(*number),
        _ => return None,
    };
    Some(EditorInputEvent::Key { key, modifiers })
}

/// The editor's key table is Ctrl-based, so a macOS ⌘ chord is sent as the
/// Ctrl key that does the same thing: the table Tauri's editor uses
/// (`editorChords.ts`). An unclaimed chord stays the app's, and ⌘V is left
/// to the native paste event.
/// Option-/ is the comment command, sent as the PTY-safe Ctrl-_ that ⌘/ is;
/// Option turns `/` into `÷` on a US layout, so that counts too, but not
/// Option-Shift-/ (`¿`), and not with ⌘ or Ctrl held.
fn alt_slash(event: &KeyEvent) -> Option<EditorInputEvent> {
    let mods = event.mods;
    if !mods.alt || mods.cmd || mods.ctrl || mods.logo {
        return None;
    }
    let Key::Text(value) = &event.key else {
        return None;
    };
    (value == "/" || (!mods.shift && value == "÷")).then_some(EditorInputEvent::Key {
        key: EditorKey::Char('_'),
        modifiers: editor_modifiers::CONTROL,
    })
}

fn command_chord(ch: char, mods: rune::Mods) -> Option<char> {
    let ch = ch.to_ascii_lowercase();
    if ch == '/' || (ch == '?' && mods.shift) {
        return (!mods.alt).then_some('_');
    }
    match (mods.alt, mods.shift, ch) {
        (true, true, _) => None,
        (true, false, 'f') => Some('r'),
        (true, false, 'l') => Some('g'),
        (true, false, _) => None,
        (false, true, 'g') => Some('b'),
        (false, true, 'z') => Some('y'),
        (false, true, _) => None,
        (false, false, 'g') => Some('n'),
        (false, false, 's' | 'c' | 'x' | 'f' | 'a' | 'z') => Some(ch),
        _ => None,
    }
}

fn modifier_bits(mods: rune::Mods) -> u8 {
    let mut bits = 0;
    if mods.shift {
        bits |= editor_modifiers::SHIFT;
    }
    if mods.ctrl {
        bits |= editor_modifiers::CONTROL;
    }
    if mods.alt {
        bits |= editor_modifiers::ALT;
    }
    if mods.cmd || mods.logo {
        bits |= editor_modifiers::META;
    }
    bits
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;

    use domain::{EditorRow, EditorScope, EditorSpan};

    /// A frame over `lines`, starting at `first_line`, with folds described by
    /// `hidden` lines above the window and in total.
    pub fn frame(
        first_line: u32,
        lines: &[(u32, &str)],
        total_lines: u32,
        hidden: (u32, u32),
    ) -> EditorFrame {
        EditorFrame {
            buffer_id: 1,
            doc_version: 1,
            first_line,
            total_lines,
            hidden_above: hidden.0,
            hidden_lines: hidden.1,
            rows: Arc::new(
                lines
                    .iter()
                    .map(|(line, text)| {
                        let mut row = EditorRow {
                            line: *line,
                            truncated: false,
                            spans: Arc::new(vec![EditorSpan {
                                text: (*text).into(),
                                scope: EditorScope::Plain,
                            }]),
                            mark: None,
                            diagnostic: None,
                            fold: None,
                            fingerprint: 0,
                        };
                        row.fingerprint = row.content_fingerprint();
                        row
                    })
                    .collect(),
            ),
            caret: EditorPlace {
                line: first_line,
                column: 0,
            },
            ..EditorFrame::default()
        }
    }

    fn surface(viewport: f32) -> (EditorUi, SessionId) {
        let session = SessionId::new();
        let mut ui = EditorUi::default();
        ui.set_session(Some(session));
        ui.scroll.set([0.0, viewport]);
        (ui, session)
    }

    #[test]
    fn the_first_window_covers_the_viewport_and_its_overscan() {
        let (mut ui, session) = surface(10.0 * ROW_H);
        let out = ui.take_requests();
        assert_eq!(out.view, Some((session, 0, 10 + 2 * OVERSCAN)));
        assert_eq!(
            ui.take_requests().view,
            None,
            "the same window is asked once"
        );
    }

    #[test]
    fn a_window_that_covers_the_viewport_asks_for_nothing() {
        let (mut ui, session) = surface(10.0 * ROW_H);
        let lines: Vec<(u32, String)> = (0..42).map(|line| (line, format!("{line}"))).collect();
        let lines: Vec<(u32, &str)> = lines.iter().map(|(l, t)| (*l, t.as_str())).collect();
        ui.apply_frame(session, frame(0, &lines, 400, (0, 0)));
        ui.take_requests();
        ui.scroll.set([5.0 * ROW_H, 10.0 * ROW_H]);
        assert_eq!(ui.take_requests().view, None);
        ui.scroll.set([100.0 * ROW_H, 10.0 * ROW_H]);
        assert_eq!(
            ui.take_requests().view,
            Some((session, 100 - OVERSCAN, 10 + 2 * OVERSCAN))
        );
    }

    #[test]
    fn a_window_has_edges_as_editor_window_does() {
        let (mut ui, session) = surface(5000.0 * ROW_H);
        assert_eq!(
            ui.take_requests().view,
            Some((session, 0, MAX_WINDOW)),
            "a tall viewport is capped"
        );
        let (mut ui, _) = surface(0.0);
        assert_eq!(
            ui.take_requests().view,
            None,
            "an unmeasured viewport asks nothing"
        );
        let (mut ui, session) = surface(10.0 * ROW_H);
        let lines: Vec<(u32, String)> = (0..42).map(|line| (line, format!("{line}"))).collect();
        let lines: Vec<(u32, &str)> = lines.iter().map(|(l, t)| (*l, t.as_str())).collect();
        ui.apply_frame(session, frame(0, &lines, 42, (0, 0)));
        ui.take_requests();
        ui.scroll.set([400.0 * ROW_H, 10.0 * ROW_H]);
        assert_eq!(
            ui.take_requests().view,
            None,
            "a scroll past the end asks for nothing past the last line"
        );
    }

    #[test]
    fn folds_above_the_window_shift_it_up_among_the_rows() {
        let shown = frame(50, &[(50, "a"), (51, "b"), (60, "c")], 200, (8, 20));
        assert_eq!(visual_base(&shown), 42);
        assert_eq!(content_rows(&shown), 180);
        assert_eq!(
            document_line(&shown, 44),
            60,
            "a fold inside the window is exact"
        );
        assert_eq!(document_line(&shown, 40), 48);
        assert_eq!(document_line(&shown, 46), 62);
    }

    #[test]
    fn a_moved_caret_scrolls_into_view_and_a_still_one_does_not() {
        let (mut ui, session) = surface(5.0 * ROW_H);
        let lines: Vec<(u32, &str)> = (0..20).map(|line| (line, "x")).collect();
        let mut shown = frame(0, &lines, 20, (0, 0));
        assert_eq!(ui.apply_frame(session, shown.clone()), None);
        shown.caret.line = 12;
        shown.doc_version = 2;
        assert_eq!(ui.apply_frame(session, shown.clone()), Some(8.0 * ROW_H));
        ui.scroll.set([8.0 * ROW_H, 5.0 * ROW_H]);
        shown.doc_version = 3;
        assert_eq!(ui.apply_frame(session, shown), None);
    }

    #[test]
    fn an_older_frame_and_another_sessions_frame_are_dropped() {
        let (mut ui, session) = surface(5.0 * ROW_H);
        let mut newer = frame(0, &[(0, "new")], 1, (0, 0));
        newer.doc_version = 5;
        ui.apply_frame(session, newer);
        let mut older = frame(0, &[(0, "old")], 1, (0, 0));
        older.doc_version = 4;
        ui.apply_frame(session, older.clone());
        ui.apply_frame(SessionId::new(), older);
        let held = ui.frame.as_ref().unwrap();
        assert_eq!(held.rows[0].text(), "new");
    }

    #[test]
    fn keys_become_named_input_and_a_burst_leaves_in_one_batch() {
        let (mut ui, session) = surface(0.0);
        let plain = rune::Mods::default();
        for key in [Key::Text("x".into()), Key::Left, Key::Text("ñé".into())] {
            assert!(ui.key(&KeyEvent { key, mods: plain }));
        }
        assert!(ui.key(&KeyEvent {
            key: Key::Command('s'),
            mods: rune::Mods { cmd: true, ..plain },
        }));
        assert!(
            !ui.key(&KeyEvent {
                key: Key::Command('v'),
                mods: rune::Mods { cmd: true, ..plain },
            }),
            "⌘V is the native paste"
        );
        let (to, events) = ui.take_requests().input.unwrap();
        assert_eq!(to, session);
        assert_eq!(
            events,
            vec![
                EditorInputEvent::Key {
                    key: EditorKey::Char('x'),
                    modifiers: 0
                },
                EditorInputEvent::Key {
                    key: EditorKey::Left,
                    modifiers: 0
                },
                EditorInputEvent::Text("ñé".into()),
                EditorInputEvent::Key {
                    key: EditorKey::Char('s'),
                    modifiers: editor_modifiers::CONTROL
                },
            ]
        );
        assert_eq!(ui.take_requests().input, None);
    }

    #[test]
    fn option_slash_is_the_comment_chord_even_as_a_division_sign() {
        let alt = rune::Mods {
            alt: true,
            ..rune::Mods::default()
        };
        let key = |text: &str, mods| {
            key_input(&KeyEvent {
                key: Key::Text(text.into()),
                mods,
            })
        };
        let comment = Some(EditorInputEvent::Key {
            key: EditorKey::Char('_'),
            modifiers: editor_modifiers::CONTROL,
        });
        assert_eq!(key("/", alt), comment);
        assert_eq!(key("÷", alt), comment);
        assert_eq!(key("/", rune::Mods { shift: true, ..alt }), comment);
        assert_ne!(key("¿", rune::Mods { shift: true, ..alt }), comment);
        assert_ne!(key("/", rune::Mods { ctrl: true, ..alt }), comment);
        assert_ne!(
            key("/", rune::Mods::default()),
            comment,
            "a plain slash is text"
        );
    }

    #[test]
    fn input_queued_for_a_session_left_is_handed_back_for_it() {
        let mut ui = EditorUi::default();
        let (first, second) = (SessionId::new(), SessionId::new());
        assert_eq!(ui.set_session(Some(first)), None);
        assert!(ui.key(&KeyEvent {
            key: Key::Text("a".into()),
            mods: rune::Mods::default(),
        }));
        let left = ui.set_session(Some(second)).expect("the queued key");
        assert_eq!(left.0, first, "sent to the session it was typed in");
        assert_eq!(left.1.len(), 1);
        assert_eq!(
            ui.set_session(None),
            None,
            "nothing queued, nothing handed back"
        );
    }

    #[test]
    fn command_chords_follow_the_tauri_table() {
        let cmd = rune::Mods {
            cmd: true,
            ..rune::Mods::default()
        };
        let shift = rune::Mods { shift: true, ..cmd };
        let alt = rune::Mods { alt: true, ..cmd };
        assert_eq!(command_chord('c', cmd), Some('c'));
        assert_eq!(command_chord('g', cmd), Some('n'), "⌘G is find-next");
        assert_eq!(command_chord('Z', shift), Some('y'));
        assert_eq!(command_chord('f', alt), Some('r'));
        assert_eq!(command_chord('/', cmd), Some('_'));
        assert_eq!(command_chord('n', cmd), None, "the app's new shell");
        assert_eq!(command_chord('v', cmd), None);
    }

    #[test]
    fn a_composition_is_drawn_and_only_its_commit_is_sent() {
        let (mut ui, session) = surface(0.0);
        ui.ime(&Ime::Preedit("に".into(), None));
        ui.ime(&Ime::Preedit("にほ".into(), None));
        assert_eq!(ui.preedit, "にほ");
        assert_eq!(ui.take_requests().input, None);
        ui.ime(&Ime::Commit("日本\n".into()));
        assert!(ui.preedit.is_empty());
        assert_eq!(
            ui.take_requests().input,
            Some((session, vec![EditorInputEvent::Text("日本".into())])),
            "a control character in a commit is not text"
        );
    }

    #[test]
    fn the_readable_text_follows_the_rows_and_not_the_caret() {
        let (mut ui, session) = surface(5.0 * ROW_H);
        let mut shown = frame(0, &[(0, "fn a() {"), (1, "}")], 2, (0, 0));
        ui.apply_frame(session, shown.clone());
        assert_eq!(&*ui.a11y_text, "fn a() {\n}\n");
        let before = Rc::clone(&ui.a11y_text);
        shown.caret.column = 3;
        shown.doc_version = 2;
        ui.apply_frame(session, shown);
        assert!(
            Rc::ptr_eq(&before, &ui.a11y_text),
            "a caret move rebuilds nothing"
        );
        let mut edited = frame(0, &[(0, "fn b() {"), (1, "}")], 2, (0, 0));
        edited.doc_version = 3;
        ui.apply_frame(session, edited);
        assert!(ui.a11y_text.starts_with("fn b"));
    }
}
