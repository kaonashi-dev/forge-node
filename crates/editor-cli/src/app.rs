//! The event loop's decision half: keys in, damage out. No terminal, no IO.
//!
//! Keys resolve to `editor_core::Command`, so the binding table is the only
//! place that knows about crossterm and the palette will reach the same
//! actions. This owns the viewport and which rows a frame must repaint; it
//! never owns the text, which lives in one `Document`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use editor_core::{execute, metrics, Command, Document, Query, Range, Refusal, Selection};

use crate::disk;

/// Rows a frame has to repaint.
pub struct Frame {
    pub rows: Vec<usize>,
    pub full: bool,
}

/// The one-line surface at the bottom, when it is not the status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    Find {
        input: String,
    },
    Replace {
        find: String,
        with: String,
        /// Which field the caret is in; Tab moves between them.
        editing_replacement: bool,
    },
    GotoLine {
        input: String,
    },
    /// A close with unsaved changes: save, discard or cancel.
    ConfirmClose,
}

pub struct App {
    document: Document,
    path: PathBuf,
    revision: Option<String>,
    register: String,
    query: Query,
    prompt: Option<Prompt>,
    help: bool,
    status: Option<String>,
    top: usize,
    left: usize,
    width: u16,
    height: u16,
    quit: bool,
    damaged: BTreeSet<usize>,
    damage_all: bool,
    /// Viewport the last frame was painted for; a change invalidates everything.
    painted: Option<(usize, usize, usize, u16)>,
}

impl App {
    pub fn new(document: Document, path: PathBuf, revision: Option<String>) -> Self {
        Self {
            document,
            path,
            revision,
            register: String::new(),
            query: Query::literal(""),
            prompt: None,
            help: false,
            status: None,
            top: 0,
            left: 0,
            width: 80,
            height: 24,
            quit: false,
            damaged: BTreeSet::new(),
            damage_all: true,
            painted: None,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    pub fn help_visible(&self) -> bool {
        self.help
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn top(&self) -> usize {
        self.top
    }

    pub fn left(&self) -> usize {
        self.left
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height_rows(&self) -> usize {
        self.height as usize
    }

    pub fn content_height(&self) -> usize {
        (self.height as usize).saturating_sub(1)
    }

    pub fn number_width(&self) -> usize {
        self.document.text().line_count().max(1).to_string().len()
    }

    /// Line-number column plus the space after it.
    pub fn gutter_width(&self) -> usize {
        self.number_width() + 1
    }

    pub fn content_width(&self) -> usize {
        (self.width as usize).saturating_sub(self.gutter_width())
    }

    /// Caret as a 1-based line and its display column.
    pub fn caret_position(&self) -> (usize, usize) {
        let position = self.document.caret_line_col();
        let column =
            metrics::display_column(self.document.text().line(position.line), position.column);
        (position.line + 1, column)
    }

    /// The selected byte range inside `line`, as offsets within that line.
    ///
    /// The end may sit one past the line's length: a selection that swallowed
    /// the line break has to look like it did.
    pub fn selection_in_line(&self, line: usize) -> Option<(usize, usize)> {
        let selection = self.document.selection();
        if selection.is_empty() {
            return None;
        }
        let range = selection.range();
        let text = self.document.text();
        let start = text.line_start(line);
        let end = text.line_end(line);
        if range.end <= start || range.start > end {
            return None;
        }
        let from = range.start.saturating_sub(start).min(end - start);
        let to = if range.end > end {
            end - start + 1
        } else {
            range.end - start
        };
        Some((from, to))
    }

    /// Open on a 1-based line, for `forge-editor +42 file.rs`.
    pub fn goto_line(&mut self, line: usize) {
        self.run(Command::GotoLine(line));
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.ensure_visible();
        self.damage_all = true;
    }

    /// Rows to repaint, and whether the whole viewport moved.
    pub fn take_frame(&mut self) -> Frame {
        let height = self.content_height();
        let viewport = (self.top, self.left, height, self.width);
        let full = self.damage_all || self.painted != Some(viewport);
        let rows = if full {
            (0..height).collect()
        } else {
            self.damaged
                .iter()
                .filter_map(|line| line.checked_sub(self.top))
                .filter(|row| *row < height)
                .collect()
        };
        self.damaged.clear();
        self.damage_all = false;
        self.painted = Some(viewport);
        Frame { rows, full }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        if self.help {
            self.help = false;
            self.damage_all = true;
            return;
        }
        if self.prompt.is_some() {
            self.handle_prompt_key(key);
            return;
        }
        match self.binding(key) {
            Some(Action::Command(command)) => self.run(command),
            Some(Action::Editor(action)) => self.run_editor_action(action),
            None => {}
        }
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.prompt {
            Some(Prompt::Find { input }) => {
                input.push_str(&text.replace('\n', " "));
                let input = input.clone();
                self.set_query(&input);
            }
            Some(Prompt::Replace {
                find,
                with,
                editing_replacement,
            }) => {
                let field = if *editing_replacement { with } else { find };
                field.push_str(&text.replace('\n', " "));
            }
            Some(Prompt::GotoLine { input }) => {
                input.extend(text.chars().filter(char::is_ascii_digit));
            }
            Some(Prompt::ConfirmClose) | None => self.run(Command::Paste(text.to_string())),
        }
        self.damage_all = true;
    }

    fn run(&mut self, command: Command) {
        let before = self.document.selection();
        let outcome = execute(&mut self.document, command);
        self.mark(before, &outcome);
        if let Some(text) = outcome.clipboard {
            self.register = text;
        }
        if let Some(count) = outcome.replaced {
            self.status = Some(format!("replaced {count}"));
        }
        if let Some(refusal) = outcome.refusal {
            self.status = Some(describe(refusal));
        }
        self.ensure_visible();
    }

    /// Damage the rows an outcome can have changed.
    fn mark(&mut self, before: Selection, outcome: &editor_core::Outcome) {
        if let Some(applied) = outcome.applied {
            if applied.line_delta != 0 {
                // Every row below the edit renumbers and shifts.
                self.damage_all = true;
            } else {
                for line in applied.first_line..=applied.last_line {
                    self.damaged.insert(line);
                }
            }
        }
        let after = self.document.selection();
        if before != after {
            self.damage_span(before.range());
            self.damage_span(after.range());
        }
    }

    fn damage_span(&mut self, range: Range) {
        let text = self.document.text();
        let first = text.line_of_offset(range.start);
        let last = text.line_of_offset(range.end);
        if last - first >= self.content_height() {
            self.damage_all = true;
            return;
        }
        for line in first..=last {
            self.damaged.insert(line);
        }
    }

    fn run_editor_action(&mut self, action: EditorAction) {
        match action {
            EditorAction::Save => self.save(),
            EditorAction::Close => self.request_close(),
            EditorAction::OpenFind => {
                self.prompt = Some(Prompt::Find {
                    input: self.query.pattern.clone(),
                });
            }
            EditorAction::OpenReplace => {
                self.prompt = Some(Prompt::Replace {
                    find: self.query.pattern.clone(),
                    with: String::new(),
                    editing_replacement: false,
                });
            }
            EditorAction::OpenGotoLine => {
                self.prompt = Some(Prompt::GotoLine {
                    input: String::new(),
                });
            }
            EditorAction::FindNext => self.find(true),
            EditorAction::FindPrevious => self.find(false),
            EditorAction::ToggleHelp => {
                self.help = true;
                self.damage_all = true;
            }
            EditorAction::PasteRegister => {
                let text = self.register.clone();
                if !text.is_empty() {
                    self.run(Command::Paste(text));
                }
            }
            EditorAction::ToggleCase => {
                self.query.case_sensitive = !self.query.case_sensitive;
                self.status = Some(format!(
                    "match case: {}",
                    if self.query.case_sensitive {
                        "on"
                    } else {
                        "off"
                    }
                ));
            }
            EditorAction::Cancel => {
                self.document.break_undo_group();
            }
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        self.damage_all = true;
        match prompt {
            Prompt::ConfirmClose => match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    self.save();
                    if !self.document.is_dirty() {
                        self.quit = true;
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('D') => self.quit = true,
                _ => self.status = Some("close cancelled".to_string()),
            },
            Prompt::GotoLine { mut input } => match key.code {
                KeyCode::Enter => match input.parse::<usize>() {
                    Ok(line) if line > 0 => self.run(Command::GotoLine(line)),
                    _ => self.status = Some("not a line number".to_string()),
                },
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    input.pop();
                    self.prompt = Some(Prompt::GotoLine { input });
                }
                KeyCode::Char(character) if character.is_ascii_digit() => {
                    input.push(character);
                    self.prompt = Some(Prompt::GotoLine { input });
                }
                _ => self.prompt = Some(Prompt::GotoLine { input }),
            },
            Prompt::Find { mut input } => match key.code {
                KeyCode::Enter => {
                    self.set_query(&input);
                    self.find(true);
                }
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    input.pop();
                    self.set_query(&input);
                    self.prompt = Some(Prompt::Find { input });
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.push(character);
                    self.set_query(&input);
                    self.prompt = Some(Prompt::Find { input });
                }
                _ => self.prompt = Some(Prompt::Find { input }),
            },
            Prompt::Replace {
                mut find,
                mut with,
                mut editing_replacement,
            } => {
                let mut keep = true;
                match key.code {
                    KeyCode::Enter => {
                        self.set_query(&find);
                        self.run(Command::ReplaceAll {
                            query: self.query.clone(),
                            replacement: with.clone(),
                        });
                        keep = false;
                    }
                    KeyCode::Esc => keep = false,
                    KeyCode::Tab => editing_replacement = !editing_replacement,
                    KeyCode::Backspace => {
                        if editing_replacement {
                            with.pop();
                        } else {
                            find.pop();
                        }
                    }
                    KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if editing_replacement {
                            with.push(character);
                        } else {
                            find.push(character);
                        }
                    }
                    _ => {}
                }
                if keep {
                    self.prompt = Some(Prompt::Replace {
                        find,
                        with,
                        editing_replacement,
                    });
                }
            }
        }
    }

    fn set_query(&mut self, pattern: &str) {
        let case_sensitive = self.query.case_sensitive;
        let whole_word = self.query.whole_word;
        self.query = Query {
            pattern: pattern.to_string(),
            case_sensitive,
            whole_word,
        };
    }

    fn find(&mut self, forward: bool) {
        if self.query.is_empty() {
            self.status = Some("nothing to find".to_string());
            return;
        }
        let command = if forward {
            Command::FindNext(self.query.clone())
        } else {
            Command::FindPrevious(self.query.clone())
        };
        self.run(command);
        if self.status.is_none() {
            let total = editor_core::count_matches(
                self.document.text(),
                &self.query,
                editor_core::limits::MAX_SEARCH_RESULTS + 1,
            );
            let shown = total.min(editor_core::limits::MAX_SEARCH_RESULTS);
            self.status = Some(if total > shown {
                format!("{}: more than {shown} matches", self.query.pattern)
            } else {
                format!("{}: {shown} matches", self.query.pattern)
            });
        }
    }

    fn save(&mut self) {
        if self.document.is_read_only() {
            self.status = Some(describe(Refusal::ReadOnly));
            return;
        }
        // Optimistic, not exclusive: another program can still write between
        // this check and the rename. It catches the agent that already did.
        if let (Some(expected), Some(current)) = (&self.revision, disk::revision(&self.path)) {
            if expected != &current {
                self.status =
                    Some("the file changed on disk — press Ctrl-S again to overwrite".to_string());
                self.revision = Some(current);
                return;
            }
        }
        let snapshot = self.document.snapshot();
        match disk::save(&self.path, snapshot.text()) {
            Ok(revision) => {
                self.document.confirm_save(&snapshot, revision.clone());
                self.revision = Some(revision);
                self.status = Some(if self.document.is_dirty() {
                    "saved — newer keystrokes are still unsaved".to_string()
                } else {
                    "saved".to_string()
                });
            }
            Err(error) => self.status = Some(format!("save failed: {error}")),
        }
    }

    fn request_close(&mut self) {
        if self.document.is_dirty() {
            self.prompt = Some(Prompt::ConfirmClose);
            self.damage_all = true;
            return;
        }
        self.quit = true;
    }

    /// Scroll just enough to keep the caret visible, never a cell more.
    fn ensure_visible(&mut self) {
        let height = self.content_height().max(1);
        let position = self.document.caret_line_col();
        if position.line < self.top {
            self.top = position.line;
        }
        let last_row = self.top + height - 1;
        if position.line > last_row {
            self.top = position.line + 1 - height;
        }

        let width = self.content_width().max(1);
        let column =
            metrics::display_column(self.document.text().line(position.line), position.column);
        if column < self.left {
            self.left = column;
        }
        let last_column = self.left + width - 1;
        if column > last_column {
            self.left = column + 1 - width;
        }
    }

    /// The key table. One place knows crossterm; everything else sees commands.
    fn binding(&self, key: KeyEvent) -> Option<Action> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let word = control || alt;
        let page = self.content_height().max(1);

        let action = match (key.code, control, alt) {
            (KeyCode::Char('s'), true, _) => Action::Editor(EditorAction::Save),
            (KeyCode::Char('q'), true, _) => Action::Editor(EditorAction::Close),
            (KeyCode::Char('f'), true, _) => Action::Editor(EditorAction::OpenFind),
            (KeyCode::Char('r'), true, _) => Action::Editor(EditorAction::OpenReplace),
            (KeyCode::Char('g'), true, _) => Action::Editor(EditorAction::OpenGotoLine),
            (KeyCode::Char('t'), true, _) => Action::Editor(EditorAction::ToggleCase),
            (KeyCode::Char('v'), true, _) => Action::Editor(EditorAction::PasteRegister),
            (KeyCode::Char('c'), true, _) => Action::Command(Command::Copy),
            (KeyCode::Char('x'), true, _) => Action::Command(Command::Cut),
            (KeyCode::Char('a'), true, _) => Action::Command(Command::SelectAll),
            (KeyCode::Char('k'), true, _) => Action::Command(Command::DeleteLine),
            (KeyCode::Char('l'), true, _) => Action::Command(Command::SelectLine),
            (KeyCode::Char('z'), true, _) => Action::Command(Command::Undo),
            (KeyCode::Char('y'), true, _) => Action::Command(Command::Redo),
            (KeyCode::Char('n'), true, _) => Action::Editor(EditorAction::FindNext),
            (KeyCode::Char('b'), true, _) => Action::Editor(EditorAction::FindPrevious),
            (KeyCode::F(1), _, _) => Action::Editor(EditorAction::ToggleHelp),
            (KeyCode::F(3), _, _) if shift => Action::Editor(EditorAction::FindPrevious),
            (KeyCode::F(3), _, _) => Action::Editor(EditorAction::FindNext),
            (KeyCode::Esc, _, _) => Action::Editor(EditorAction::Cancel),

            (KeyCode::Left, _, _) if word => {
                Action::Command(Command::MoveWordLeft { extend: shift })
            }
            (KeyCode::Right, _, _) if word => {
                Action::Command(Command::MoveWordRight { extend: shift })
            }
            (KeyCode::Left, _, _) => Action::Command(Command::MoveLeft { extend: shift }),
            (KeyCode::Right, _, _) => Action::Command(Command::MoveRight { extend: shift }),
            (KeyCode::Up, _, _) => Action::Command(Command::MoveUp { extend: shift }),
            (KeyCode::Down, _, _) => Action::Command(Command::MoveDown { extend: shift }),
            (KeyCode::Home, true, _) => {
                Action::Command(Command::MoveDocumentStart { extend: shift })
            }
            (KeyCode::End, true, _) => Action::Command(Command::MoveDocumentEnd { extend: shift }),
            (KeyCode::Home, _, _) => Action::Command(Command::MoveLineStart { extend: shift }),
            (KeyCode::End, _, _) => Action::Command(Command::MoveLineEnd { extend: shift }),
            (KeyCode::PageUp, _, _) => Action::Command(Command::MovePage {
                lines: page,
                down: false,
                extend: shift,
            }),
            (KeyCode::PageDown, _, _) => Action::Command(Command::MovePage {
                lines: page,
                down: true,
                extend: shift,
            }),

            (KeyCode::Enter, _, _) => Action::Command(Command::InsertNewline),
            (KeyCode::Tab, _, _) => Action::Command(Command::InsertTab),
            (KeyCode::Backspace, _, true) => Action::Command(Command::DeleteWordBackward),
            (KeyCode::Backspace, _, _) => Action::Command(Command::DeleteBackward),
            (KeyCode::Delete, _, _) => Action::Command(Command::DeleteForward),
            (KeyCode::Char(character), false, false) => {
                Action::Command(Command::InsertText(character.to_string()))
            }
            _ => return None,
        };
        Some(action)
    }
}

enum Action {
    Command(Command),
    Editor(EditorAction),
}

enum EditorAction {
    Save,
    Close,
    OpenFind,
    OpenReplace,
    OpenGotoLine,
    FindNext,
    FindPrevious,
    ToggleHelp,
    ToggleCase,
    PasteRegister,
    Cancel,
}

fn describe(refusal: Refusal) -> String {
    match refusal {
        Refusal::ReadOnly => "read-only: editing is off".to_string(),
        Refusal::TooLarge { size, limit } => {
            format!("refused: the result would be {size} bytes, over the {limit} byte limit")
        }
        Refusal::NothingToUndo => "nothing to undo".to_string(),
        Refusal::NothingToRedo => "nothing to redo".to_string(),
        Refusal::NoMatch => "no match".to_string(),
        Refusal::TooManyMatches { limit } => {
            format!("refused: more than {limit} matches, nothing was replaced")
        }
    }
}

/// Keys the help overlay lists, in the order it shows them.
pub const HELP: &[(&str, &str)] = &[
    ("arrows, Home/End, PgUp/PgDn", "move; hold Shift to select"),
    ("Ctrl/Alt + arrows", "move by word"),
    ("Ctrl-S", "save"),
    ("Ctrl-Z / Ctrl-Y", "undo / redo"),
    ("Ctrl-C / Ctrl-X / Ctrl-V", "copy / cut / paste"),
    (
        "Ctrl-A / Ctrl-L / Ctrl-K",
        "select all / select line / delete line",
    ),
    ("Ctrl-F", "find; Enter searches, Esc closes"),
    ("Ctrl-N / F3", "find next"),
    ("Ctrl-B / Shift-F3", "find previous"),
    ("Ctrl-T", "toggle match case"),
    ("Ctrl-R", "replace all; Tab switches field"),
    ("Ctrl-G", "go to line"),
    ("Ctrl-Q", "close; unsaved changes ask first"),
    ("F1", "this help"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn app(text: &str, read_only: bool) -> App {
        let document = Document::from_bytes(text.as_bytes(), read_only).expect("fixture");
        App::new(document, PathBuf::from("fixture.txt"), None)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn control(code: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL)
    }

    fn shifted(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn typing_inserts_and_marks_dirty() {
        let mut app = app("hello", false);
        app.handle_key(key(KeyCode::Char('X')));
        app.handle_key(key(KeyCode::Char('!')));
        assert_eq!(app.document().as_str(), "X!hello");
        assert!(app.document().is_dirty());
        assert_eq!(app.caret_position(), (1, 2));
    }

    #[test]
    fn a_keystroke_repaints_one_row_not_the_viewport() {
        let text = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text, false);
        app.resize(40, 12);
        let first = app.take_frame();
        assert!(first.full, "the first frame paints everything");

        app.handle_key(key(KeyCode::Char('X')));
        let frame = app.take_frame();
        assert!(!frame.full);
        assert_eq!(frame.rows, vec![0]);
    }

    #[test]
    fn a_line_break_repaints_the_rows_below_it() {
        let text = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text, false);
        app.resize(40, 12);
        app.take_frame();
        app.handle_key(key(KeyCode::Enter));
        assert!(app.take_frame().full);
    }

    #[test]
    fn shift_arrow_selects_and_typing_replaces_the_selection() {
        let mut app = app("hello", false);
        app.handle_key(shifted(KeyCode::Right));
        app.handle_key(shifted(KeyCode::Right));
        assert_eq!(app.selection_in_line(0), Some((0, 2)));
        app.handle_key(key(KeyCode::Char('Z')));
        assert_eq!(app.document().as_str(), "Zllo");
    }

    #[test]
    fn undo_and_redo_walk_the_typing_run() {
        let mut app = app("", false);
        for character in "abc".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(control('z'));
        assert_eq!(app.document().as_str(), "");
        app.handle_key(control('y'));
        assert_eq!(app.document().as_str(), "abc");
    }

    #[test]
    fn cut_and_paste_go_through_the_register() {
        let mut app = app("one\ntwo\n", false);
        app.handle_key(control('x'));
        assert_eq!(app.document().as_str(), "two\n");
        app.handle_key(control('v'));
        assert_eq!(app.document().as_str(), "one\ntwo\n");
    }

    #[test]
    fn control_c_copies_instead_of_quitting() {
        let mut app = app("one\n", false);
        app.handle_key(control('c'));
        assert!(!app.should_quit());
        assert_eq!(app.register, "one\n");
    }

    #[test]
    fn closing_a_dirty_buffer_asks_before_discarding() {
        let mut app = app("hello", false);
        app.handle_key(key(KeyCode::Char('X')));
        app.handle_key(control('q'));
        assert!(!app.should_quit());
        assert_eq!(app.prompt(), Some(&Prompt::ConfirmClose));

        app.handle_key(key(KeyCode::Char('c')));
        assert!(!app.should_quit(), "an unrecognized answer cancels");

        app.handle_key(control('q'));
        app.handle_key(key(KeyCode::Char('d')));
        assert!(app.should_quit());
    }

    #[test]
    fn a_clean_buffer_closes_without_a_prompt() {
        let mut app = app("hello", false);
        app.handle_key(control('q'));
        assert!(app.should_quit());
    }

    #[test]
    fn read_only_reports_instead_of_editing() {
        let mut app = app("hello", true);
        app.handle_key(key(KeyCode::Char('X')));
        assert_eq!(app.document().as_str(), "hello");
        assert_eq!(app.status(), Some("read-only: editing is off"));
    }

    #[test]
    fn find_moves_the_caret_and_counts_the_matches() {
        let mut app = app("alpha beta alpha", false);
        app.handle_key(control('f'));
        for character in "alpha".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.document().selection().range(), Range::new(0, 5));
        assert_eq!(app.status(), Some("alpha: 2 matches"));
        assert!(app.prompt().is_none());
    }

    #[test]
    fn replace_all_runs_from_the_prompt() {
        let mut app = app("a a a", false);
        app.handle_key(control('r'));
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.document().as_str(), "b b b");
        assert_eq!(app.status(), Some("replaced 3"));
    }

    #[test]
    fn go_to_line_takes_a_one_based_number() {
        let mut app = app("one\ntwo\nthree\n", false);
        app.handle_key(control('g'));
        app.handle_key(key(KeyCode::Char('3')));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.caret_position(), (3, 0));
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
        assert_eq!(app.caret_position().0, 10);
        assert_eq!(app.top(), 1);
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.caret_position().0, 19);
        assert_eq!(app.top(), 10);
    }

    #[test]
    fn esc_cancels_a_prompt_without_touching_the_document() {
        let mut app = app("hello", false);
        app.handle_key(control('f'));
        app.handle_key(key(KeyCode::Char('z')));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.prompt().is_none());
        assert_eq!(app.document().as_str(), "hello");
        assert!(!app.document().is_dirty());
    }

    #[test]
    fn a_selection_that_crosses_a_line_break_shows_on_both_rows() {
        let mut app = app("ab\ncd", false);
        app.handle_key(control('a'));
        assert_eq!(app.selection_in_line(0), Some((0, 3)));
        assert_eq!(app.selection_in_line(1), Some((0, 2)));
    }
}
