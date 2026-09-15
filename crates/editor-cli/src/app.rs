//! The event loop's decision half: keys in, damage out. No terminal, no IO.
//!
//! Keys resolve to `editor_core::Command`, so the binding table is the only
//! place that knows about crossterm and the palette will reach the same
//! actions. This owns the viewport and which rows a frame must repaint; it
//! never owns the text, which lives in one `Document`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use editor_control::{EditorStateWire, WireEdit, WireMark, WireMarkKind};
use editor_core::{
    execute, metrics, Command, Document, Edit, Grammar, Origin, Query, Range, Refusal, Selection,
    Snapshot, Syntax, Transaction,
};

use crate::disk;
use crate::view;

/// Rows a frame has to repaint.
pub struct Frame {
    /// Every row of the viewport when it moved or the document changed under
    /// it; only the damaged rows otherwise.
    pub rows: Vec<usize>,
    /// Whether the grid changed shape since the last frame.
    ///
    /// Only a resize needs the clear: a scrolled viewport repaints every row
    /// anyway, and blanking first is what makes a client reading the delta
    /// between two writes paint an empty screen.
    pub clear: bool,
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
    /// Type an opener and get its partner. On by default, off for the person
    /// who would rather type both, and off while a macro-ish paste runs.
    close_brackets: bool,
    /// Gutter marks by 1-based line, as the daemon last computed them. Empty
    /// standalone: this editor never runs git of its own.
    marks: BTreeMap<usize, WireMarkKind>,
    /// Header lines of the folded blocks, 0-based.
    folded: BTreeSet<usize>,
    /// Foldable regions, recomputed when the text changes and never per row.
    regions: Vec<editor_core::fold::Region>,
    /// Where the pointer went down, for a rectangular drag.
    drag_origin: Option<(u16, u16)>,
    /// An OSC 52 the renderer has not written yet, if a copy just happened.
    clipboard_escape: Option<String>,
    /// Save on a pause. The opener's preference; off standalone.
    autosave: bool,
    autosave_suspended: bool,
    close_after_save: bool,
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
    /// Whether the live query paints marks. Escape clears this and keeps the
    /// pattern, so `Ctrl-N` still works and no marks are left behind.
    highlight: bool,
    /// Why the live pattern cannot be searched with, when it cannot. Only a
    /// regular expression can be invalid.
    query_error: Option<String>,
    /// Where find-as-you-type searches from. Fixed when the prompt opens, so
    /// adding a character narrows the same match instead of walking forward.
    find_origin: usize,
    /// Decorations for the visible rows, and what they were computed against.
    decorations: Vec<(usize, view::Mark)>,
    decor_key: Option<DecorKey>,
    /// The overview ruler's column, one entry per screen row.
    ruler: Vec<Option<RulerMark>>,
    prompt: Option<Prompt>,
    help: bool,
    status: Option<String>,
    top: usize,
    /// Wrap segment of `top` the first row shows. Always 0 without wrap, and
    /// non-zero only for a logical line taller than the viewport — without it
    /// the tail of such a line is unreachable.
    top_sub: usize,
    left: usize,
    /// Wrap long lines onto continuation rows instead of scrolling sideways.
    /// On under the daemon, where the pane is a fixed width and the files are
    /// as often prose as code; off standalone, where a terminal user can widen
    /// the window and a code file reads better unwrapped. When on, `left` stays
    /// 0 — there is nothing to scroll horizontally past.
    wrap: bool,
    width: u16,
    height: u16,
    quit: bool,
    damaged: BTreeSet<usize>,
    damage_all: bool,
    /// Shape the last frame was painted for; a change invalidates everything.
    painted: Option<Painted>,
}

/// What one row of the overview ruler shows.
///
/// Ordered by which wins the cell: a row can hold a change, a match and a caret
/// at once, and the caret is the one a person is looking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RulerMark {
    Change,
    Match,
    Caret,
}

/// What the visible decorations were computed against.
///
/// Every input is here, so a mismatch is the whole recompute trigger: nothing
/// derives them per frame, which is the rule `docs/performance.md` states.
#[derive(PartialEq, Eq)]
struct DecorKey {
    version: u64,
    selection: Range,
    caret: usize,
    top: usize,
    top_sub: usize,
    height: usize,
    width: u16,
    /// `None` when Escape cleared the marks; the pattern itself stays.
    query: Option<Query>,
}

/// Largest slice of the buffer decorations are looked for in.
///
/// The viewport bounds the rows, not the bytes: one logical line can be taller
/// than the screen, and its `line_end` is as far away as the document allows.
const MAX_DECORATION_WINDOW: usize = 64 * 1024;

/// Narrowest grid that still gets an overview ruler.
///
/// One cell of the text column buys the whole document's shape, but not at the
/// width where the text column is already the problem.
const MIN_RULER_WIDTH: u16 = 40;

/// Longest selection that marks its own other occurrences.
///
/// A word, not a paragraph: `highlightSelectionMatches` is for seeing where a
/// symbol else appears, and a multi-line selection has no siblings to find.
const MAX_SELECTION_MATCH_BYTES: usize = 128;

/// What the last frame was painted against.
///
/// The row heights are the half that a wrapped buffer needs: an edit that
/// changes how many rows a line takes reflows every row under it, and comparing
/// against what is already on screen is the only way to tell that from an edit
/// that just changed some characters.
struct Painted {
    top: usize,
    top_sub: usize,
    left: usize,
    height: usize,
    width: u16,
    /// Screen rows each visible line occupied, from `top`. Empty without wrap.
    rows_per_line: Vec<usize>,
}

impl App {
    pub fn new(document: Document, path: PathBuf, revision: Option<String>) -> Self {
        let grammar = Grammar::for_path(&path.to_string_lossy());
        let mut app = Self::blank(document, path, revision);
        app.grammar = grammar;
        app.adopt_input_style();
        app.rescan();
        app
    }

    /// Tell the document how this file indents, and what its blocks are made of.
    ///
    /// Read off the buffer and the path here rather than in the core: the core
    /// owns no path, and the answer changes only when the text is replaced.
    fn adopt_input_style(&mut self) {
        let indent = editor_core::indent::detect(self.document.text());
        self.document.set_input_style(editor_core::InputStyle {
            indent,
            close_brackets: self.close_brackets,
            grammar: self.grammar,
        });
    }

    fn blank(document: Document, path: PathBuf, revision: Option<String>) -> Self {
        Self {
            document,
            path,
            revision,
            integrated: false,
            grammar: Grammar::None,
            close_brackets: true,
            marks: BTreeMap::new(),
            folded: BTreeSet::new(),
            regions: Vec::new(),
            drag_origin: None,
            clipboard_escape: None,
            autosave: false,
            autosave_suspended: false,
            close_after_save: false,
            autosave_at: None,
            syntax: Syntax::default(),
            in_flight_save: None,
            outbox: None,
            // The daemon mints ids from 1 for its own requests; the editor's
            // start past them so a log line names one side unambiguously.
            next_request_id: 1_000,
            register: String::new(),
            query: Query::literal(""),
            highlight: false,
            query_error: None,
            find_origin: 0,
            decorations: Vec::new(),
            decor_key: None,
            ruler: Vec::new(),
            prompt: None,
            help: false,
            status: None,
            top: 0,
            top_sub: 0,
            left: 0,
            wrap: false,
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

    /// The find bar's live modifiers, as the prompt row shows them.
    ///
    /// Only what is *on* is named: a row that always said "case:off word:off
    /// regex:off" would be three words of chrome for the default state.
    #[must_use]
    pub fn query_flags(&self) -> String {
        if let Some(error) = &self.query_error {
            return format!("  [{error}]");
        }
        let mut on = Vec::new();
        if self.query.case_sensitive {
            on.push("case");
        }
        if self.query.whole_word {
            on.push("word");
        }
        if self.query.regex {
            on.push("regex");
        }
        if on.is_empty() {
            String::new()
        } else {
            format!("  [{}]", on.join(" "))
        }
    }

    pub fn help_visible(&self) -> bool {
        self.help
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The first visible line. Only the tests read it now that the renderer
    /// walks rows through `row_line_sub`.
    #[cfg(test)]
    pub fn top(&self) -> usize {
        self.top
    }

    #[cfg(test)]
    pub fn top_sub(&self) -> usize {
        self.top_sub
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
        if self.integrated && !self.needs_status_row() {
            self.height as usize
        } else {
            (self.height as usize).saturating_sub(1)
        }
    }

    /// Whether the last row belongs to the status/prompt bar this frame.
    ///
    /// Standalone always keeps it: the idle bar is the only place the path,
    /// position and `F1 help` live. Integrated leaves that chrome to the GUI
    /// pane, so the row is claimed only for a prompt or a transient message —
    /// the two things the HTML around us cannot show — and reclaimed as a
    /// content row the rest of the time.
    pub fn needs_status_row(&self) -> bool {
        !self.integrated || self.prompt.is_some() || self.status.is_some()
    }

    pub fn number_width(&self) -> usize {
        self.document.text().line_count().max(1).to_string().len()
    }

    /// Line-number column, the mark column, the fold column, and the space
    /// after them.
    ///
    /// Every column is always there, marks or not: a gutter that widens the
    /// first time git answers would shift every line of the file sideways.
    pub fn gutter_width(&self) -> usize {
        self.number_width() + 3
    }

    /// How many lines the fold on `line` is hiding.
    #[must_use]
    pub fn folded_line_count(&self, line: usize) -> usize {
        self.regions
            .iter()
            .find(|region| region.header == line)
            .map_or(0, |region| region.last - region.header)
    }

    /// Whether a screen column falls in the fold marker's cell.
    fn is_fold_column(&self, col: u16) -> bool {
        col as usize == self.number_width() + 1
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
        } else {
            self.arm_autosave();
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
        if !self.autosave_suspended && self.document.is_dirty() && !self.document.is_read_only() {
            self.save();
        }
    }

    /// Restart the pause, because the document just changed.
    fn arm_autosave(&mut self) {
        if self.autosave
            && !self.autosave_suspended
            && self.document.is_dirty()
            && !self.document.is_read_only()
        {
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
        let ruler = usize::from(self.ruler_visible());
        (self.width as usize)
            .saturating_sub(self.gutter_width())
            .saturating_sub(ruler)
    }

    /// Whether the rightmost column is the overview ruler.
    ///
    /// A column and not an HTML strip: the editor already holds the marks, the
    /// matches and the caret, while the GUI holds none of them — a DOM ruler
    /// would mean a new broadcast carrying every changed line of the file on a
    /// channel that exists for a caret position.
    #[must_use]
    pub fn ruler_visible(&self) -> bool {
        self.width >= MIN_RULER_WIDTH && self.document.text().line_count() > 1
    }

    /// The ruler's screen column.
    #[must_use]
    pub fn ruler_column(&self) -> usize {
        (self.width as usize).saturating_sub(1)
    }

    /// What the ruler shows on `row`, strongest signal first.
    #[must_use]
    pub fn ruler_at(&self, row: usize) -> Option<RulerMark> {
        self.ruler.get(row).copied().flatten()
    }

    /// The 1-based line a click on the ruler's `row` means.
    #[must_use]
    pub fn ruler_line(&self, row: usize) -> usize {
        let total = self.document.text().line_count().max(1);
        let height = self.content_height().max(1);
        (row * total / height).min(total - 1) + 1
    }

    /// Recompute the ruler's column of marks.
    ///
    /// One pass over the document's *signals*, not its rows: the marks are a
    /// map the daemon already sent, and the matches are a capped scan that only
    /// runs while the find bar is open.
    fn refresh_ruler(&mut self, height: usize) {
        if !self.ruler_visible() || height == 0 {
            self.ruler.clear();
            return;
        }
        let total = self.document.text().line_count().max(1);
        let bucket = |line: usize| (line * height / total).min(height - 1);
        let mut column = vec![None; height];
        let mut put = |at: usize, what: RulerMark| {
            let slot = &mut column[at];
            if slot.is_none_or(|current| what > current) {
                *slot = Some(what);
            }
        };
        for line in self.marks.keys() {
            put(bucket(line.saturating_sub(1)), RulerMark::Change);
        }
        if self.highlight && !self.query.is_empty() && self.query_error.is_none() {
            let found = editor_core::find_all(self.document.text(), &self.query);
            let text = self.document.text();
            for hit in found.found {
                put(
                    bucket(text.line_of_offset(hit.range.start)),
                    RulerMark::Match,
                );
            }
        }
        for cursor in self.document.selection().cursors() {
            put(
                bucket(self.document.text().line_of_offset(cursor.head)),
                RulerMark::Caret,
            );
        }
        self.ruler = column;
    }

    /// Whether long lines wrap onto continuation rows.
    pub fn wrap(&self) -> bool {
        self.wrap
    }

    /// How many screen rows a 0-based logical line occupies.
    ///
    /// One unless wrapping is on and the line is wider than the text column, in
    /// which case it is split into as many rows as it takes. Always at least
    /// one, so an empty line is still a row.
    pub fn line_rows(&self, line: usize) -> usize {
        if self.is_hidden(line) {
            return 0;
        }
        if !self.wrap {
            return 1;
        }
        let text = self.document.text();
        if line >= text.line_count() {
            return 1;
        }
        let width = self.content_width().max(1);
        let cells = metrics::display_column(text.line(line), text.line(line).len());
        cells.div_ceil(width).max(1)
    }

    /// The `(line, sub)` shown at screen `row`, or `None` past the last line.
    ///
    /// `sub` is the wrapped segment within the line — 0 is its first row. Walked
    /// from `top`, which always sits at a line's first segment, so the walk is
    /// bounded by the viewport height.
    pub fn row_line_sub(&self, row: usize) -> Option<(usize, usize)> {
        let count = self.document.text().line_count();
        if !self.wrap {
            if self.folded.is_empty() {
                let line = self.top + row;
                return (line < count).then_some((line, 0));
            }
            let mut remaining = row;
            for line in self.top..count {
                if self.is_hidden(line) {
                    continue;
                }
                if remaining == 0 {
                    return Some((line, 0));
                }
                remaining -= 1;
            }
            return None;
        }
        let mut remaining = row;
        let mut line = self.top;
        // The anchor line may start part-way down: `top_sub` is the segment the
        // first row shows, so that line contributes only the rows below it.
        let mut skip = self.top_sub;
        while line < count {
            let rows = self.line_rows(line).saturating_sub(skip);
            if rows > 0 && remaining < rows {
                return Some((line, skip + remaining));
            }
            remaining -= rows;
            line += 1;
            skip = 0;
        }
        None
    }

    /// The screen row a line's segment lands on, or `None` when it is scrolled
    /// out of the viewport.
    fn line_sub_to_row(&self, target: usize, sub: usize) -> Option<usize> {
        if target < self.top {
            return None;
        }
        let height = self.content_height();
        if !self.wrap {
            if self.is_hidden(target) {
                return None;
            }
            if self.folded.is_empty() {
                let row = target - self.top;
                return (row < height).then_some(row);
            }
            let row = (self.top..target)
                .filter(|line| !self.is_hidden(*line))
                .count();
            return (row < height).then_some(row);
        }
        if target == self.top && sub < self.top_sub {
            return None;
        }
        let mut row = 0;
        let mut skip = self.top_sub;
        for line in self.top..target {
            row += self.line_rows(line).saturating_sub(skip);
            skip = 0;
            if row >= height {
                return None;
            }
        }
        let row = row + sub - skip;
        (row < height).then_some(row)
    }

    /// The last logical line with any row on screen, for a wheel scroll's caret
    /// clamp.
    fn last_visible_line(&self) -> usize {
        let count = self.document.text().line_count().saturating_sub(1);
        if !self.wrap {
            return (self.top + self.content_height().saturating_sub(1)).min(count);
        }
        let height = self.content_height().max(1);
        let mut rows = 0;
        let mut line = self.top;
        let mut skip = self.top_sub;
        while line < count {
            rows += self.line_rows(line).saturating_sub(skip);
            skip = 0;
            if rows >= height {
                break;
            }
            line += 1;
        }
        line.min(count)
    }

    /// Screen rows each visible line occupies, from `top`, clipped to `height`.
    ///
    /// The first entry is the anchor line's rows *below* `top_sub`, so the
    /// vector reads as what is on screen rather than what the lines are worth.
    fn visible_line_rows(&self, height: usize) -> Vec<usize> {
        let count = self.document.text().line_count();
        let mut out = Vec::new();
        let mut rows = 0;
        let mut line = self.top;
        let mut skip = self.top_sub;
        while line < count && rows < height {
            let taken = self.line_rows(line).saturating_sub(skip);
            out.push(taken);
            rows += taken;
            line += 1;
            skip = 0;
        }
        out
    }

    /// The caret's cell on screen — `(row, column)` — or `None` when it is
    /// scrolled out of view. The column includes the gutter.
    pub fn caret_screen(&self) -> Option<(usize, usize)> {
        let (line1, column) = self.caret_position();
        let line = line1 - 1;
        if self.wrap {
            let width = self.content_width().max(1);
            let row = self.line_sub_to_row(line, column / width)?;
            Some((row, self.gutter_width() + column % width))
        } else {
            let row = line.checked_sub(self.top)?;
            (row < self.content_height()).then_some(())?;
            Some((row, self.gutter_width() + column.saturating_sub(self.left)))
        }
    }

    /// Caret as a 1-based line and its display column.
    pub fn caret_position(&self) -> (usize, usize) {
        let position = self.document.caret_line_col();
        let column =
            metrics::display_column(self.document.text().line(position.line), position.column);
        (position.line + 1, column)
    }

    /// Every caret's selected range inside `line`, as offsets within it.
    ///
    /// One entry per caret that reaches this line, ascending. The end may sit
    /// one past the line's length: a selection that swallowed the line break
    /// has to look like it did.
    #[must_use]
    pub fn selections_in_line(&self, line: usize) -> Vec<(usize, usize)> {
        let text = self.document.text();
        let start = text.line_start(line);
        let end = text.line_end(line);
        self.document
            .selection()
            .cursors()
            .iter()
            .filter(|cursor| !cursor.is_empty())
            .filter_map(|cursor| {
                let range = cursor.range();
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
            })
            .collect()
    }

    /// Where each caret sits on `line`, as byte columns within it.
    ///
    /// The renderer paints the extra carets itself: a terminal has one hardware
    /// cursor, and the primary is the only one that can have it.
    #[must_use]
    pub fn carets_in_line(&self, line: usize) -> Vec<usize> {
        let text = self.document.text();
        let start = text.line_start(line);
        let end = text.line_end(line);
        let selection = self.document.selection();
        let primary = selection.head();
        selection
            .cursors()
            .iter()
            .map(|cursor| cursor.head)
            .filter(|head| *head != primary && *head >= start && *head <= end)
            .map(|head| head - start)
            .collect()
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
        // The pane is a fixed width the person cannot widen, so a long line has
        // to wrap or it is unreachable without a horizontal scroll the GUI does
        // not offer.
        self.wrap = true;
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
        self.autosave_suspended = false;
        if std::mem::take(&mut self.close_after_save) && !self.document.is_dirty() {
            self.quit = true;
        }
        self.arm_autosave();
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
        self.autosave_suspended = true;
        self.autosave_at = None;
        self.close_after_save = false;
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
        let visible = self.last_visible_line() + 1 - self.top;
        EditorStateWire {
            path: self.path.to_string_lossy().into_owned(),
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(column + 1).unwrap_or(u32::MAX),
            dirty: self.document.is_dirty(),
            read_only: self.document.is_read_only(),
            document_version: self.document.version().0,
            top_line: u32::try_from(self.top + 1).unwrap_or(u32::MAX),
            visible_lines: u32::try_from(visible.max(1)).unwrap_or(u32::MAX),
            total_lines: u32::try_from(self.document.text().line_count()).unwrap_or(u32::MAX),
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
        self.close_after_save = false;
        self.autosave_suspended = false;
        self.autosave_at = None;
        self.adopt_input_style();
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
        self.rescan_edited(applied);
        self.damage_all = true;
        self.ensure_visible();
        Ok(applied.version.0)
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width.max(1);
        self.height = height.max(1);
        // A narrower column gives the anchor line more rows, a wider one fewer;
        // a `top_sub` past the end would anchor the viewport on nothing.
        self.top_sub = self.top_sub.min(self.line_rows(self.top).saturating_sub(1));
        self.ensure_visible();
        self.damage_all = true;
    }

    /// Rows to repaint, and whether the grid changed shape.
    /// The decorations on one 0-based line, for the renderer.
    #[must_use]
    pub fn marks_in_line(&self, line: usize) -> Vec<view::Mark> {
        self.decorations
            .iter()
            .filter(|(at, _)| *at == line)
            .map(|(_, mark)| *mark)
            .collect()
    }

    /// Whether `line` holds the caret. The gutter reads brighter there.
    #[must_use]
    pub fn is_active_line(&self, line: usize) -> bool {
        self.document.caret_line_col().line == line
    }

    /// Recompute the visible decorations when one of their inputs moved, and
    /// damage every row that gained or lost one.
    fn refresh_decorations(&mut self, height: usize) {
        let key = DecorKey {
            version: self.document.version().0,
            selection: self.document.selection().range(),
            caret: self.document.caret(),
            top: self.top,
            top_sub: self.top_sub,
            height,
            width: self.width,
            query: (self.highlight && !self.query.is_empty() && self.query_error.is_none())
                .then(|| self.query.clone()),
        };
        if self.decor_key.as_ref() == Some(&key) {
            return;
        }
        let next = self.compute_decorations(&key);
        if next != self.decorations {
            let touched: BTreeSet<usize> = self
                .decorations
                .iter()
                .chain(next.iter())
                .map(|(line, _)| *line)
                .collect();
            self.damaged.extend(touched);
            self.decorations = next;
        }
        self.decor_key = Some(key);
    }

    fn compute_decorations(&self, key: &DecorKey) -> Vec<(usize, view::Mark)> {
        let text = self.document.text();
        let start = text.line_start(self.top);
        let end = text
            .line_end(self.last_visible_line())
            .min(start + MAX_DECORATION_WINDOW)
            .min(text.len());
        let window = Range::new(start, end.max(start));

        let mut ranges: Vec<(Range, view::Decoration)> = Vec::new();
        // The live query wins the row: while find is open, an unrelated word
        // under the caret marking its siblings would read as a second result.
        let occurrences = match &key.query {
            Some(query) => editor_core::find_in(text, query, window),
            None => self.selection_matches(window),
        };
        ranges.extend(
            occurrences
                .into_iter()
                .map(|range| (range, view::Decoration::Match)),
        );
        if let Some((open, close)) =
            editor_core::brackets::matching(text, &self.syntax, self.document.caret())
        {
            for at in [open, close] {
                if at >= window.start && at < window.end {
                    ranges.push((Range::new(at, at + 1), view::Decoration::Bracket));
                }
            }
        }

        let mut out = Vec::new();
        for (range, what) in ranges {
            let first = text.line_of_offset(range.start);
            let last = text.line_of_offset(range.end.saturating_sub(1).max(range.start));
            for line in first..=last {
                let line_start = text.line_start(line);
                let line_end = text.line_end(line);
                let from = range.start.max(line_start) - line_start;
                let to = range.end.min(line_end) - line_start;
                if to > from {
                    out.push((line, (from, to, what)));
                }
            }
        }
        out.sort_unstable_by_key(|(line, (from, _, _))| (*line, *from));
        out
    }

    /// Other literal occurrences of a short, single-line selection.
    ///
    /// CodeMirror's `highlightSelectionMatches`: select a symbol and see where
    /// else it is. The selection's own range is left out — it already reads as
    /// selected, and marking it too would say something the reverse video does
    /// not.
    fn selection_matches(&self, window: Range) -> Vec<Range> {
        let selection = self.document.selection();
        if selection.is_empty() {
            return Vec::new();
        }
        let range = selection.range();
        let text = self.document.text();
        if range.end - range.start > MAX_SELECTION_MATCH_BYTES
            || text.line_of_offset(range.start) != text.line_of_offset(range.end)
        {
            return Vec::new();
        }
        let needle = &text.as_str()[range.start..range.end];
        if needle.trim().is_empty() {
            return Vec::new();
        }
        // Literal, whatever the find bar is set to: this is "where else does
        // this text appear", and reading a selected `a.c` as a pattern would
        // mark things the person never asked about.
        let query = Query {
            pattern: needle.to_string(),
            case_sensitive: self.query.case_sensitive,
            whole_word: false,
            regex: false,
        };
        editor_core::find_in(text, &query, window)
            .into_iter()
            .filter(|found| *found != range)
            .collect()
    }

    pub fn take_frame(&mut self) -> Frame {
        let height = self.content_height();
        self.refresh_decorations(height);
        self.refresh_ruler(height);
        let rows_per_line = if self.wrap {
            self.visible_line_rows(height)
        } else {
            Vec::new()
        };
        let moved = self.painted.as_ref().is_none_or(|painted| {
            painted.top != self.top
                || painted.top_sub != self.top_sub
                || painted.left != self.left
                || painted.height != height
                || painted.width != self.width
        });
        // Only a narrower or wider grid needs the blank: every row a frame
        // paints clears itself first, so a viewport that only grew taller just
        // paints the rows it gained. Blanking is what makes a client reading the
        // delta between two writes paint an empty screen.
        let clear = self
            .painted
            .as_ref()
            .is_none_or(|painted| painted.width != self.width);
        let rows = if self.damage_all || moved {
            (0..height).collect()
        } else if self.wrap {
            self.reflowed_rows(&rows_per_line, height)
        } else {
            self.damaged
                .iter()
                .filter_map(|line| line.checked_sub(self.top))
                .filter(|row| *row < height)
                .collect()
        };
        self.damaged.clear();
        self.damage_all = false;
        self.painted = Some(Painted {
            top: self.top,
            top_sub: self.top_sub,
            left: self.left,
            height,
            width: self.width,
            rows_per_line,
        });
        Frame { rows, clear }
    }

    /// Rows a wrapped viewport must repaint for the damaged lines.
    ///
    /// A line that still takes the same number of rows repaints only its own;
    /// the first line whose height changed reflows the row↔line map under it,
    /// and from there down every row is somebody else's text now.
    fn reflowed_rows(&self, current: &[usize], height: usize) -> Vec<usize> {
        let previous = self
            .painted
            .as_ref()
            .map_or(&[][..], |painted| painted.rows_per_line.as_slice());
        let mut rows = Vec::new();
        let mut row = 0;
        for (index, taken) in current.iter().copied().enumerate() {
            if row >= height {
                break;
            }
            if previous.get(index).copied() != Some(taken) {
                rows.extend(row..height);
                break;
            }
            if self.damaged.contains(&(self.top + index)) {
                rows.extend(row..(row + taken).min(height));
            }
            row += taken;
        }
        rows
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

    /// Turn a mouse report into a caret move, a selection change, or a scroll.
    ///
    /// Left button lands the caret; Shift or a drag extends the selection to
    /// the same point; the wheel moves the viewport. Everything else — the
    /// middle button, motion with no button — is not a gesture this editor has.
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        let alt = event.modifiers.contains(KeyModifiers::ALT);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if alt => {
                self.drag_origin = Some((event.column, event.row));
                self.add_caret_at(event.column, event.row);
            }
            MouseEventKind::Down(MouseButton::Left)
                if self.ruler_visible() && event.column as usize >= self.ruler_column() =>
            {
                let line = self.ruler_line(event.row as usize);
                self.goto_line(line);
                self.damage_all = true;
            }
            MouseEventKind::Down(MouseButton::Left) if self.is_fold_column(event.column) => {
                self.click_fold(event.row);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag_origin = Some((event.column, event.row));
                let extend = event.modifiers.contains(KeyModifiers::SHIFT);
                self.click(event.column, event.row, extend);
            }
            // Alt-drag is a column, not a run: every line between the two rows
            // gets its own caret at the dragged columns.
            MouseEventKind::Drag(MouseButton::Left) if alt => {
                self.rectangular(event.column, event.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => self.click(event.column, event.row, true),
            MouseEventKind::Up(MouseButton::Left) => self.drag_origin = None,
            MouseEventKind::ScrollUp => self.scroll(-1),
            MouseEventKind::ScrollDown => self.scroll(1),
            _ => {}
        }
    }

    /// Toggle the fold whose marker sits on `row`.
    fn click_fold(&mut self, row: u16) {
        let Some((line, sub)) = self.row_line_sub(row as usize) else {
            return;
        };
        if sub != 0 || self.fold_state(line).is_none() {
            return;
        }
        if !self.folded.remove(&line) {
            self.folded.insert(line);
        }
        self.clamp_anchor();
        self.ensure_visible();
        self.damage_all = true;
    }

    /// The document offset a screen cell names, clamped into the buffer.
    fn offset_at(&self, col: u16, row: u16) -> usize {
        let text = self.document.text();
        let (line, base) = match self.row_line_sub(row as usize) {
            Some((line, sub)) => (line, sub * self.content_width().max(1)),
            None => (text.line_count().saturating_sub(1), 0),
        };
        let display = (col as usize).saturating_sub(self.gutter_width()) + base + self.left;
        let line_text = text.line(line);
        let column = display.min(metrics::display_column(line_text, line_text.len()));
        text.line_start(line) + metrics::byte_column_for_display(line_text, column)
    }

    /// Alt-click: another caret where the pointer is.
    fn add_caret_at(&mut self, col: u16, row: u16) {
        if row as usize >= self.content_height() {
            return;
        }
        let before = self.document.selection();
        let grown = before.with_added(editor_core::Cursor::caret(self.offset_at(col, row)));
        self.document.set_selection(grown);
        self.damage_all = true;
        self.report_carets();
    }

    /// Alt-drag: one caret per line, between the two display columns.
    ///
    /// Rebuilt from the drag's origin on every motion rather than accumulated,
    /// so dragging back up removes the carets the way it added them.
    fn rectangular(&mut self, col: u16, row: u16) {
        let Some((from_col, from_row)) = self.drag_origin else {
            return;
        };
        if row as usize >= self.content_height() {
            return;
        }
        let text = self.document.text();
        let (top, bottom) = if from_row <= row {
            (from_row, row)
        } else {
            (row, from_row)
        };
        let (left, right) = if from_col <= col {
            (from_col, col)
        } else {
            (col, from_col)
        };
        let gutter = self.gutter_width();
        let start_column = (left as usize).saturating_sub(gutter) + self.left;
        let end_column = (right as usize).saturating_sub(gutter) + self.left;
        let mut cursors = Vec::new();
        for screen_row in top..=bottom {
            let Some((line, _)) = self.row_line_sub(screen_row as usize) else {
                continue;
            };
            let line_text = text.line(line);
            let width = metrics::display_column(line_text, line_text.len());
            // A line too short for the column contributes a caret at its end,
            // which is what makes a column of them usable for appending.
            let from = text.line_start(line)
                + metrics::byte_column_for_display(line_text, start_column.min(width));
            let to = text.line_start(line)
                + metrics::byte_column_for_display(line_text, end_column.min(width));
            cursors.push(editor_core::Cursor::new(from, to));
        }
        if cursors.is_empty() {
            return;
        }
        let primary = cursors.len() - 1;
        self.document
            .set_selection(editor_core::Selection::many(cursors, primary));
        self.damage_all = true;
        self.report_carets();
    }

    /// Say how many carets there are, and that the cap bit when it did.
    fn report_carets(&mut self) {
        let count = self.document.selection().count();
        if count <= 1 {
            return;
        }
        self.status = Some(if count == editor_core::limits::MAX_CURSORS {
            format!("{count} carets — the most this buffer will hold")
        } else {
            format!("{count} carets")
        });
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
    /// Re-read which blocks can be folded, and drop folds that no longer name
    /// one. A header that stopped being a header must not keep hiding lines.
    fn refold(&mut self) {
        self.regions = editor_core::fold::regions(self.document.text());
        let headers: BTreeSet<usize> = self.regions.iter().map(|region| region.header).collect();
        self.folded.retain(|line| headers.contains(line));
    }

    /// Whether `line` is inside a folded block, and so takes no rows.
    #[must_use]
    pub fn is_hidden(&self, line: usize) -> bool {
        self.folded.iter().any(|header| {
            self.regions
                .iter()
                .find(|region| region.header == *header)
                .is_some_and(|region| region.hidden().contains(&line))
        })
    }

    /// The fold marker a line's gutter carries, if it has one.
    ///
    /// `Some(true)` is folded, `Some(false)` is foldable and open.
    #[must_use]
    pub fn fold_state(&self, line: usize) -> Option<bool> {
        self.regions
            .iter()
            .any(|region| region.header == line)
            .then(|| self.folded.contains(&line))
    }

    /// Fold or unfold the block the caret is in.
    pub fn toggle_fold(&mut self) {
        let line = self.document.caret_line_col().line;
        let Some(region) = editor_core::fold::enclosing(&self.regions, line) else {
            self.status = Some("nothing to fold here".to_string());
            return;
        };
        if !self.folded.remove(&region.header) {
            self.folded.insert(region.header);
            // A caret inside what just folded has nowhere to be; the header is
            // the line the block is now shown as.
            if region.hidden().contains(&line) {
                self.goto_line(region.header + 1);
            }
        }
        self.clamp_anchor();
        self.ensure_visible();
        self.damage_all = true;
    }

    /// Open every fold, for the person who cannot find what folded away.
    pub fn unfold_all(&mut self) {
        if self.folded.is_empty() {
            return;
        }
        self.folded.clear();
        self.ensure_visible();
        self.damage_all = true;
    }

    /// Pull the viewport anchor onto a visible line.
    ///
    /// A `top` inside a fold names a line that takes no rows, and the row walk
    /// would then start on nothing.
    fn clamp_anchor(&mut self) {
        let count = self.document.text().line_count();
        while self.top < count && self.is_hidden(self.top) {
            self.top += 1;
            self.top_sub = 0;
        }
        while self.top > 0 && self.is_hidden(self.top) {
            self.top -= 1;
            self.top_sub = 0;
        }
    }

    fn rescan(&mut self) {
        self.refold();
        self.syntax = if self.document.text().len() > MAX_HIGHLIGHT_BYTES {
            Syntax::default()
        } else {
            Syntax::parse(self.document.as_str(), self.grammar)
        };
    }

    /// Re-colour what one edit can have reached.
    ///
    /// The scan restarts above the edit and stops as soon as the grammar is
    /// back in the state the previous one was in, so typing on line 4000 does
    /// not re-lex the 3999 lines over it.
    fn rescan_edited(&mut self, applied: editor_core::Applied) {
        self.refold();
        if self.document.text().len() > MAX_HIGHLIGHT_BYTES {
            self.syntax = Syntax::default();
            return;
        }
        self.syntax = self.syntax.edited(
            self.document.as_str(),
            self.grammar,
            applied.first_line,
            applied.last_line,
            applied.line_delta,
        );
    }

    /// The colouring of one 0-based line, for the renderer.
    #[must_use]
    pub fn syntax_line(&self, line: usize) -> &[editor_core::Span] {
        self.syntax.line(line)
    }

    /// Damage the rows an outcome can have changed.
    fn mark(&mut self, before: Selection, outcome: &editor_core::Outcome) {
        if let Some(applied) = outcome.applied {
            self.rescan_edited(applied);
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
            self.damage_selection(&before);
            self.damage_selection(&after);
        }
    }

    /// Damage every row any of a selection's carets touches.
    fn damage_selection(&mut self, selection: &Selection) {
        for range in selection
            .cursors()
            .iter()
            .map(editor_core::Cursor::range)
            .collect::<Vec<_>>()
        {
            self.damage_span(range);
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
                self.find_origin = self.document.selection().range().start;
                self.highlight = true;
                self.prompt = Some(Prompt::Find {
                    input: self.query.pattern.clone(),
                });
            }
            EditorAction::OpenReplace => {
                self.find_origin = self.document.selection().range().start;
                self.highlight = true;
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
            EditorAction::ToggleFold => self.toggle_fold(),
            EditorAction::UnfoldAll => self.unfold_all(),
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
            EditorAction::ToggleCloseBrackets => {
                self.close_brackets = !self.close_brackets;
                self.adopt_input_style();
                self.status = Some(format!("close brackets: {}", on_off(self.close_brackets)));
            }
            EditorAction::ToggleCase => {
                self.query.case_sensitive = !self.query.case_sensitive;
                self.status = Some(format!("match case: {}", on_off(self.query.case_sensitive)));
            }
            EditorAction::ToggleWholeWord => {
                self.query.whole_word = !self.query.whole_word;
                self.set_query(&self.query.pattern.clone());
                self.status = Some(format!("whole word: {}", on_off(self.query.whole_word)));
            }
            EditorAction::ToggleRegex => {
                self.query.regex = !self.query.regex;
                self.set_query(&self.query.pattern.clone());
                self.status = Some(match &self.query_error {
                    Some(error) => format!("regex: on — {error}"),
                    None => format!("regex: {}", on_off(self.query.regex)),
                });
            }
            EditorAction::Cancel => {
                // Escape's first job is to get back to one caret: a person who
                // added twenty and then reached for Escape wants out of that,
                // not a new undo unit.
                if self.document.selection().is_multiple() {
                    self.run(Command::CollapseCarets);
                    return;
                }
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
                    self.close_after_save = self.in_flight_save.is_some();
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
                // Closing leaves no marks behind; the pattern stays, so
                // `Ctrl-N` still steps through what was being looked for.
                KeyCode::Esc => self.highlight = false,
                KeyCode::Backspace => {
                    input.pop();
                    self.set_query(&input);
                    self.find_as_you_type();
                    self.prompt = Some(Prompt::Find { input });
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.push(character);
                    self.set_query(&input);
                    self.find_as_you_type();
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
                    // Enter replaces *this* match and moves to the next, so a
                    // person can walk a file deciding one at a time; Ctrl-R
                    // rewrites the rest in one go and closes the prompt.
                    KeyCode::Enter => {
                        self.set_query(&find);
                        self.run(Command::ReplaceMatch {
                            query: self.query.clone(),
                            replacement: with.clone(),
                        });
                        if self.status.is_none() {
                            self.find(true);
                        }
                    }
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.set_query(&find);
                        self.run(Command::ReplaceAll {
                            query: self.query.clone(),
                            replacement: with.clone(),
                        });
                        keep = false;
                    }
                    KeyCode::Esc => {
                        self.highlight = false;
                        keep = false;
                    }
                    KeyCode::Tab => editing_replacement = !editing_replacement,
                    KeyCode::Backspace => {
                        if editing_replacement {
                            with.pop();
                        } else {
                            find.pop();
                            self.set_query(&find);
                            self.find_as_you_type();
                        }
                    }
                    KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if editing_replacement {
                            with.push(character);
                        } else {
                            find.push(character);
                            self.set_query(&find);
                            self.find_as_you_type();
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
        self.query = Query {
            pattern: pattern.to_string(),
            ..self.query.clone()
        };
        // Reported as the pattern changes, not at Enter: a half-typed `(` is
        // invalid on the way to being valid, and saying so beats a find bar
        // that silently finds nothing.
        self.query_error = self.query.validate().err().map(|error| error.to_string());
    }

    fn find(&mut self, forward: bool) {
        if self.query.is_empty() {
            self.status = Some("nothing to find".to_string());
            return;
        }
        if let Some(error) = &self.query_error {
            self.status = Some(error.clone());
            return;
        }
        self.highlight = true;
        let command = if forward {
            Command::FindNext(self.query.clone())
        } else {
            Command::FindPrevious(self.query.clone())
        };
        self.run(command);
        if self.status.is_none() {
            self.status = Some(self.match_tally());
        }
    }

    /// `pattern: 3/41`, or what it can say when there are more than it counted.
    ///
    /// Counted here and not on every caret move: this is a discrete gesture,
    /// and a tally on each arrow key would be a document scan on the movement
    /// rung.
    fn match_tally(&self) -> String {
        let cap = editor_core::limits::MAX_SEARCH_RESULTS;
        let text = self.document.text();
        let total = editor_core::count_matches(text, &self.query, cap + 1);
        let here = self.document.selection().range().start;
        let before = editor_core::count_matches_before(text, &self.query, here, cap + 1);
        let index = (before + 1).min(total.max(1));
        if total > cap {
            format!("{}: {index} of more than {cap}", self.query.pattern)
        } else {
            format!("{}: {index}/{total}", self.query.pattern)
        }
    }

    /// Move to the first match at or after where the prompt opened.
    ///
    /// The origin is fixed so that narrowing a query re-searches the same
    /// place rather than walking forward one match per keystroke — CodeMirror's
    /// `openSearchPanel` behaviour. A query with no match leaves the caret
    /// where it is: the person is still typing it.
    fn find_as_you_type(&mut self) {
        self.highlight = true;
        if self.query.is_empty() {
            self.status = None;
            return;
        }
        if let Some(error) = &self.query_error {
            self.status = Some(error.clone());
            return;
        }
        let Some(found) =
            editor_core::find_next(self.document.text(), &self.query, self.find_origin)
        else {
            self.status = Some(format!("{}: no matches", self.query.pattern));
            return;
        };
        self.document
            .set_selection(Selection::new(found.start, found.end));
        self.ensure_visible();
        self.status = None;
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
                self.autosave_suspended = true;
                self.autosave_at = None;
                return;
            }
        }
        let snapshot = self.document.snapshot();
        match disk::save(&self.path, snapshot.text()) {
            Ok(revision) => {
                self.document.confirm_save(&snapshot, revision.clone());
                self.revision = Some(revision);
                self.autosave_suspended = false;
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

    /// Point the caret at the cell `(col, row)` names, or extend to it.
    ///
    /// The inverse of `place_caret`: a screen cell is the gutter plus the
    /// display column past `left`, and `top` names the line under `row`. The
    /// display column rounds down to a grapheme the way a vertical move does,
    /// so a click never lands inside a wide character.
    fn click(&mut self, col: u16, row: u16, extend: bool) {
        let row = row as usize;
        // The status/prompt row is chrome, not text; `content_height` already
        // excludes it, so a click at or past it moves nothing.
        if row >= self.content_height() {
            return;
        }
        let text = self.document.text();
        // A wrapped row names a segment of its line, so the click's display
        // column is measured from that segment's start, not the line's; an
        // empty row past the end lands on the last line.
        let (line, base) = match self.row_line_sub(row) {
            Some((line, sub)) => (line, sub * self.content_width().max(1)),
            None => (text.line_count().saturating_sub(1), 0),
        };
        let display = (col as usize).saturating_sub(self.gutter_width()) + base + self.left;
        let line_text = text.line(line);
        let column = display.min(metrics::display_column(line_text, line_text.len()));
        let offset = text.line_start(line) + metrics::byte_column_for_display(line_text, column);
        self.move_caret_to(offset, extend);
    }

    /// The click twin of a movement command: `with_head` drops the anchor for a
    /// plain click and keeps it for a Shift-click or a drag, and the old and new
    /// caret rows are the only damage.
    fn move_caret_to(&mut self, offset: usize, extend: bool) {
        let before = self.document.selection();
        self.document
            .set_selection(before.with_head(offset, extend));
        let after = self.document.selection();
        if before != after {
            self.damage_selection(&before);
            self.damage_selection(&after);
        }
        self.ensure_visible();
    }

    /// Move the viewport `delta` lines for a wheel notch.
    ///
    /// The wheel scrolls the view, not the caret — the opposite of
    /// [`Self::ensure_visible`]. The caret only moves when the scroll pushed it
    /// off screen, and then just to the nearest visible line, keeping its
    /// column so typing resumes where it reads that it will.
    fn scroll(&mut self, delta: isize) {
        let before = (self.top, self.top_sub);
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                self.advance_anchor();
            } else {
                self.retreat_anchor();
            }
        }
        if (self.top, self.top_sub) == before {
            return;
        }
        self.damage_all = true;

        let caret_line = self.document.caret_line_col().line;
        let target_line = caret_line.clamp(self.top, self.last_visible_line());
        if target_line == caret_line {
            return;
        }
        let text = self.document.text();
        let display = self.caret_position().1;
        let line_text = text.line(target_line);
        let column = display.min(metrics::display_column(line_text, line_text.len()));
        let offset =
            text.line_start(target_line) + metrics::byte_column_for_display(line_text, column);
        self.document.set_selection(Selection::caret(offset));
    }

    /// Move the viewport anchor down one screen row.
    ///
    /// A row and not a line: with wrap on, a logical line taller than the
    /// viewport would otherwise scroll past in one notch, and its middle would
    /// be unreachable.
    fn advance_anchor(&mut self) {
        let last = self.document.text().line_count().saturating_sub(1);
        if self.wrap && self.top_sub + 1 < self.line_rows(self.top) {
            self.top_sub += 1;
            return;
        }
        // A folded block is one row on screen, so the anchor steps over every
        // line it hides in one go.
        while self.top < last {
            self.top += 1;
            self.top_sub = 0;
            if !self.is_hidden(self.top) {
                return;
            }
        }
    }

    /// Move the viewport anchor up one screen row.
    fn retreat_anchor(&mut self) {
        if self.top_sub > 0 {
            self.top_sub -= 1;
            return;
        }
        while self.top > 0 {
            self.top -= 1;
            if !self.is_hidden(self.top) {
                break;
            }
        }
        self.top_sub = if self.wrap {
            self.line_rows(self.top).saturating_sub(1)
        } else {
            0
        };
    }

    /// Scroll just enough to keep the caret visible, never a cell more.
    fn ensure_visible(&mut self) {
        let height = self.content_height().max(1);
        let position = self.document.caret_line_col();
        self.clamp_anchor();

        if self.wrap {
            // No sideways scroll to keep in step; the line wraps instead.
            self.left = 0;
            if position.line < self.top {
                self.top = position.line;
                self.top_sub = 0;
            }
            let width = self.content_width().max(1);
            let column =
                metrics::display_column(self.document.text().line(position.line), position.column);
            let sub = column / width;
            if position.line == self.top && sub < self.top_sub {
                self.top_sub = sub;
            }
            // Advance the anchor one *row* at a time until the caret's row fits.
            // The guard is the bound, not a line count: a caret that sits
            // exactly on a wrap boundary has no row of its own, and the anchor
            // at the end of the buffer would otherwise spin on it.
            while self.line_sub_to_row(position.line, sub).is_none() {
                let before = (self.top, self.top_sub);
                self.advance_anchor();
                if (self.top, self.top_sub) == before {
                    break;
                }
            }
            return;
        }

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
            (KeyCode::Char('w'), true, _) => Action::Editor(EditorAction::ToggleWholeWord),
            (KeyCode::Char('e'), true, _) => Action::Editor(EditorAction::ToggleRegex),
            (KeyCode::Char('p'), true, _) => Action::Editor(EditorAction::ToggleCloseBrackets),
            // Alt, not Ctrl: Ctrl-N and Ctrl-B are already the find bar's.
            (KeyCode::Char('n'), _, true) => Action::Editor(EditorAction::NextChange),
            (KeyCode::Char('p'), _, true) => Action::Editor(EditorAction::PreviousChange),
            (KeyCode::Char('f'), _, true) if shift => Action::Editor(EditorAction::UnfoldAll),
            (KeyCode::Char('f'), _, true) => Action::Editor(EditorAction::ToggleFold),
            (KeyCode::Char('v'), true, _) => Action::Editor(EditorAction::PasteRegister),
            (KeyCode::Char('c'), true, _) => Action::Command(Command::Copy),
            (KeyCode::Char('x'), true, _) => Action::Command(Command::Cut),
            (KeyCode::Char('a'), true, _) => Action::Command(Command::SelectAll),
            (KeyCode::Char('k'), true, _) => Action::Command(Command::DeleteLine),
            (KeyCode::Char('l'), true, _) => Action::Command(Command::SelectLine),
            (KeyCode::Char('d'), true, _) => Action::Command(Command::AddNextOccurrence),
            (KeyCode::Up, _, true) => Action::Command(Command::AddCaretVertically { down: false }),
            (KeyCode::Down, _, true) => Action::Command(Command::AddCaretVertically { down: true }),
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
    ToggleCloseBrackets,
    ToggleWholeWord,
    ToggleRegex,
    PasteRegister,
    /// Jump to the next / previous changed block in the gutter.
    NextChange,
    PreviousChange,
    ToggleFold,
    UnfoldAll,
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

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
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
    (
        "Ctrl-T / Ctrl-W / Ctrl-E",
        "toggle match case / whole word / regex",
    ),
    (
        "Ctrl-R",
        "replace; Tab switches field, Enter one, Ctrl-R all",
    ),
    ("Ctrl-G", "go to line"),
    ("Ctrl-P", "toggle closing brackets as you type"),
    (
        "Alt-F / Alt-Shift-F",
        "fold the block at the caret / unfold everything",
    ),
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

    fn mouse(kind: MouseEventKind, col: u16, row: u16, modifiers: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers,
        }
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
        assert_eq!(
            first.rows,
            (0..app.content_height()).collect::<Vec<_>>(),
            "the first frame paints everything"
        );

        app.handle_key(key(KeyCode::Char('X')));
        let frame = app.take_frame();
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
        assert_eq!(
            app.take_frame().rows,
            (0..app.content_height()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn shift_arrow_selects_and_typing_replaces_the_selection() {
        let mut app = app("hello", false);
        app.handle_key(shifted(KeyCode::Right));
        app.handle_key(shifted(KeyCode::Right));
        assert_eq!(app.selections_in_line(0), vec![(0, 2)]);
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

    /// Typing in the find bar moves to a match as it goes, and Enter steps to
    /// the next one — the same two gestures CodeMirror's search panel has.
    #[test]
    fn find_selects_as_you_type_and_enter_steps_on() {
        let mut app = app("alpha beta alpha", false);
        app.resize(40, 10);
        app.handle_key(control('f'));
        for character in "alp".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        assert_eq!(
            app.document().selection().range(),
            Range::new(0, 3),
            "the first match, without pressing Enter"
        );
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Char('a')));
        assert_eq!(
            app.document().selection().range(),
            Range::new(0, 5),
            "narrowing re-searches the same place instead of walking forward"
        );

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.document().selection().range(), Range::new(11, 16));
        assert_eq!(app.status(), Some("alpha: 2/2"));
        assert!(app.prompt().is_none());
    }

    /// The counter is `n of m`, so a person can tell the third hit of forty
    /// from the thirtieth.
    #[test]
    fn the_status_counts_which_match_the_caret_is_on() {
        let mut app = app("x x x x", false);
        app.resize(40, 10);
        app.handle_key(control('f'));
        app.handle_key(key(KeyCode::Char('x')));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.status(), Some("x: 2/4"));
        app.handle_key(control('n'));
        assert_eq!(app.status(), Some("x: 3/4"));
    }

    /// Enter replaces the match under the caret and moves on; Ctrl-R is the one
    /// that rewrites the rest.
    #[test]
    fn replace_takes_one_match_on_enter_and_the_rest_on_control_r() {
        let mut app = app("a a a", false);
        app.resize(40, 10);
        app.handle_key(control('r'));
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.document().as_str(), "b a a");
        assert!(app.prompt().is_some(), "the prompt stays for the next one");

        app.handle_key(control('r'));
        assert_eq!(app.document().as_str(), "b b b");
        assert_eq!(app.status(), Some("replaced 2"));
        assert!(app.prompt().is_none());
    }

    /// Every visible hit is marked, not just the one the caret is on.
    #[test]
    fn find_marks_every_visible_match() {
        let mut app = app("cat dog cat\ncat\n", false);
        app.resize(40, 10);
        app.handle_key(control('f'));
        app.handle_key(key(KeyCode::Char('c')));
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('t')));
        app.take_frame();
        assert_eq!(
            app.marks_in_line(0),
            vec![
                (0, 3, view::Decoration::Match),
                (8, 11, view::Decoration::Match)
            ]
        );
        assert_eq!(app.marks_in_line(1), vec![(0, 3, view::Decoration::Match)]);
    }

    /// Escape closes the bar and takes the query's marks with it. What is left
    /// is the found match *as a selection*, which marks its siblings the way
    /// any selection does — and the pattern stays, so `Ctrl-N` steps on.
    #[test]
    fn escape_clears_the_query_marks_and_keeps_the_pattern() {
        let mut app = app("cat cat\n", false);
        app.resize(40, 10);
        app.handle_key(control('f'));
        for character in "cat".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.take_frame();
        assert_eq!(
            app.marks_in_line(0),
            vec![
                (0, 3, view::Decoration::Match),
                (4, 7, view::Decoration::Match)
            ],
            "the live query marks every hit, the caret's included"
        );

        app.handle_key(key(KeyCode::Esc));
        app.take_frame();
        assert_eq!(
            app.marks_in_line(0),
            vec![(4, 7, view::Decoration::Match)],
            "no orphan mark under the caret; the sibling is the selection's"
        );

        app.handle_key(control('n'));
        assert_eq!(app.status(), Some("cat: 2/2"));
    }

    /// A selected symbol marks where else it appears, and never itself.
    #[test]
    fn a_selection_marks_its_other_occurrences() {
        let mut app = app("total = total + 1\n", false);
        app.resize(40, 10);
        for _ in 0..5 {
            app.handle_key(shifted(KeyCode::Right));
        }
        app.take_frame();
        assert_eq!(
            app.marks_in_line(0),
            vec![(8, 13, view::Decoration::Match)],
            "the other occurrence, not the selection itself"
        );
    }

    /// The caret next to a bracket marks the one that closes it, and a brace in
    /// a string is text rather than a pair.
    #[test]
    fn the_caret_marks_the_bracket_pair_it_is_next_to() {
        let mut app = app("fn f(a: u8) {}\n", false);
        app.resize(40, 10);
        for _ in 0..4 {
            app.handle_key(key(KeyCode::Right));
        }
        app.take_frame();
        assert_eq!(
            app.marks_in_line(0),
            vec![
                (4, 5, view::Decoration::Bracket),
                (10, 11, view::Decoration::Bracket)
            ]
        );
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
        assert_eq!(app.selections_in_line(0), vec![(0, 3)]);
        assert_eq!(app.selections_in_line(1), vec![(0, 2)]);
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

    #[test]
    fn refusal_suspends_autosave_until_an_explicit_save_succeeds() {
        let mut app = app("hello", false);
        app.set_integrated();
        app.set_autosave(true);
        app.handle_key(key(KeyCode::Char('!')));
        app.save();
        let (id, _, _) = app.take_save_request().unwrap();
        app.save_refused(id, "conflict");
        app.handle_key(key(KeyCode::Char('?')));
        app.set_autosave(false);
        app.set_autosave(true);
        assert!(app.autosave_deadline().is_none());
        app.autosave_if_due();
        assert!(app.take_save_request().is_none());
        app.request_save();
        let (id, _, _) = app.take_save_request().unwrap();
        app.save_confirmed(id, "new".into());
        app.handle_key(key(KeyCode::Char('.')));
        assert!(app.autosave_deadline().is_some());
    }

    #[test]
    fn asynchronous_save_and_close_waits_and_preserves_newer_edits_or_refusals() {
        for outcome in 0..3 {
            let mut app = app("hello", false);
            app.set_integrated();
            app.handle_key(key(KeyCode::Char('!')));
            app.request_close();
            app.handle_key(key(KeyCode::Char('s')));
            assert!(!app.should_quit());
            let (id, _, _) = app.take_save_request().unwrap();
            if outcome == 1 {
                app.handle_key(key(KeyCode::Char('?')));
            }
            if outcome == 2 {
                app.save_refused(id, "conflict");
            } else {
                app.save_confirmed(id, "new".into());
            }
            assert_eq!(app.should_quit(), outcome == 0);
            assert!(!app.close_after_save);
        }
    }

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

    /// Integrated mode with nothing to say hands the last row back to content:
    /// the GUI pane already paints the path and position as HTML chrome.
    #[test]
    fn integrated_idle_reclaims_the_status_row() {
        let mut app = app("hello\nworld\n", false);
        app.set_integrated();
        app.resize(80, 24);
        assert!(!app.needs_status_row());
        assert_eq!(app.content_height(), 24);
    }

    /// A prompt still owns the last row in integrated mode: the HTML around us
    /// cannot show the find bar.
    #[test]
    fn a_prompt_keeps_the_status_row_in_integrated_mode() {
        let mut app = app("hello\nworld\n", false);
        app.set_integrated();
        app.resize(80, 24);
        app.handle_key(control('f'));
        assert!(matches!(app.prompt(), Some(Prompt::Find { .. })));
        assert!(app.needs_status_row());
        assert_eq!(app.content_height(), 23);
    }

    /// A transient message keeps the row too, for the one frame it shows.
    #[test]
    fn a_message_keeps_the_status_row_in_integrated_mode() {
        let mut app = app("hello\nworld\n", true);
        app.set_integrated();
        app.resize(80, 24);
        // A read-only buffer refuses the edit and reports why.
        app.handle_key(key(KeyCode::Char('X')));
        assert!(app.status().is_some());
        assert!(app.needs_status_row());
        assert_eq!(app.content_height(), 23);
    }

    /// Standalone always keeps the idle bar, so its geometry never changes.
    #[test]
    fn standalone_always_keeps_the_status_row() {
        let mut app = app("hello\nworld\n", false);
        app.resize(80, 24);
        assert!(app.needs_status_row());
        assert_eq!(app.content_height(), 23);
    }

    /// A click maps a screen cell back to a byte offset, past the gutter.
    #[test]
    fn a_click_lands_the_caret_past_the_gutter() {
        let mut app = app("hello\nworld\n", false);
        app.resize(80, 24);
        let gutter = app.gutter_width();
        assert_eq!(
            gutter, 4,
            "one number column, a mark cell, a fold cell and a space"
        );
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            (gutter + 2) as u16,
            1,
            KeyModifiers::NONE,
        ));
        // Row 1 is line 2 ("world"); two cells past the gutter is byte 2 of it,
        // which is offset 8 across the whole buffer.
        assert_eq!(app.caret_position(), (2, 2));
        assert_eq!(app.document().caret(), 8);
    }

    /// The status/prompt row is chrome: a click on it is not a caret move.
    #[test]
    fn a_click_on_the_status_row_moves_nothing() {
        let mut app = app("hello\nworld\n", false);
        app.resize(20, 5);
        let before = app.document().caret();
        // content_height is 4, so row 4 is the status row.
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            5,
            4,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.document().caret(), before);
    }

    /// Integrated mode wraps a line wider than the text column onto as many
    /// rows as it takes, and the rows below it shift down.
    #[test]
    fn wrapping_spreads_a_long_line_across_rows() {
        // First line is 25 cells wide; at content_width 10 that is three rows.
        let mut app = app("0123456789abcdefghijklmno\nsecond\n", false);
        app.set_integrated();
        app.resize(14, 10);
        assert!(app.wrap());
        assert_eq!(app.content_width(), 10);
        assert_eq!(app.line_rows(0), 3);
        assert_eq!(app.line_rows(1), 1);
        assert_eq!(app.row_line_sub(0), Some((0, 0)));
        assert_eq!(app.row_line_sub(1), Some((0, 1)));
        assert_eq!(app.row_line_sub(2), Some((0, 2)));
        assert_eq!(app.row_line_sub(3), Some((1, 0)));
    }

    /// The caret at the end of a wrapped line shows on the continuation row it
    /// falls on, not off the right edge of the first.
    #[test]
    fn the_caret_follows_a_wrapped_line_onto_its_continuation_row() {
        let mut app = app("0123456789abcdefghijklmno\nsecond\n", false);
        app.set_integrated();
        app.resize(14, 10);
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.caret_position(), (1, 25));
        // Column 25 is the sixth cell of the third segment (25 / 10, 25 % 10),
        // four cells of gutter in.
        assert_eq!(app.caret_screen(), Some((2, 9)));
    }

    /// A click on a continuation row measures its column from that segment's
    /// start, so it lands deep in the logical line.
    #[test]
    fn a_click_on_a_continuation_row_lands_past_the_wrap() {
        let mut app = app("0123456789abcdefghijklmno\nsecond\n", false);
        app.set_integrated();
        app.resize(14, 10);
        let gutter = app.gutter_width() as u16;
        // Third row (segment 2), three cells in: 2 * 10 + 3.
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            gutter + 3,
            2,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.document().caret(), 23);
    }

    /// Moving onto a line below a screenful of wrapped rows scrolls the anchor
    /// down one *row* at a time, not one line, so it never overshoots.
    #[test]
    fn wrapping_scrolls_to_keep_the_caret_visible() {
        let line = "0123456789abcdefghijklmno\n"; // 25 cells → three rows each
        let mut app = app(&line.repeat(4), false);
        app.set_integrated();
        app.resize(14, 5);
        assert_eq!(app.content_height(), 5);
        app.goto_line(4);
        assert_eq!(
            (app.top(), app.top_sub()),
            (1, 2),
            "the anchor advanced the five rows it took, and no more"
        );
        assert!(matches!(app.caret_screen(), Some((row, _)) if row < 5));
    }

    /// A single logical line taller than the viewport is still reachable: the
    /// anchor names one of its wrapped rows rather than the whole line.
    #[test]
    fn a_line_taller_than_the_viewport_scrolls_within_itself() {
        let mut app = app(&"x".repeat(95), false);
        app.set_integrated();
        app.resize(14, 4);
        // 95 cells over a 10-cell column is ten rows; the viewport holds four.
        assert_eq!(app.line_rows(0), 10);
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(
            (app.top(), app.top_sub()),
            (0, 6),
            "the anchor walked into the line instead of past it"
        );
        assert!(
            app.caret_screen().is_some(),
            "the caret at the end of the line is on screen"
        );
    }

    /// Standalone never wraps: a long line is one row and scrolls sideways.
    #[test]
    fn standalone_does_not_wrap() {
        let mut app = app("0123456789abcdefghijklmno\nsecond\n", false);
        app.resize(14, 10);
        assert!(!app.wrap());
        assert_eq!(app.line_rows(0), 1);
        assert_eq!(app.row_line_sub(1), Some((1, 0)));
    }

    /// Shift-click keeps the anchor and moves the head, like a shifted arrow.
    #[test]
    fn a_shift_click_extends_the_selection() {
        let mut app = app("hello world\n", false);
        app.resize(80, 24);
        let gutter = app.gutter_width() as u16;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            gutter,
            0,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.document().caret(), 0);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            gutter + 5,
            0,
            KeyModifiers::SHIFT,
        ));
        assert_eq!(app.document().selection().range(), Range::new(0, 5));
    }

    /// A drag extends the selection the same way a shifted click does.
    #[test]
    fn a_drag_extends_the_selection() {
        let mut app = app("hello world\n", false);
        app.resize(80, 24);
        let gutter = app.gutter_width() as u16;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            gutter,
            0,
            KeyModifiers::NONE,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            gutter + 5,
            0,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.document().selection().range(), Range::new(0, 5));
    }

    /// The wheel moves the viewport; a caret still on screen does not budge.
    #[test]
    fn the_wheel_scrolls_and_leaves_a_visible_caret_alone() {
        let text = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text, false);
        app.resize(40, 10);
        app.goto_line(5);
        let caret = app.document().caret();
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 0, 0, KeyModifiers::NONE));
        assert_eq!(app.top(), 1);
        assert_eq!(app.document().caret(), caret);
    }

    /// A scrolled viewport repaints every row, but must not blank the screen
    /// first: a client reading the delta between the clear and the rows paints
    /// the gap as a flicker.
    #[test]
    fn scrolling_repaints_without_clearing_the_screen() {
        let text = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text, false);
        app.resize(40, 10);
        assert!(
            app.take_frame().clear,
            "the first frame owns the alt screen"
        );

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 0, 0, KeyModifiers::NONE));
        let frame = app.take_frame();
        assert_eq!(
            frame.rows.len(),
            app.content_height(),
            "the viewport moved, so nothing is reusable"
        );
        assert!(!frame.clear, "a move is not a resize");

        app.resize(40, 20);
        let taller = app.take_frame();
        assert!(
            !taller.clear,
            "a taller viewport paints the rows it gained; it does not blank"
        );
        assert_eq!(taller.rows.len(), app.content_height());

        app.resize(50, 20);
        assert!(
            app.take_frame().clear,
            "only a width change can leave half a grapheme behind"
        );
    }

    /// Typing inside a wrapped line repaints that line's rows and nothing above
    /// them: the row↔line map only moves when a line's height changes.
    #[test]
    fn a_keystroke_in_a_wrapped_buffer_repaints_one_line() {
        let line = "0123456789abcdefghijklmno\n"; // 25 cells → three rows each
        let mut app = app(&line.repeat(6), false);
        app.set_integrated();
        app.resize(14, 9);
        app.take_frame();
        app.goto_line(2);
        app.take_frame();

        app.handle_key(key(KeyCode::Char('X')));
        assert_eq!(
            app.take_frame().rows,
            vec![3, 4, 5],
            "only the edited line's own rows"
        );
    }

    /// A line that gains a row reflows everything under it, and only under it.
    #[test]
    fn a_wrapped_line_that_grows_repaints_from_itself_down() {
        let line = "0123456789abcdefghijk\n"; // 21 cells → three rows
        let mut app = app(&line.repeat(6), false);
        app.set_integrated();
        app.resize(14, 9);
        app.take_frame();
        app.goto_line(2);
        app.take_frame();

        // Line 2 is 21 cells over a 10-cell column: ten more take it to four
        // rows, and every row below it is somebody else's text now.
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Char('X')));
        }
        assert_eq!(app.take_frame().rows, (3..9).collect::<Vec<_>>());
    }

    /// When the scroll pushes the caret off screen it follows to the edge.
    #[test]
    fn the_wheel_pulls_a_caret_that_would_leave_the_viewport() {
        let text = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app(&text, false);
        app.resize(40, 10);
        // The caret starts on line 1; scrolling down moves it off the top.
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 0, 0, KeyModifiers::NONE));
        assert_eq!(app.top(), 1);
        assert_eq!(
            app.caret_position().0,
            2,
            "pulled to the first visible line"
        );
    }

    /// Scrolling up at the top is a no-op, not a move into negative space.
    #[test]
    fn scrolling_up_stops_at_the_top() {
        let mut app = app("a\nb\nc\n", false);
        app.resize(40, 10);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 0, 0, KeyModifiers::NONE));
        assert_eq!(app.top(), 0);
    }

    /// Ctrl-E reads the pattern as a regular expression; the marks and the
    /// counter follow, and a half-typed group says so instead of finding
    /// nothing in silence.
    #[test]
    fn a_regular_expression_find_marks_and_counts() {
        let mut app = app("fn one() {}\nfn two() {}\n", false);
        app.resize(40, 10);
        app.handle_key(control('e'));
        app.handle_key(control('f'));
        for character in "fn .".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.take_frame();
        assert_eq!(app.marks_in_line(0), vec![(0, 4, view::Decoration::Match)]);
        assert_eq!(app.marks_in_line(1), vec![(0, 4, view::Decoration::Match)]);

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.status(), Some("fn .: 2/2"));
    }

    #[test]
    fn an_unfinished_pattern_reports_instead_of_searching() {
        let mut app = app("abc\n", false);
        app.resize(40, 10);
        app.handle_key(control('e'));
        app.handle_key(control('f'));
        app.handle_key(key(KeyCode::Char('(')));
        assert!(
            app.status().is_some_and(|text| text.contains("unclosed")),
            "said nothing about the pattern: {:?}",
            app.status()
        );
        app.take_frame();
        assert!(
            app.marks_in_line(0).is_empty(),
            "no marks from a bad pattern"
        );
    }

    /// The flags a find is running with are on the prompt row, and only the
    /// ones that are on.
    #[test]
    fn the_prompt_names_the_flags_that_are_on() {
        let mut app = app("abc\n", false);
        assert_eq!(app.query_flags(), "  [case]");
        app.handle_key(control('t'));
        assert_eq!(app.query_flags(), "");
        app.handle_key(control('w'));
        app.handle_key(control('e'));
        assert_eq!(app.query_flags(), "  [word regex]");
    }

    /// Alt-click drops another caret where the pointer is; the next keystroke
    /// lands in both places.
    #[test]
    fn alt_click_adds_a_caret() {
        let mut app = app("one\ntwo\n", false);
        app.resize(40, 10);
        let gutter = app.gutter_width() as u16;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            gutter,
            1,
            KeyModifiers::ALT,
        ));
        assert_eq!(app.document().selection().count(), 2);
        app.handle_key(key(KeyCode::Char('-')));
        assert_eq!(app.document().as_str(), "-one\n-two\n");
    }

    /// Alt-drag is a column: one caret per row between the two, at the dragged
    /// display columns, and a short line contributes one at its end.
    #[test]
    fn alt_drag_makes_a_column_of_carets() {
        let mut app = app("aaaa\nbb\ncccc\n", false);
        app.resize(40, 10);
        let gutter = app.gutter_width() as u16;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            gutter + 1,
            0,
            KeyModifiers::ALT,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            gutter + 1,
            2,
            KeyModifiers::ALT,
        ));
        assert_eq!(app.document().selection().count(), 3);
        app.handle_key(key(KeyCode::Char('.')));
        assert_eq!(app.document().as_str(), "a.aaa\nb.b\nc.ccc\n");
    }

    /// Dragging back up removes the carets the way it added them, because the
    /// column is rebuilt from the origin rather than accumulated.
    #[test]
    fn an_alt_drag_that_comes_back_up_shrinks_the_column() {
        let mut app = app("a\nb\nc\nd\n", false);
        app.resize(40, 10);
        let gutter = app.gutter_width() as u16;
        let at = |row| {
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                gutter,
                row,
                KeyModifiers::ALT,
            )
        };
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            gutter,
            0,
            KeyModifiers::ALT,
        ));
        app.handle_mouse(at(3));
        assert_eq!(app.document().selection().count(), 4);
        app.handle_mouse(at(1));
        assert_eq!(app.document().selection().count(), 2);
    }

    /// Escape's first job is getting back to one caret.
    #[test]
    fn escape_collapses_the_carets_before_it_does_anything_else() {
        let mut app = app("a\nb\n", false);
        app.resize(40, 10);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
        assert_eq!(app.document().selection().count(), 2);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.document().selection().count(), 1);
    }

    /// Every caret's row is painted: the terminal owns one hardware cursor, so
    /// the others are cells the renderer draws.
    #[test]
    fn the_extra_carets_are_reported_for_painting() {
        let mut app = app("one\ntwo\n", false);
        app.resize(40, 10);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
        // The caret just added is the primary and owns the hardware cursor, so
        // the one left behind is the cell the renderer has to draw.
        assert_eq!(app.carets_in_line(0), vec![0]);
        assert_eq!(app.carets_in_line(1), Vec::<usize>::new());
    }

    /// Ctrl-D grows the selection one occurrence at a time, and every range
    /// reads as selected.
    #[test]
    fn control_d_selects_the_next_occurrence_and_marks_both() {
        let mut app = app("sum = sum\n", false);
        app.resize(40, 10);
        app.handle_key(control('d'));
        app.handle_key(control('d'));
        assert_eq!(app.document().selection().count(), 2);
        assert_eq!(app.selections_in_line(0), vec![(0, 3), (6, 9)]);
    }

    /// A folded block takes one row, and the rows under it are the lines that
    /// come after the block rather than the ones inside it.
    #[test]
    fn folding_a_block_takes_its_rows_out_of_the_viewport() {
        let mut app = app("fn f() {\n    one();\n    two();\n}\nfn g() {}\n", false);
        app.resize(40, 10);
        assert_eq!(app.fold_state(0), Some(false), "line 1 is foldable");

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(app.fold_state(0), Some(true));
        assert_eq!(app.row_line_sub(0), Some((0, 0)));
        assert_eq!(
            app.row_line_sub(1),
            Some((3, 0)),
            "the body is gone, so the closing brace is the next row"
        );
        assert_eq!(app.folded_line_count(0), 2);

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(app.row_line_sub(1), Some((1, 0)));
    }

    /// A caret inside what just folded has nowhere to be, so it moves to the
    /// header the block is now shown as.
    #[test]
    fn folding_pulls_a_caret_out_of_what_it_hid() {
        let mut app = app("fn f() {\n    one();\n}\n", false);
        app.resize(40, 10);
        app.goto_line(2);
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(app.caret_position().0, 1);
        assert!(app.caret_screen().is_some());
    }

    /// Clicking the fold marker is the same gesture as the key.
    #[test]
    fn a_click_on_the_fold_marker_toggles_it() {
        let mut app = app("fn f() {\n    one();\n}\n", false);
        app.resize(40, 10);
        let column = (app.number_width() + 1) as u16;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            column,
            0,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.fold_state(0), Some(true));
        assert_eq!(
            app.caret_position().0,
            1,
            "the click folded, it did not move"
        );
    }

    /// An edit that stops a line being a header must not leave it hiding rows.
    #[test]
    fn a_fold_whose_block_disappeared_is_dropped() {
        let mut app = app("fn f() {\n    one();\n}\n", false);
        app.resize(40, 10);
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(app.fold_state(0), Some(true));

        app.handle_key(control('a'));
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.fold_state(0), None);
        assert!(!app.is_hidden(0));
    }

    #[test]
    fn unfold_all_opens_everything() {
        let mut app = app("a:\n  b:\n    c: 1\n", false);
        app.resize(40, 10);
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        app.goto_line(1);
        assert!(app.is_hidden(1));
        app.handle_key(KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ));
        assert!(!app.is_hidden(1));
    }

    /// The ruler is the document's shape in one column: a change, a match and
    /// the caret, strongest last.
    #[test]
    fn the_ruler_shows_changes_matches_and_the_caret() {
        let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let mut app = app(&text, false);
        app.resize(60, 11);
        assert!(app.ruler_visible());
        marked(&mut app, &[(90, WireMarkKind::Modified)]);

        app.handle_key(control('f'));
        for character in "line 5".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.take_frame();

        let height = app.content_height();
        assert_eq!(
            app.ruler_at(0),
            Some(RulerMark::Caret),
            "the caret wins its row"
        );
        assert_eq!(
            app.ruler_at(89 * height / 100),
            Some(RulerMark::Change),
            "the changed line's bucket"
        );
        assert!(
            (0..height).any(|row| app.ruler_at(row) == Some(RulerMark::Match)),
            "the matches are on the ruler too"
        );
    }

    /// A click on the ruler jumps to the line that bucket stands for.
    #[test]
    fn a_click_on_the_ruler_jumps_to_that_line() {
        let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let mut app = app(&text, false);
        app.resize(60, 11);
        let column = app.ruler_column() as u16;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            column,
            5,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.caret_position().0, app.ruler_line(5));
        assert_eq!(app.caret_position().0, 51);
    }

    /// A narrow grid keeps every cell for the text.
    #[test]
    fn a_narrow_grid_has_no_ruler() {
        let mut app = app("a\nb\n", false);
        app.resize(30, 10);
        assert!(!app.ruler_visible());
        let wide = app.content_width();
        app.resize(60, 10);
        assert!(app.ruler_visible());
        assert_eq!(
            app.content_width(),
            wide + 30 - 1,
            "the ruler costs exactly one cell"
        );
    }
}
