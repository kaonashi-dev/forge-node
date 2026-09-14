//! The event loop's decision half: no terminal, no IO.
//!
//! Keeping the app free of crossterm's writer is what lets keys be tested by
//! constructing `KeyEvent`s, and the viewport policy lives here: scroll just
//! enough to keep the cursor visible, never a cell more.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::document::{Document, Position};
use crate::view;

const REFUSED: &str = "read-only: editing is off";

pub struct App {
    document: Document,
    path: PathBuf,
    cursor: Position,
    top: usize,
    left: usize,
    width: u16,
    height: u16,
    status: Option<String>,
    quit_armed: bool,
    quit: bool,
}

impl App {
    pub fn new(document: Document, path: PathBuf) -> Self {
        Self {
            document,
            path,
            cursor: Position::default(),
            top: 0,
            left: 0,
            width: 80,
            height: 24,
            status: None,
            quit_armed: false,
            quit: false,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cursor(&self) -> Position {
        self.cursor
    }

    pub fn top(&self) -> usize {
        self.top
    }

    pub fn left(&self) -> usize {
        self.left
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn content_height(&self) -> usize {
        (self.height as usize).saturating_sub(1)
    }

    pub fn number_width(&self) -> usize {
        self.document.line_count().max(1).to_string().len()
    }

    /// Line-number column plus the space after it.
    pub fn gutter_width(&self) -> usize {
        self.number_width() + 1
    }

    pub fn content_width(&self) -> usize {
        (self.width as usize).saturating_sub(self.gutter_width())
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.ensure_visible();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.status = None;
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if control {
            match key.code {
                KeyCode::Char('s') => {
                    self.save();
                    return true;
                }
                KeyCode::Char('q') | KeyCode::Char('c') => {
                    self.request_quit();
                    return true;
                }
                _ => return true,
            }
        }

        match key.code {
            KeyCode::Left => self.set_cursor(self.document.move_left(self.cursor)),
            KeyCode::Right => self.set_cursor(self.document.move_right(self.cursor)),
            KeyCode::Up => self.set_cursor(self.document.move_vertical(self.cursor, -1)),
            KeyCode::Down => self.set_cursor(self.document.move_vertical(self.cursor, 1)),
            KeyCode::Home => self.set_cursor(self.document.line_start(self.cursor)),
            KeyCode::End => self.set_cursor(self.document.line_end(self.cursor)),
            KeyCode::PageUp => {
                let step = self.content_height().max(1) as isize;
                self.set_cursor(self.document.move_vertical(self.cursor, -step));
            }
            KeyCode::PageDown => {
                let step = self.content_height().max(1) as isize;
                self.set_cursor(self.document.move_vertical(self.cursor, step));
            }
            KeyCode::Enter => self.edit(|doc, cursor| doc.insert_str(cursor, "\n")),
            KeyCode::Tab => self.edit(|doc, cursor| doc.insert_tab(cursor)),
            KeyCode::Backspace => self.edit(|doc, cursor| doc.backspace(cursor)),
            KeyCode::Delete => self.edit(|doc, cursor| doc.delete(cursor)),
            KeyCode::Char(character) if !control && !alt => {
                let mut buffer = [0_u8; 4];
                let text = character.encode_utf8(&mut buffer);
                self.edit(|doc, cursor| doc.insert_str(cursor, text));
            }
            _ => {}
        }
        true
    }

    pub fn paste(&mut self, text: &str) {
        self.edit(|doc, cursor| doc.insert_str(cursor, text));
    }

    fn edit(&mut self, action: impl FnOnce(&mut Document, Position) -> Option<Position>) {
        self.status = None;
        match action(&mut self.document, self.cursor) {
            Some(cursor) => {
                self.quit_armed = false;
                self.set_cursor(cursor);
            }
            None => self.status = Some(REFUSED.to_string()),
        }
    }

    fn set_cursor(&mut self, cursor: Position) {
        self.cursor = self.document.clamp(cursor);
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        let height = self.content_height().max(1);
        if self.cursor.line < self.top {
            self.top = self.cursor.line;
        }
        let last_row = self.top + height - 1;
        if self.cursor.line > last_row {
            self.top = self.cursor.line + 1 - height;
        }

        let width = self.content_width().max(1);
        let column = view::display_col(self.document.line(self.cursor.line), self.cursor.col);
        if column < self.left {
            self.left = column;
        }
        let last_column = self.left + width - 1;
        if column > last_column {
            self.left = column + 1 - width;
        }
    }

    fn save(&mut self) {
        if self.document.is_read_only() {
            self.status = Some(REFUSED.to_string());
            return;
        }
        let path = self.path.clone();
        match self.document.save_to(&path) {
            Ok(()) => self.status = Some("saved".to_string()),
            Err(error) => self.status = Some(format!("save failed: {error}")),
        }
    }

    fn request_quit(&mut self) {
        if self.document.is_dirty() && !self.quit_armed {
            self.quit_armed = true;
            self.status = Some("unsaved changes — Ctrl-Q again discards them".to_string());
            return;
        }
        self.quit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(text: &str, read_only: bool) -> App {
        let document = Document::from_bytes(text.as_bytes(), read_only).expect("fixture");
        App::new(document, PathBuf::from("fixture.txt"))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_inserts_and_marks_dirty() {
        let mut app = app("hello", false);
        app.handle_key(key(KeyCode::Char('X')));
        app.handle_key(key(KeyCode::Char('!')));
        assert_eq!(app.document().render_text(), "X!hello");
        assert!(app.document().is_dirty());
        assert_eq!(app.cursor(), Position { line: 0, col: 2 });
    }

    #[test]
    fn a_second_control_q_discards_unsaved_changes() {
        let mut app = app("hello", false);
        app.handle_key(key(KeyCode::Char('X')));
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(!app.should_quit());
        assert!(app.status().is_some());
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(app.should_quit());
    }

    #[test]
    fn read_only_typing_reports_instead_of_editing() {
        let mut app = app("hello", true);
        app.handle_key(key(KeyCode::Char('X')));
        assert_eq!(app.document().render_text(), "hello");
        assert_eq!(app.status(), Some(REFUSED));
    }

    #[test]
    fn paging_keeps_the_cursor_inside_the_viewport() {
        let text = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text, false);
        app.resize(40, 10);
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.cursor().line, 9);
        assert_eq!(app.top(), 1);
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.cursor().line, 18);
        assert_eq!(app.top(), 10);
    }

    #[test]
    fn paste_goes_through_the_document() {
        let mut app = app("", false);
        app.paste("one\ntwo");
        assert_eq!(app.document().render_text(), "one\ntwo");
        assert_eq!(app.cursor(), Position { line: 1, col: 3 });
    }
}
