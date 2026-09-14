//! The spike's buffer: lines, grapheme positions and an atomic save.
//!
//! Deliberately not the F2 core. Text lives in `Vec<String>`; there is no undo,
//! no selection and no rope. What it does carry is the contract the F1 smoke
//! must be honest about: grapheme-level movement and deletion, UTF-8 and 2 MiB
//! limits, LF/CRLF and final-newline preservation, and a save that writes a
//! temp file and renames it so a crash cannot leave half a file.

use std::fmt;
use std::io::Write;
use std::path::Path;

use unicode_segmentation::UnicodeSegmentation;

use crate::view;

/// The plan's D10 limit, enforced while loading so no larger allocation starts.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("file is larger than 2 MiB ({bytes} bytes)")]
    TooLarge { bytes: usize },
    #[error("file is not valid UTF-8")]
    NotUtf8,
}

/// The line terminator the file arrived with. A spike normalizes mixed files
/// to the first terminator it saw; the product must preserve them per line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }
}

/// Line index (0-based) and grapheme index within that line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

pub struct Document {
    lines: Vec<String>,
    eol: Eol,
    final_newline: bool,
    dirty: bool,
    read_only: bool,
}

impl Document {
    pub fn from_bytes(bytes: &[u8], read_only: bool) -> Result<Self, LoadError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(LoadError::TooLarge { bytes: bytes.len() });
        }
        let text = std::str::from_utf8(bytes).map_err(|_| LoadError::NotUtf8)?;
        let eol = if text.contains("\r\n") {
            Eol::Crlf
        } else {
            Eol::Lf
        };
        let final_newline = text.ends_with('\n');
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|line| match eol {
                Eol::Crlf => line.strip_suffix('\r').unwrap_or(line).to_string(),
                Eol::Lf => line.to_string(),
            })
            .collect();
        // A trailing terminator produces one empty piece that is the sentinel,
        // not a line; `final_newline` is what records it.
        if final_newline {
            lines.pop();
        }
        Ok(Self {
            lines,
            eol,
            final_newline,
            dirty: false,
            read_only,
        })
    }

    pub fn line(&self, index: usize) -> &str {
        &self.lines[index]
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The full text as it would be written to disk.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                out.push_str(self.eol.as_str());
            }
            out.push_str(line);
        }
        if self.final_newline {
            out.push_str(self.eol.as_str());
        }
        out
    }

    pub fn clamp(&self, position: Position) -> Position {
        let line = position.line.min(self.lines.len() - 1);
        Position {
            line,
            col: position.col.min(view::grapheme_count(&self.lines[line])),
        }
    }

    /* ------------------------------------------------------------ movement --- */

    pub fn line_start(&self, position: Position) -> Position {
        Position {
            line: self.clamp(position).line,
            col: 0,
        }
    }

    pub fn line_end(&self, position: Position) -> Position {
        let at = self.clamp(position);
        Position {
            line: at.line,
            col: view::grapheme_count(&self.lines[at.line]),
        }
    }

    pub fn move_left(&self, position: Position) -> Position {
        let mut at = self.clamp(position);
        if at.col > 0 {
            at.col -= 1;
        } else if at.line > 0 {
            at.line -= 1;
            at.col = view::grapheme_count(&self.lines[at.line]);
        }
        at
    }

    pub fn move_right(&self, position: Position) -> Position {
        let mut at = self.clamp(position);
        if at.col < view::grapheme_count(&self.lines[at.line]) {
            at.col += 1;
        } else if at.line + 1 < self.lines.len() {
            at.line += 1;
            at.col = 0;
        }
        at
    }

    pub fn move_vertical(&self, position: Position, delta: isize) -> Position {
        let at = self.clamp(position);
        let last = self.lines.len() - 1;
        let line = (at.line as isize + delta).clamp(0, last as isize) as usize;
        Position {
            line,
            col: at.col.min(view::grapheme_count(&self.lines[line])),
        }
    }

    /* --------------------------------------------------------------- edits --- */

    /// Insert `text`, including `\n` (typing, newline and paste share this).
    ///
    /// Returns where the cursor lands, or `None` when the buffer is read-only.
    pub fn insert_str(&mut self, position: Position, text: &str) -> Option<Position> {
        if self.read_only {
            return None;
        }
        let at = self.clamp(position);
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();

        let offset = byte_at(&self.lines[at.line], at.col);
        if rest.is_empty() {
            self.lines[at.line].insert_str(offset, first);
            self.dirty = true;
            let added = view::grapheme_count(first);
            return Some(Position {
                line: at.line,
                col: at.col + added,
            });
        }

        let tail = self.lines[at.line][offset..].to_string();
        {
            let line = &mut self.lines[at.line];
            line.truncate(offset);
            line.push_str(first);
        }

        let mut inserted = Vec::with_capacity(rest.len());
        let last_index = rest.len() - 1;
        for (index, part) in rest.iter().enumerate() {
            let mut piece = part.to_string();
            if index == last_index {
                piece.push_str(&tail);
            }
            inserted.push(piece);
        }
        // The cursor lands after the inserted text, not after the tail that
        // was already there — one line down and one column right of `at`.
        let last_col = view::grapheme_count(rest.last().copied().unwrap_or(""));
        let last_line = at.line + inserted.len();
        self.lines.splice(at.line + 1..at.line + 1, inserted);
        self.dirty = true;
        Some(Position {
            line: last_line,
            col: last_col,
        })
    }

    pub fn backspace(&mut self, position: Position) -> Option<Position> {
        if self.read_only {
            return None;
        }
        let at = self.clamp(position);
        if at.col > 0 {
            let line = &mut self.lines[at.line];
            let end = byte_at(line, at.col);
            let start = byte_at(line, at.col - 1);
            line.replace_range(start..end, "");
            self.dirty = true;
            Some(Position {
                line: at.line,
                col: at.col - 1,
            })
        } else if at.line > 0 {
            let tail = self.lines[at.line].clone();
            let col = view::grapheme_count(&self.lines[at.line - 1]);
            self.lines[at.line - 1].push_str(&tail);
            self.lines.remove(at.line);
            self.dirty = true;
            Some(Position {
                line: at.line - 1,
                col,
            })
        } else {
            Some(at)
        }
    }

    pub fn delete(&mut self, position: Position) -> Option<Position> {
        if self.read_only {
            return None;
        }
        let at = self.clamp(position);
        let last_col = view::grapheme_count(&self.lines[at.line]);
        if at.col < last_col {
            let line = &mut self.lines[at.line];
            let start = byte_at(line, at.col);
            let end = byte_at(line, at.col + 1);
            line.replace_range(start..end, "");
            self.dirty = true;
        } else if at.line + 1 < self.lines.len() {
            let tail = self.lines.remove(at.line + 1);
            self.lines[at.line].push_str(&tail);
            self.dirty = true;
        }
        Some(at)
    }

    pub fn insert_tab(&mut self, position: Position) -> Option<Position> {
        let at = self.clamp(position);
        let column = view::display_col(&self.lines[at.line], at.col);
        let spaces = view::TAB_WIDTH - column % view::TAB_WIDTH;
        let text = " ".repeat(spaces);
        self.insert_str(at, &text)
    }

    /* ---------------------------------------------------------------- disk --- */

    /// Write the buffer through a temp file in the destination directory.
    ///
    /// Not the product's revision-checked write (F7); it is the simple thing
    /// that never leaves a truncated file, which is what the smoke can assert.
    pub fn save_to(&mut self, path: &Path) -> std::io::Result<()> {
        let text = self.render_text();
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("forge-editor");
        let temp = directory.join(format!(".{name}.forge-editor-{}", std::process::id()));

        let write = || -> std::io::Result<()> {
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()
        };
        if let Err(error) = write() {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        self.dirty = false;
        Ok(())
    }
}

impl fmt::Debug for Document {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Document")
            .field("lines", &self.lines.len())
            .field("eol", &self.eol)
            .field("final_newline", &self.final_newline)
            .field("dirty", &self.dirty)
            .field("read_only", &self.read_only)
            .finish()
    }
}

fn byte_at(line: &str, grapheme_index: usize) -> usize {
    line.grapheme_indices(true)
        .nth(grapheme_index)
        .map(|(offset, _)| offset)
        .unwrap_or(line.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        Document::from_bytes(text.as_bytes(), false).expect("valid fixture")
    }

    #[test]
    fn round_trips_lf_crlf_and_a_missing_final_newline() {
        for text in ["a\nb\n", "a\r\nb\r\n", "a\nb", "", "\n", "a\n\n"] {
            let doc = document(text);
            assert_eq!(doc.render_text(), text, "round trip of {text:?}");
        }
    }

    #[test]
    fn insert_and_backspace_move_by_grapheme() {
        let mut doc = document("a🇪🇸b");
        let at = doc
            .insert_str(Position { line: 0, col: 1 }, "X")
            .expect("editable");
        assert_eq!(at, Position { line: 0, col: 2 });
        assert_eq!(doc.render_text(), "aX🇪🇸b");
        assert!(doc.is_dirty());
        let at = doc.backspace(at).expect("editable");
        assert_eq!(at, Position { line: 0, col: 1 });
        assert_eq!(doc.render_text(), "a🇪🇸b");
    }

    #[test]
    fn newline_splits_and_backspace_at_column_zero_joins() {
        let mut doc = document("hello");
        let at = doc
            .insert_str(Position { line: 0, col: 2 }, "\n")
            .expect("editable");
        assert_eq!(at, Position { line: 1, col: 0 });
        assert_eq!(doc.render_text(), "he\nllo");
        let at = doc.backspace(at).expect("editable");
        assert_eq!(at, Position { line: 0, col: 2 });
        assert_eq!(doc.render_text(), "hello");
    }

    #[test]
    fn paste_inserts_every_line_and_leaves_the_cursor_at_the_end() {
        let mut doc = document("ab\ncd\n");
        let at = doc
            .insert_str(Position { line: 0, col: 1 }, "X\nY\nZ")
            .expect("editable");
        assert_eq!(doc.render_text(), "aX\nY\nZb\ncd\n");
        assert_eq!(at, Position { line: 2, col: 1 });
    }

    #[test]
    fn tabs_advance_to_the_next_stop() {
        let mut doc = document("abcdef");
        doc.insert_tab(Position { line: 0, col: 1 })
            .expect("editable");
        assert_eq!(doc.render_text(), "a   bcdef");
    }

    #[test]
    fn read_only_refuses_every_edit() {
        let mut doc = Document::from_bytes(b"a", true).expect("valid fixture");
        assert_eq!(doc.insert_str(Position::default(), "x"), None);
        assert_eq!(doc.backspace(Position::default()), None);
        assert_eq!(doc.delete(Position::default()), None);
        assert_eq!(doc.insert_tab(Position::default()), None);
        assert!(!doc.is_dirty());
    }

    #[test]
    fn rejects_oversized_and_invalid_utf8_files() {
        let oversized = vec![b'a'; MAX_FILE_BYTES + 1];
        assert!(matches!(
            Document::from_bytes(&oversized, false),
            Err(LoadError::TooLarge { .. })
        ));
        assert!(matches!(
            Document::from_bytes(&[0xff, 0xfe], false),
            Err(LoadError::NotUtf8)
        ));
    }

    #[test]
    fn save_is_atomic_and_clears_dirty() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("note.txt");
        std::fs::write(&path, "old").expect("fixture write");

        let mut doc = document("new");
        doc.insert_str(Position::default(), "!").expect("editable");
        assert!(doc.is_dirty());
        doc.save_to(&path).expect("save");

        assert!(!doc.is_dirty());
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), "!new");
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("forge-editor"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn vertical_movement_clamps_the_column() {
        let doc = document("long line\nx");
        let at = doc.move_vertical(Position { line: 0, col: 9 }, 1);
        assert_eq!(at, Position { line: 1, col: 1 });
    }
}
