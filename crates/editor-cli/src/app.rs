//! The event loop's decision half: keys in, damage out. No terminal, no IO.
//!
//! Keys resolve to `editor_core::Command`, so the binding table is the only
//! place that knows about crossterm and the palette will reach the same
//! actions. This owns the viewport and which rows a frame must repaint; it
//! never owns the text, which lives in one `Document`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use editor_control::{EditorStateWire, WireEdit, WireMark, WireMarkKind};
use editor_core::{
    execute, metrics, Command, Document, Edit, Grammar, Origin, Query, Range, Refusal, Selection,
    Snapshot, Syntax, Transaction,
};

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

/// Largest copy that travels as an OSC 52.
///
/// Matches `terminal_core::MAX_CLIPBOARD_BYTES`: the terminal drops anything
/// over it, so sending more would be bytes through a PTY for nothing.
const MAX_CLIPBOARD_BYTES: usize = 64 * 1024;

/// How long the buffer has to stand still before an autosave fires.
///
/// The same second the GUI editor waits. Shorter and a burst of typing becomes
/// a burst of writes; longer and "it saves by itself" stops being true.
const AUTOSAVE_PAUSE: Duration = Duration::from_secs(1);

/// Past this many bytes a buffer is shown without colour.
///
/// Matches the GUI editor's cap for the same reason: colouring is a linear
/// pass on every mutation, and past a point a correct colour is not worth a
/// frame.
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;

pub struct App {
    document: Document,
    path: PathBuf,
    revision: Option<String>,
    /// True when the daemon owns this buffer over the control channel: the
    /// local disk adapter is off and a save travels as a request. False is the
    /// standalone editor, which saves through `disk`.
    integrated: bool,
    /// The grammar this buffer is coloured with, from its path.
    grammar: Grammar,
    /// Gutter marks by 1-based line, as the daemon last computed them. Empty
    /// standalone: this editor never runs git of its own.
    marks: BTreeMap<usize, WireMarkKind>,
    /// An OSC 52 the renderer has not written yet, if a copy just happened.
    clipboard_escape: Option<String>,
    /// Save on a pause. The opener's preference; off standalone.
    autosave: bool,
    /// When the pause is up, if one is running. Cleared by the save it fires.
    autosave_at: Option<Instant>,
    /// Spans per line, rescanned when the document changes and never per row:
    /// a block comment opened above decides the colour of everything below it,
    /// so the scan cannot be limited to what is on screen.
    syntax: Syntax,
    /// The save this editor asked the daemon for and has not heard back on,
    /// with the snapshot it captured. One at a time: a second `Ctrl-S` while
    /// one is in flight would confirm the wrong state.
    in_flight_save: Option<(u64, Snapshot)>,
    /// The save request the control loop has not sent yet.
    outbox: Option<(u64, String, u64)>,
    next_request_id: u64,
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
        let grammar = Grammar::for_path(&path.to_string_lossy());
        let mut app = Self::blank(document, path, revision);
        app.grammar = grammar;
        app.rescan();
        app
    }

    fn blank(document: Document, path: PathBuf, revision: Option<String>) -> Self {
        Self {
            document,
            path,
            revision,
            integrated: false,
            grammar: Grammar::None,
            marks: BTreeMap::new(),
            clipboard_escape: None,
            autosave: false,
            autosave_at: None,
            syntax: Syntax::default(),
            in_flight_save: None,
            outbox: None,
            // The daemon mints ids from 1 for its own requests; the editor's
            // start past them so a log line names one side unambiguously.
            next_request_id: 1_000,
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

    /// Line-number column, the mark column, and the space after them.
    ///
    /// The mark column is always there, marks or not: a gutter that widens the
    /// first time git answers would shift every line of the file sideways.
    pub fn gutter_width(&self) -> usize {
        self.number_width() + 2
    }

    /// The OSC 52 a copy left for the renderer to write. Take-and-clear.
    pub fn take_clipboard_escape(&mut self) -> Option<String> {
        self.clipboard_escape.take()
    }

    /// Turn saving-on-a-pause on or off.
    pub fn set_autosave(&mut self, autosave: bool) {
        self.autosave = autosave;
        if !autosave {
            self.autosave_at = None;
        }
    }

    /// When the main loop should wake to autosave, if it should.
    #[must_use]
    pub fn autosave_deadline(&self) -> Option<Instant> {
        self.autosave_at
    }

    /// Fire a due autosave. A no-op when the pause has not elapsed.
    ///
    /// Re-checked here rather than trusted from the caller's timer: a
    /// keystroke between the wake-up and this call pushes the pause out again,
    /// which is the whole point of waiting for stillness.
    pub fn autosave_if_due(&mut self) {
        let Some(at) = self.autosave_at else { return };
        if Instant::now() < at {
            return;
        }
        self.autosave_at = None;
        if self.document.is_dirty() && !self.document.is_read_only() {
            self.save();
        }
    }

    /// Restart the pause, because the document just changed.
    fn arm_autosave(&mut self) {
        if self.autosave && self.document.is_dirty() && !self.document.is_read_only() {
            self.autosave_at = Some(Instant::now() + AUTOSAVE_PAUSE);
        }
    }

    /// Replace every gutter mark. Whole, never a delta (see the wire's note).
    pub fn set_marks(&mut self, marks: &[WireMark]) {
        self.marks = marks
            .iter()
            .map(|mark| (mark.line as usize, mark.kind))
            .collect();
        self.damage_all = true;
    }

    /// The mark on a 1-based line, if it has one.
    #[must_use]
    pub fn mark_at(&self, line: usize) -> Option<WireMarkKind> {
        self.marks.get(&line).copied()
    }

    /// Move to the next changed block in `step`'s direction.
    ///
    /// Blocks, not lines: a run of marked lines is one change to a reader, and
    /// stepping through it line by line would be a worse `Ctrl-N`.
    pub fn goto_change(&mut self, step: isize) {
        let here = self.caret_position().0;
        let starts: Vec<usize> = self
            .marks
            .keys()
            .copied()
            .filter(|line| !self.marks.contains_key(&(line - 1)))
            .collect();
        if starts.is_empty() {
            self.status = Some("no changes in this file".to_string());
            return;
        }
        let target = if step > 0 {
            starts.iter().find(|line| **line > here).or(starts.first())
        } else {
            starts
                .iter()
                .rev()
                .find(|line| **line < here)
                .or(starts.last())
        };
        if let Some(line) = target.copied() {
            self.goto_line(line);
        }
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

    /// This buffer belongs to the daemon: keep the local disk adapter off.
    ///
    /// A save becomes a request on the control channel instead of a write from
    /// here, because the daemon owns the checkout and holds the revision the
    /// write is conditioned on.
    pub fn set_integrated(&mut self) {
        self.integrated = true;
    }

    /// Take the save request the control loop has to send, if there is one.
    ///
    /// Returns `(request_id, text, document_version)`. The snapshot stays here
    /// until [`App::save_confirmed`] or [`App::save_refused`] answers.
    pub fn take_save_request(&mut self) -> Option<(u64, String, u64)> {
        self.outbox.take()
    }

    /// The daemon wrote the buffer: mark the captured snapshot saved.
    ///
    /// Confirms the *snapshot*, not the current text, so keystrokes that
    /// landed while the write was in flight leave the buffer dirty.
    pub fn save_confirmed(&mut self, request_id: u64, revision: String) {
        let Some((pending, snapshot)) = self.in_flight_save.take() else {
            return;
        };
        if pending != request_id {
            // Not ours: put it back rather than confirming a save that is
            // still waiting for its own answer.
            self.in_flight_save = Some((pending, snapshot));
            return;
        }
        self.document.confirm_save(&snapshot, revision);
        self.status = Some(if self.document.is_dirty() {
            "saved — newer keystrokes are still unsaved".to_string()
        } else {
            "saved".to_string()
        });
    }

    /// The daemon refused the write; the buffer is untouched and stays dirty.
    pub fn save_refused(&mut self, request_id: u64, reason: &str) {
        match self.in_flight_save.take() {
            Some((pending, snapshot)) if pending != request_id => {
                self.in_flight_save = Some((pending, snapshot));
                return;
            }
            _ => {}
        }
        self.status = Some(reason.to_string());
    }

    /// Move the viewport to a line, for the daemon's `Reveal` request.
    ///
    /// `column` is a 1-based display column, as the wire counts; `None` leaves
    /// the caret where the goto put it.
    pub fn reveal(&mut self, line: usize, column: Option<usize>) {
        self.goto_line(line);
        if let Some(column) = column {
            let position = self.document.caret_line_col();
            let text = self.document.text();
            let line_text = text.line(position.line);
            let target = column
                .saturating_sub(1)
                .min(metrics::display_column(line_text, line_text.len()));
            let offset = text.line_start(position.line)
                + metrics::byte_column_for_display(line_text, target);
            self.document.set_selection(Selection::caret(offset));
            self.ensure_visible();
        }
        self.damage_all = true;
    }

    /// State as the control channel reports it: path, position, dirty, version.
    ///
    /// The wire counts columns from 1 (`EditorStateWire`, `domain::EditorState`)
    /// while [`Self::caret_position`] returns the screen column the renderer
    /// places the caret at, counted from 0. Converting here keeps the whole
    /// wire 1-based in both directions: what [`Self::reveal`] accepts is what
    /// the next state reports back.
    pub fn wire_state(&self) -> EditorStateWire {
        let (line, column) = self.caret_position();
        EditorStateWire {
            path: self.path.to_string_lossy().into_owned(),
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(column + 1).unwrap_or(u32::MAX),
            dirty: self.document.is_dirty(),
            read_only: self.document.is_read_only(),
            document_version: self.document.version().0,
        }
    }

    /// Replace the buffer with what the daemon read from disk.
    ///
    /// A new document, not an edit: the draft is discarded because the person
    /// chose the other side, and there is no undo that gets back to text this
    /// buffer never had. The caret keeps its line where the new text is long
    /// enough for it.
    pub fn reload(&mut self, text: &str, revision: Option<String>) -> Result<u64, String> {
        let line = self.caret_position().0;
        let read_only = self.document.is_read_only();
        let document =
            Document::from_bytes(text.as_bytes(), read_only).map_err(|error| error.to_string())?;
        self.document = document;
        if let Some(revision) = revision {
            self.document.set_disk_revision(revision);
        }
        // A save that was in flight described text that is no longer here.
        self.in_flight_save = None;
        self.outbox = None;
        self.rescan();
        self.goto_line(line);
        self.damage_all = true;
        self.status = Some("reloaded from disk".to_string());
        Ok(self.document.version().0)
    }

    /// Send the buffer again, whatever the daemon last said about the disk.
    ///
    /// The *keep mine* gesture, arriving from the GUI instead of a key press:
    /// the daemon learned the disk's revision when it refused, so this write
    /// is the one that lands.
    pub fn request_save(&mut self) {
        self.save();
    }

    /// Apply a preview edit, refusing when the document moved under the caller.
    ///
    /// The version check and the apply share this one thread, so a keystroke
    /// cannot slip between them.
    pub fn apply_preview_edit(
        &mut self,
        edits: &[WireEdit],
        expected_document_version: u64,
    ) -> Result<u64, String> {
        let current = self.document.version().0;
        if current != expected_document_version {
            return Err(format!(
                "document is at version {current}, expected {expected_document_version}"
            ));
        }
        let before = self.document.selection();
        let edits = edits
            .iter()
            .map(|edit| Edit {
                range: Range::new(edit.from as usize, edit.to as usize),
                insert: edit.insert.clone(),
            })
            .collect();
        let transaction =
            Transaction::new(edits, Origin::Preview, before).map_err(|error| error.to_string())?;
        let applied = self
            .document
            .apply(transaction)
            .map_err(|error| error.to_string())?;
        self.damage_all = true;
        self.ensure_visible();
        Ok(applied.version.0)
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
            // The register is the editor's own scratch space; OSC 52 is how the
            // copy leaves it. Emitted even standalone, because the terminal a
            // person ran this in is the one that owns their clipboard.
            self.clipboard_escape = osc52(&text);
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

    /// Re-read the document's colouring.
    ///
    /// One linear pass, on a mutation and never on a cursor move. Past the cap
    /// the buffer is shown plain: a scan on every keystroke is not worth a
    /// frame once a file is large enough, and the renderer treats an empty
    /// `Syntax` as plain text without a branch of its own.
    fn rescan(&mut self) {
        self.syntax = if self.document.text().len() > MAX_HIGHLIGHT_BYTES {
            Syntax::default()
        } else {
            Syntax::parse(self.document.as_str(), self.grammar)
        };
    }

    /// The colouring of one 0-based line, for the renderer.
    #[must_use]
    pub fn syntax_line(&self, line: usize) -> &[editor_core::Span] {
        self.syntax.line(line)
    }

    /// Damage the rows an outcome can have changed.
    fn mark(&mut self, before: Selection, outcome: &editor_core::Outcome) {
        if outcome.applied.is_some() {
            self.rescan();
            self.arm_autosave();
        }
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
            EditorAction::NextChange => self.goto_change(1),
            EditorAction::PreviousChange => self.goto_change(-1),
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
        if self.integrated {
            // The daemon owns the checkout: never stat or write it from here.
            // The request carries the text; the answer carries the revision.
            if self.in_flight_save.is_some() {
                self.status = Some("a save is already in flight".to_string());
                return;
            }
            let snapshot = self.document.snapshot();
            let request_id = self.next_request_id;
            self.next_request_id += 1;
            self.outbox = Some((
                request_id,
                snapshot.text().to_string(),
                snapshot.version().0,
            ));
            self.in_flight_save = Some((request_id, snapshot));
            self.status = Some("saving…".to_string());
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
            // Alt, not Ctrl: Ctrl-N and Ctrl-B are already the find bar's.
            (KeyCode::Char('n'), _, true) => Action::Editor(EditorAction::NextChange),
            (KeyCode::Char('p'), _, true) => Action::Editor(EditorAction::PreviousChange),
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
    /// Jump to the next / previous changed block in the gutter.
    NextChange,
    PreviousChange,
    Cancel,
}

/// `ESC ] 52 ; c ; <base64> BEL` — the clipboard *store*.
///
/// Hand-rolled rather than pulled in as a dependency: this is the only base64
/// in the editor, and the alphabet is four lines. Oversize is `None`, matching
/// the cap the terminal applies on the other side — a copy nobody can accept is
/// better dropped here than truncated there.
fn osc52(text: &str) -> Option<String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if text.len() > MAX_CLIPBOARD_BYTES {
        return None;
    }
    let bytes = text.as_bytes();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let packed = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for slot in 0..4 {
            if slot <= chunk.len() {
                let index = (packed >> (18 - 6 * slot)) & 0x3f;
                encoded.push(ALPHABET[index as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    Some(format!("\x1b]52;c;{encoded}\x07"))
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

    fn marked(app: &mut App, lines: &[(u32, WireMarkKind)]) {
        let marks: Vec<WireMark> = lines
            .iter()
            .map(|(line, kind)| WireMark {
                line: *line,
                kind: *kind,
            })
            .collect();
        app.set_marks(&marks);
    }

    #[test]
    fn a_copy_leaves_an_osc_52_for_the_terminal() {
        let mut app = app("hello", false);
        app.handle_key(shifted(KeyCode::Right));
        app.handle_key(shifted(KeyCode::Right));
        app.handle_key(control('c'));
        let escape = app.take_clipboard_escape().expect("a copy emits OSC 52");
        assert_eq!(escape, "\u{1b}]52;c;aGU=\u{7}");
        assert!(app.take_clipboard_escape().is_none(), "take-and-clear");
    }

    /// The base64 here is hand-rolled, so its padding is worth pinning.
    #[test]
    fn osc_52_encodes_every_padding_case() {
        assert_eq!(osc52("a").unwrap(), "\u{1b}]52;c;YQ==\u{7}");
        assert_eq!(osc52("ab").unwrap(), "\u{1b}]52;c;YWI=\u{7}");
        assert_eq!(osc52("abc").unwrap(), "\u{1b}]52;c;YWJj\u{7}");
        assert_eq!(osc52("").unwrap(), "\u{1b}]52;c;\u{7}");
        // Bytes past ASCII, and the `+` and `/` an off-by-one in the alphabet
        // would miss — the two characters base64 puts at indexes 62 and 63.
        assert_eq!(osc52("é").unwrap(), "\u{1b}]52;c;w6k=\u{7}");
        assert_eq!(osc52("~~~").unwrap(), "\u{1b}]52;c;fn5+\u{7}");
        assert_eq!(
            osc52("\u{7f}\u{7f}\u{7f}").unwrap(),
            "\u{1b}]52;c;f39/\u{7}"
        );
    }

    /// Over the cap is dropped, not truncated: the terminal would refuse it.
    #[test]
    fn an_oversize_copy_emits_nothing() {
        assert!(osc52(&"a".repeat(MAX_CLIPBOARD_BYTES + 1)).is_none());
        assert!(osc52(&"a".repeat(MAX_CLIPBOARD_BYTES)).is_some());
    }

    #[test]
    fn autosave_arms_on_a_change_and_only_when_it_is_on() {
        let mut app = app("hello", false);
        app.set_integrated();
        // Off by default: a buffer must not start writing itself.
        app.handle_key(key(KeyCode::Char('!')));
        assert!(app.autosave_deadline().is_none());

        app.set_autosave(true);
        app.handle_key(key(KeyCode::Char('?')));
        let armed = app.autosave_deadline().expect("a change arms the pause");
        assert!(armed > Instant::now(), "the pause is in the future");

        // Turning it off disarms what was already running.
        app.set_autosave(false);
        assert!(app.autosave_deadline().is_none());
    }

    /// The deadline is re-checked when it fires: a keystroke in between pushes
    /// the pause out, which is the whole point of waiting for stillness.
    #[test]
    fn an_autosave_that_is_not_due_does_not_fire() {
        let mut app = app("hello", false);
        app.set_integrated();
        app.set_autosave(true);
        app.handle_key(key(KeyCode::Char('!')));
        app.autosave_if_due();
        assert!(
            app.take_save_request().is_none(),
            "the pause has not elapsed"
        );
        assert!(app.autosave_deadline().is_some(), "and it is still armed");
    }

    /// A read-only buffer never arms one, so it can never write itself.
    #[test]
    fn a_read_only_buffer_never_autosaves() {
        let mut app = app("hello", true);
        app.set_integrated();
        app.set_autosave(true);
        app.handle_key(key(KeyCode::Char('!')));
        assert!(app.autosave_deadline().is_none());
    }

    #[test]
    fn gutter_marks_are_stored_by_line_and_replaced_whole() {
        let mut app = app("a\nb\nc\n", false);
        marked(
            &mut app,
            &[(1, WireMarkKind::Added), (3, WireMarkKind::Deleted)],
        );
        assert_eq!(app.mark_at(1), Some(WireMarkKind::Added));
        assert_eq!(app.mark_at(2), None);
        assert_eq!(app.mark_at(3), Some(WireMarkKind::Deleted));

        // Whole, not merged: a stale mark must not survive the next answer.
        marked(&mut app, &[(2, WireMarkKind::Modified)]);
        assert_eq!(app.mark_at(1), None);
        assert_eq!(app.mark_at(2), Some(WireMarkKind::Modified));
    }

    /// The gutter keeps its width whether or not git has answered, or every
    /// line of the file shifts sideways the moment it does.
    #[test]
    fn the_gutter_does_not_widen_when_marks_arrive() {
        let mut app = app("a\nb\nc\n", false);
        let before = app.gutter_width();
        marked(&mut app, &[(1, WireMarkKind::Added)]);
        assert_eq!(app.gutter_width(), before);
    }

    /// A run of marked lines is one change, so stepping lands on its start.
    #[test]
    fn change_navigation_steps_blocks_and_wraps() {
        let mut app = app("1\n2\n3\n4\n5\n6\n", false);
        marked(
            &mut app,
            &[
                (2, WireMarkKind::Added),
                (3, WireMarkKind::Added),
                (5, WireMarkKind::Modified),
            ],
        );
        app.goto_line(1);
        app.goto_change(1);
        assert_eq!(app.caret_position().0, 2, "the first block's start");
        app.goto_change(1);
        assert_eq!(app.caret_position().0, 5, "past the run, not into it");
        app.goto_change(1);
        assert_eq!(app.caret_position().0, 2, "wraps to the first");
        app.goto_change(-1);
        assert_eq!(app.caret_position().0, 5, "wraps backwards to the last");
    }

    #[test]
    fn change_navigation_says_so_when_there_is_nothing_to_step_to() {
        let mut app = app("a\nb\n", false);
        app.goto_change(1);
        assert_eq!(app.caret_position().0, 1);
        assert_eq!(app.status(), Some("no changes in this file"));
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

    #[test]
    fn wire_state_reports_path_position_and_version() {
        let mut app = app("hello\nworld", false);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char('!')));
        let state = app.wire_state();
        assert_eq!(state.path, "fixture.txt");
        assert_eq!(state.line, 2);
        // The caret sits after the typed `!`: screen column 1, wire column 2.
        assert_eq!(state.column, 2);
        assert_eq!(app.caret_position(), (2, 1));
        assert!(state.dirty);
        assert!(!state.read_only);
        assert_eq!(state.document_version, app.document().version().0);
    }

    #[test]
    fn one_state_shape_for_notifications_and_requests() {
        // Notifications and GetState share `wire_state`; a second call without
        // a mutation must be identical, so a snapshot cannot drift from the
        // last published state.
        let mut app = app("hello\nworld", true);
        app.reveal(2, Some(1));
        let notified = app.wire_state();
        let requested = app.wire_state();
        assert_eq!(notified, requested);
        assert_eq!(notified.line, 2);
        assert_eq!(notified.column, 1);
        assert!(notified.read_only);
    }

    #[test]
    fn control_requests_apply_on_the_event_thread() {
        // Reveal is the same serialized `App` mutation a keystroke uses. The
        // control reader never touches `Document` itself.
        let mut app = app("alpha\nbeta\ngamma\n", false);
        app.reveal(3, Some(2));
        // `reveal` takes the wire's 1-based column and `wire_state` reports it
        // back unchanged; `caret_position` is the renderer's 0-based screen
        // column for the same caret.
        let state = app.wire_state();
        assert_eq!((state.line, state.column), (3, 2));
        assert_eq!(app.caret_position(), (3, 1));
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.document().as_str().lines().nth(2), Some("gxamma"));
    }

    #[test]
    fn reveal_moves_the_caret_to_a_line_and_display_column() {
        // `é` is one grapheme over two bytes, so the column the wire counts is
        // a display column, not a byte offset.
        let mut app = app("alpha\nbeta\né_char\n", false);
        app.reveal(3, Some(3));
        assert_eq!(app.caret_position(), (3, 2));
    }

    /// An integrated save never touches the disk from here: it leaves a request
    /// for the control loop and waits for the daemon's revision.
    #[test]
    fn an_integrated_save_asks_the_daemon_instead_of_writing() {
        let mut app = app("hello", false);
        app.set_integrated();
        app.handle_key(key(KeyCode::Char('!')));
        assert!(app.document().is_dirty());

        app.save();
        let (request_id, text, version) = app.take_save_request().expect("a save request");
        assert_eq!(text, "!hello");
        assert_eq!(version, app.document().version().0);
        assert_eq!(app.status(), Some("saving…"));
        assert!(
            app.take_save_request().is_none(),
            "the request is taken once"
        );

        app.save_confirmed(request_id, "rev-2".to_string());
        assert!(
            !app.document().is_dirty(),
            "the confirmed snapshot is saved"
        );
        assert_eq!(app.status(), Some("saved"));
        assert_eq!(app.document().disk_revision(), Some("rev-2"));
    }

    /// A keystroke that lands while the write is in flight is not covered by
    /// the answer: the snapshot is confirmed, the buffer stays dirty.
    #[test]
    fn a_keystroke_during_an_integrated_save_keeps_the_buffer_dirty() {
        let mut app = app("hello", false);
        app.set_integrated();
        app.save();
        let (request_id, _, _) = app.take_save_request().expect("a save request");
        app.handle_key(key(KeyCode::Char('!')));
        app.save_confirmed(request_id, "rev-2".to_string());
        assert!(app.document().is_dirty());
        assert_eq!(
            app.status(),
            Some("saved — newer keystrokes are still unsaved")
        );
    }

    /// A refusal leaves the buffer alone and says why.
    #[test]
    fn a_refused_integrated_save_keeps_the_buffer() {
        let mut app = app("hello", false);
        app.set_integrated();
        app.handle_key(key(KeyCode::Char('!')));
        app.save();
        let (request_id, _, _) = app.take_save_request().expect("a save request");
        app.save_refused(request_id, "the file changed on disk");
        assert_eq!(app.status(), Some("the file changed on disk"));
        assert!(app.document().is_dirty(), "a refusal is not a save");
        assert_eq!(app.document().as_str(), "!hello");
    }

    /// A read-only integrated buffer refuses before it asks.
    #[test]
    fn a_read_only_integrated_buffer_sends_no_save_request() {
        let mut app = app("hello", true);
        app.set_integrated();
        app.save();
        assert!(app.take_save_request().is_none());
        assert_eq!(app.status(), Some(describe(Refusal::ReadOnly).as_str()));
    }

    /// One save at a time: a second `Ctrl-S` while one is in flight would
    /// confirm the wrong snapshot when the first answer lands.
    #[test]
    fn a_second_integrated_save_waits_for_the_first() {
        let mut app = app("hello", false);
        app.set_integrated();
        app.save();
        let (first, _, _) = app.take_save_request().expect("a save request");
        app.save();
        assert!(app.take_save_request().is_none(), "no second request");
        assert_eq!(app.status(), Some("a save is already in flight"));
        app.save_confirmed(first, "rev-2".to_string());
        app.save();
        assert!(
            app.take_save_request().is_some(),
            "the next save goes out once the first is answered"
        );
    }

    #[test]
    fn a_preview_edit_applies_and_reports_its_version() {
        let mut app = app("hello", false);
        let before = app.wire_state().document_version;
        let edits = vec![WireEdit {
            from: 0,
            to: 5,
            insert: "HELLO".into(),
        }];
        let after = app.apply_preview_edit(&edits, before).unwrap();
        assert_eq!(after, before + 1);
        assert_eq!(app.document().as_str(), "HELLO");
        // Undo walks the preview out like any other transaction.
        app.handle_key(control('z'));
        assert_eq!(app.document().as_str(), "hello");
    }

    #[test]
    fn a_preview_edit_refuses_a_stale_version() {
        let mut app = app("hello", false);
        let version = app.wire_state().document_version;
        app.handle_key(key(KeyCode::Char('X')));
        let edits: Vec<WireEdit> = vec![];
        let error = app
            .apply_preview_edit(&edits, version)
            .expect_err("stale version");
        assert!(error.contains("version"), "{error}");
        assert_eq!(app.document().as_str(), "Xhello");
    }

    #[test]
    fn the_control_budget_matches_the_core_budget() {
        assert_eq!(
            editor_control::MAX_DOCUMENT_BYTES,
            editor_core::limits::MAX_DOCUMENT_BYTES
        );
    }
}
