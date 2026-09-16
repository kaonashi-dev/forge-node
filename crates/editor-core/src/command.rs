//! Named intentions, one per thing a person can ask the editor to do.
//!
//! A key binding, the palette and a future Vim operator all resolve to one of
//! these, so there is a single definition of what "delete word" means and one
//! list to build a help screen from. Geometry arrives as data (`lines`), never
//! as a peek at the view.

use crate::document::{Applied, Document};
use crate::indent::{self, IndentUnit};
use crate::metrics::display_column;
use crate::movement;
use crate::search::{self, Query, ReplaceOutcome};
use crate::selection::{Range, Selection};
use crate::transaction::{Edit, EditError, Origin, Transaction};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    MoveLeft {
        extend: bool,
    },
    MoveRight {
        extend: bool,
    },
    MoveUp {
        extend: bool,
    },
    MoveDown {
        extend: bool,
    },
    MoveWordLeft {
        extend: bool,
    },
    MoveWordRight {
        extend: bool,
    },
    MoveLineStart {
        extend: bool,
    },
    MoveLineEnd {
        extend: bool,
    },
    MoveDocumentStart {
        extend: bool,
    },
    MoveDocumentEnd {
        extend: bool,
    },
    /// Move by a page the view has measured; `lines` is its content height.
    MovePage {
        lines: usize,
        down: bool,
        extend: bool,
    },
    SelectAll,
    SelectLine,
    InsertText(String),
    InsertNewline,
    InsertTab,
    ToggleLineComment,
    DeleteBackward,
    DeleteForward,
    DeleteWordBackward,
    DeleteLine,
    /// Cut and copy answer with the text; the clipboard is an adapter's job.
    Copy,
    Cut,
    Paste(String),
    Undo,
    Redo,
    /// 1-based, the way a person and a compiler error count lines.
    GotoLine(usize),
    FindNext(Query),
    FindPrevious(Query),
    ReplaceMatch {
        query: Query,
        replacement: String,
    },
    ReplaceAll {
        query: Query,
        replacement: String,
    },
    /// Add a caret on the line above or below the primary, at its column.
    AddCaretVertically {
        down: bool,
    },
    /// Select the next occurrence of what is selected, keeping the carets
    /// already placed. With nothing selected it takes the word under the caret
    /// first, the way `Ctrl-D` does everywhere else.
    AddNextOccurrence,
    /// Drop back to one caret.
    CollapseCarets,
}

impl Command {
    /// A stable name for the palette, help and logs.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Command::MoveLeft { .. } => "move.left",
            Command::MoveRight { .. } => "move.right",
            Command::MoveUp { .. } => "move.up",
            Command::MoveDown { .. } => "move.down",
            Command::MoveWordLeft { .. } => "move.wordLeft",
            Command::MoveWordRight { .. } => "move.wordRight",
            Command::MoveLineStart { .. } => "move.lineStart",
            Command::MoveLineEnd { .. } => "move.lineEnd",
            Command::MoveDocumentStart { .. } => "move.documentStart",
            Command::MoveDocumentEnd { .. } => "move.documentEnd",
            Command::MovePage { .. } => "move.page",
            Command::SelectAll => "select.all",
            Command::SelectLine => "select.line",
            Command::InsertText(_) => "edit.insert",
            Command::InsertNewline => "edit.newline",
            Command::InsertTab => "edit.tab",
            Command::ToggleLineComment => "edit.toggleLineComment",
            Command::DeleteBackward => "edit.deleteBackward",
            Command::DeleteForward => "edit.deleteForward",
            Command::DeleteWordBackward => "edit.deleteWordBackward",
            Command::DeleteLine => "edit.deleteLine",
            Command::Copy => "clipboard.copy",
            Command::Cut => "clipboard.cut",
            Command::Paste(_) => "clipboard.paste",
            Command::Undo => "history.undo",
            Command::Redo => "history.redo",
            Command::GotoLine(_) => "go.line",
            Command::FindNext(_) => "find.next",
            Command::FindPrevious(_) => "find.previous",
            Command::ReplaceMatch { .. } => "find.replace",
            Command::ReplaceAll { .. } => "find.replaceAll",
            Command::AddCaretVertically { .. } => "select.addCaret",
            Command::AddNextOccurrence => "select.addNextOccurrence",
            Command::CollapseCarets => "select.collapseCarets",
        }
    }
}

/// Why a command did nothing. A command that refuses says so; it never edits
/// part of what it was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    ReadOnly,
    /// The result would cross the document budget.
    TooLarge {
        size: usize,
        limit: usize,
    },
    NothingToUndo,
    NothingToRedo,
    NoMatch,
    /// More matches than one replace-all may rewrite; nothing was replaced.
    TooManyMatches {
        limit: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub changed: bool,
    pub applied: Option<Applied>,
    /// Text a copy or cut produced, for the adapter to put on the clipboard.
    pub clipboard: Option<String>,
    pub replaced: Option<usize>,
    pub refusal: Option<Refusal>,
}

impl Outcome {
    fn refused(refusal: Refusal) -> Self {
        Self {
            refusal: Some(refusal),
            ..Self::default()
        }
    }

    fn from_edit(result: Result<Applied, EditError>) -> Self {
        Self::from_optional_edit(result.map(Some))
    }

    fn from_optional_edit(result: Result<Option<Applied>, EditError>) -> Self {
        match result {
            Ok(Some(applied)) => Self {
                changed: true,
                applied: Some(applied),
                ..Self::default()
            },
            Ok(None) => Self::default(),
            Err(error) => Self::refused(refusal_for(error)),
        }
    }
}

fn refusal_for(error: EditError) -> Refusal {
    match error {
        EditError::ReadOnly => Refusal::ReadOnly,
        EditError::TooLarge { size, limit } => Refusal::TooLarge { size, limit },
        // A range the document rejected is a caller bug, not a person's; the
        // honest report is that nothing happened.
        EditError::Overlapping | EditError::OutOfBounds { .. } => Refusal::NoMatch,
    }
}

/// Run one command against `document`.
pub fn execute(document: &mut Document, command: Command) -> Outcome {
    match command {
        Command::MoveLeft { extend } => move_to(document, extend, movement::left),
        Command::MoveRight { extend } => move_to(document, extend, movement::right),
        Command::MoveUp { extend } => vertical(document, -1, extend),
        Command::MoveDown { extend } => vertical(document, 1, extend),
        Command::MovePage {
            lines,
            down,
            extend,
        } => {
            let step = lines.max(1) as isize;
            vertical(document, if down { step } else { -step }, extend)
        }
        Command::MoveWordLeft { extend } => move_to(document, extend, movement::word_left),
        Command::MoveWordRight { extend } => move_to(document, extend, movement::word_right),
        Command::MoveLineStart { extend } => {
            move_to(document, extend, movement::line_first_non_blank)
        }
        Command::MoveLineEnd { extend } => move_to(document, extend, movement::line_end),
        Command::MoveDocumentStart { extend } => move_to(document, extend, |_, _| 0),
        Command::MoveDocumentEnd { extend } => move_to(document, extend, |text, _| text.len()),
        Command::SelectAll => {
            document.set_selection(Selection::new(0, document.text().len()));
            Outcome::default()
        }
        Command::SelectLine => {
            let (start, end) = movement::line_span(document.text(), document.selection());
            document.set_selection(Selection::new(start, end));
            Outcome::default()
        }
        Command::InsertText(text) => insert_text(document, &text),
        Command::InsertNewline => insert_newline(document),
        Command::InsertTab => {
            let unit = indent_text(document);
            Outcome::from_edit(document.insert(&unit, Origin::Input))
        }
        Command::DeleteBackward => {
            Outcome::from_optional_edit(document.delete(movement::left, Origin::Input))
        }
        Command::DeleteForward => {
            Outcome::from_optional_edit(document.delete(movement::right, Origin::Input))
        }
        Command::DeleteWordBackward => {
            document.break_undo_group();
            Outcome::from_optional_edit(document.delete(movement::word_left, Origin::Input))
        }
        Command::DeleteLine => delete_line(document),
        Command::Copy => copy(document, false),
        Command::Cut => copy(document, true),
        Command::ToggleLineComment => Outcome::from_optional_edit(crate::comment::toggle(document)),
        Command::Paste(text) => {
            document.break_undo_group();
            let outcome = Outcome::from_edit(document.insert(&text, Origin::Paste));
            document.break_undo_group();
            outcome
        }
        Command::Undo => match document.undo() {
            Some(applied) => Outcome {
                changed: true,
                applied: Some(applied),
                ..Outcome::default()
            },
            None => Outcome::refused(Refusal::NothingToUndo),
        },
        Command::Redo => match document.redo() {
            Some(applied) => Outcome {
                changed: true,
                applied: Some(applied),
                ..Outcome::default()
            },
            None => Outcome::refused(Refusal::NothingToRedo),
        },
        Command::GotoLine(line) => {
            document.goto_line(line);
            Outcome::default()
        }
        Command::FindNext(query) => find(document, &query, true),
        Command::FindPrevious(query) => find(document, &query, false),
        Command::AddCaretVertically { down } => add_caret_vertically(document, down),
        Command::AddNextOccurrence => add_next_occurrence(document),
        Command::CollapseCarets => {
            let single = document.selection().single();
            document.set_selection(single);
            Outcome::default()
        }
        Command::ReplaceMatch { query, replacement } => {
            replace_match(document, &query, &replacement)
        }
        Command::ReplaceAll { query, replacement } => {
            match document.replace_all(&query, &replacement) {
                Ok(ReplaceOutcome::Replaced(0)) => Outcome::refused(Refusal::NoMatch),
                Ok(ReplaceOutcome::Replaced(count)) => Outcome {
                    changed: true,
                    replaced: Some(count),
                    ..Outcome::default()
                },
                Ok(ReplaceOutcome::OverBudget { limit }) => {
                    Outcome::refused(Refusal::TooManyMatches { limit })
                }
                Err(error) => Outcome::refused(refusal_for(error)),
            }
        }
    }
}

fn move_to(
    document: &mut Document,
    extend: bool,
    to: impl Fn(&crate::text::Text, usize) -> usize,
) -> Outcome {
    let head = to(document.text(), document.caret());
    document.set_selection(document.selection().with_head(head, extend));
    Outcome::default()
}

fn vertical(document: &mut Document, delta: isize, extend: bool) -> Outcome {
    let selection = document.selection();
    let (head, goal) = movement::vertical(
        document.text(),
        selection.head(),
        delta,
        selection.goal_column(),
    );
    let mut cursor = selection.primary().with_head(head, extend);
    cursor.goal_column = Some(goal);
    document.set_selection(Selection::one(cursor));
    Outcome::default()
}

/// Put another caret one line up or down from the primary, in its column.
///
/// The column is a *display* column, so a caret walking past a tab lands under
/// what it looks like it is under rather than under the same byte count.
fn add_caret_vertically(document: &mut Document, down: bool) -> Outcome {
    let selection = document.selection();
    let text = document.text();
    let primary = selection.primary();
    let here = text.line_col(primary.head);
    let step = if down { 1_isize } else { -1 };
    let Some(line) = here.line.checked_add_signed(step) else {
        return Outcome::default();
    };
    if line >= text.line_count() {
        return Outcome::default();
    }
    let goal = primary
        .goal_column
        .unwrap_or_else(|| display_column(text.line(here.line), here.column));
    let target = text.line(line);
    let column = goal.min(display_column(target, target.len()));
    let at = text.line_start(line) + crate::metrics::byte_column_for_display(target, column);
    let mut added = crate::Cursor::caret(at);
    added.goal_column = Some(goal);
    let grown = selection.with_added(added);
    // A caret that merged into one already there is not a new caret; saying so
    // beats a gesture that silently does nothing.
    let gained = grown.count() > selection.count();
    document.set_selection(grown);
    if gained {
        Outcome::default()
    } else {
        Outcome::refused(Refusal::NoMatch)
    }
}

/// `Ctrl-D`: select the next occurrence, keeping the carets already placed.
fn add_next_occurrence(document: &mut Document) -> Outcome {
    let selection = document.selection();
    let text = document.text();
    let primary = selection.primary();
    // Nothing selected takes the word under the caret first, which is the
    // gesture's first press everywhere it exists.
    if primary.is_empty() {
        let (start, end) = movement::word_span(text, primary.head);
        if start == end {
            return Outcome::refused(Refusal::NoMatch);
        }
        document.set_selection(Selection::one(crate::Cursor::new(start, end)));
        return Outcome::default();
    }
    let needle = text.slice(primary.range()).to_string();
    // Literal and case-sensitive: this is "the same text again", not a search.
    let query = Query::literal(needle);
    let from = selection
        .cursors()
        .iter()
        .map(|cursor| cursor.range().end)
        .max()
        .unwrap_or(primary.range().end);
    let Some(found) = search::find_next(text, &query, from) else {
        return Outcome::refused(Refusal::NoMatch);
    };
    let grown = selection.with_added(crate::Cursor::new(found.start, found.end));
    let gained = grown.count() > selection.count();
    document.set_selection(grown);
    if gained {
        Outcome::default()
    } else {
        Outcome::refused(Refusal::NoMatch)
    }
}

/// The terminator a new line should carry: the one the current line already
/// uses, so a CRLF file does not grow LF lines.
fn newline_for(document: &Document) -> String {
    let text = document.text();
    let line = text.line_of_offset(document.caret());
    let here = text.line_terminator(line);
    if !here.is_empty() {
        return here.to_string();
    }
    let first = text.line_terminator(0);
    if first.is_empty() {
        "\n".to_string()
    } else {
        first.to_string()
    }
}

/// What Tab inserts: the buffer's own indent unit, aligned to its next stop.
///
/// A tabbed file gets a tab; a spaced one gets enough spaces to reach the next
/// multiple of its own width, which is not always four — a two-space file
/// indented to four columns would drift on every Tab.
fn indent_text(document: &Document) -> String {
    let unit = document.input_style().indent;
    if unit == IndentUnit::Tab {
        return "\t".to_string();
    }
    let text = document.text();
    let position = text.line_col(document.caret());
    let column = display_column(text.line(position.line), position.column);
    let width = unit.width().max(1);
    " ".repeat(width - column % width)
}

/// Typing one character, with the bracket and quote rules on top.
///
/// A closer already under the caret is stepped over rather than doubled, and an
/// opener brings its partner only where one would not be in the way. Both are
/// off when the buffer says so, and a selection replaces as it always did.
fn insert_text(document: &mut Document, typed: &str) -> Outcome {
    let style = document.input_style();
    let caret = document.caret();
    let mut characters = typed.chars();
    let (Some(character), None) = (characters.next(), characters.next()) else {
        return Outcome::from_edit(document.insert(typed, Origin::Input));
    };
    if !style.close_brackets || !document.selection().is_empty() {
        return Outcome::from_edit(document.insert(typed, Origin::Input));
    }
    if matches!(character, ')' | ']' | '}' | '"' | '\'' | '`')
        && indent::skips_closer(document.text(), caret, character)
    {
        document.set_selection(Selection::caret(caret + character.len_utf8()));
        return Outcome::default();
    }
    match indent::closer_for(character) {
        Some(closer) if indent::wants_closer(document.text(), caret, character) => {
            Outcome::from_edit(document.insert_around(
                typed,
                closer.encode_utf8(&mut [0_u8; 4]),
                Origin::Input,
            ))
        }
        _ => Outcome::from_edit(document.insert(typed, Origin::Input)),
    }
}

/// Enter, carrying the indentation the line already had.
///
/// One level deeper when the line opens a block, and a closer waiting on the
/// other side of the caret is pushed onto a line of its own at the original
/// depth — the shape `{` then Enter is supposed to produce.
fn insert_newline(document: &mut Document) -> Outcome {
    let terminator = newline_for(document);
    let style = document.input_style();
    let caret = document.caret();
    let text = document.text();
    let here = indent::leading(text, text.line_start(text.line_of_offset(caret)));
    let deeper = indent::opens_block(text, caret, style.grammar);
    let unit = style.indent.text();
    let opened = format!(
        "{terminator}{here}{}",
        if deeper { unit.as_str() } else { "" }
    );
    // `{|}`: the closer belongs on its own line at the opening depth, so the
    // body has somewhere to be.
    let closing = deeper
        && text.as_str()[caret..]
            .chars()
            .next()
            .is_some_and(|next| matches!(next, '}' | ']' | ')'));
    if closing {
        let after = format!("{terminator}{here}");
        return Outcome::from_edit(document.insert_around(&opened, &after, Origin::Input));
    }
    Outcome::from_edit(document.insert(&opened, Origin::Input))
}

fn delete_line(document: &mut Document) -> Outcome {
    let (start, end) = movement::line_span(document.text(), document.selection());
    if start == end {
        return Outcome::default();
    }
    document.break_undo_group();
    let transaction = match Transaction::new(
        vec![Edit::delete(Range::new(start, end))],
        Origin::Input,
        document.selection(),
    ) {
        Ok(transaction) => transaction.with_selection_after(Selection::caret(start)),
        Err(error) => return Outcome::refused(refusal_for(error)),
    };
    let outcome = Outcome::from_edit(document.apply(transaction));
    document.break_undo_group();
    outcome
}

/// Copy or cut the selection, or the whole line when there is none.
fn copy(document: &mut Document, cut: bool) -> Outcome {
    let selection = document.selection();
    let range = if selection.is_empty() {
        let (start, end) = movement::line_span(document.text(), selection.clone());
        Range::new(start, end)
    } else {
        selection.range()
    };
    let text = document.text().slice(range).to_string();
    if !cut || range.is_empty() {
        return Outcome {
            clipboard: Some(text),
            ..Outcome::default()
        };
    }
    document.break_undo_group();
    let transaction =
        match Transaction::new(vec![Edit::delete(range)], Origin::Input, selection.clone()) {
            Ok(transaction) => transaction.with_selection_after(Selection::caret(range.start)),
            Err(error) => return Outcome::refused(refusal_for(error)),
        };
    let mut outcome = Outcome::from_edit(document.apply(transaction));
    document.break_undo_group();
    outcome.clipboard = Some(text);
    outcome
}

fn find(document: &mut Document, query: &Query, forward: bool) -> Outcome {
    if query.is_empty() {
        return Outcome::refused(Refusal::NoMatch);
    }
    let selection = document.selection();
    let found = if forward {
        search::find_next(
            document.text(),
            query,
            selection.range().end.max(selection.head()),
        )
    } else {
        search::find_previous(document.text(), query, selection.range().start)
    };
    match found {
        Some(range) => {
            document.set_selection(Selection::new(range.start, range.end));
            Outcome::default()
        }
        None => Outcome::refused(Refusal::NoMatch),
    }
}

/// Replace the selection when it already is a match, then move to the next one.
fn replace_match(document: &mut Document, query: &Query, replacement: &str) -> Outcome {
    let selection = document.selection();
    let selected = document.text().slice(selection.range());
    let matches = if query.case_sensitive {
        selected == query.pattern
    } else {
        selected.eq_ignore_ascii_case(&query.pattern)
    };
    if !matches || query.is_empty() {
        return find(document, query, true);
    }
    document.break_undo_group();
    let outcome = Outcome::from_edit(document.insert(replacement, Origin::Replace));
    document.break_undo_group();
    if outcome.refusal.is_some() {
        return outcome;
    }
    let mut outcome = outcome;
    outcome.replaced = Some(1);
    find(document, query, true);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(body: &str) -> Document {
        Document::from_string(body.to_string(), false)
    }

    fn run(document: &mut Document, command: Command) -> Outcome {
        execute(document, command)
    }

    #[test]
    fn typing_then_undo_restores_text_and_selection() {
        let mut doc = document("hello");
        run(&mut doc, Command::MoveDocumentEnd { extend: false });
        run(&mut doc, Command::InsertText("!".to_string()));
        run(&mut doc, Command::InsertText("?".to_string()));
        assert_eq!(doc.as_str(), "hello!?");
        assert!(doc.is_dirty());

        run(&mut doc, Command::Undo);
        assert_eq!(doc.as_str(), "hello");
        assert_eq!(doc.caret(), 5);
        assert!(!doc.is_dirty(), "undo back to the loaded text is not dirty");

        run(&mut doc, Command::Redo);
        assert_eq!(doc.as_str(), "hello!?");
        assert!(doc.is_dirty());
    }

    #[test]
    fn a_paste_undoes_as_one_unit() {
        let mut doc = document("");
        run(&mut doc, Command::Paste("one\ntwo\nthree".to_string()));
        assert_eq!(doc.text().line_count(), 3);
        run(&mut doc, Command::Undo);
        assert_eq!(doc.as_str(), "");
    }

    #[test]
    fn a_new_line_keeps_the_files_terminator() {
        let mut doc = document("a\r\nb\r\n");
        run(&mut doc, Command::InsertNewline);
        assert_eq!(doc.as_str(), "\r\na\r\nb\r\n");
    }

    #[test]
    fn read_only_refuses_every_edit_at_the_transaction_entry() {
        let mut doc = Document::from_string("hello".to_string(), true);
        for command in [
            Command::InsertText("x".to_string()),
            Command::InsertNewline,
            Command::InsertTab,
            Command::DeleteBackward,
            Command::DeleteForward,
            Command::DeleteLine,
            Command::Paste("x".to_string()),
            Command::Cut,
        ] {
            let outcome = run(&mut doc, command.clone());
            assert_eq!(
                outcome.refusal,
                Some(Refusal::ReadOnly),
                "{} was not refused",
                command.name()
            );
        }
        assert_eq!(doc.as_str(), "hello");
        assert!(!doc.is_dirty());
    }

    #[test]
    fn cut_with_no_selection_takes_the_whole_line() {
        let mut doc = document("one\ntwo\n");
        let outcome = run(&mut doc, Command::Cut);
        assert_eq!(outcome.clipboard.as_deref(), Some("one\n"));
        assert_eq!(doc.as_str(), "two\n");
    }

    #[test]
    fn replace_all_is_one_undo_step_and_rewrites_every_match() {
        let mut doc = document("a a a");
        let outcome = run(
            &mut doc,
            Command::ReplaceAll {
                query: Query::literal("a"),
                replacement: "bb".to_string(),
            },
        );
        assert_eq!(outcome.replaced, Some(3));
        assert_eq!(doc.as_str(), "bb bb bb");
        run(&mut doc, Command::Undo);
        assert_eq!(doc.as_str(), "a a a");
    }

    #[test]
    fn find_wraps_and_reports_a_miss() {
        let mut doc = document("alpha beta alpha");
        let query = Query::literal("alpha");
        run(&mut doc, Command::FindNext(query.clone()));
        assert_eq!(doc.selection().range(), Range::new(0, 5));
        run(&mut doc, Command::FindNext(query.clone()));
        assert_eq!(doc.selection().range(), Range::new(11, 16));
        run(&mut doc, Command::FindNext(query));
        assert_eq!(doc.selection().range(), Range::new(0, 5));

        let miss = run(&mut doc, Command::FindNext(Query::literal("zzz")));
        assert_eq!(miss.refusal, Some(Refusal::NoMatch));
    }

    #[test]
    fn a_tab_advances_to_the_next_stop_from_the_caret() {
        let mut doc = document("ab");
        run(&mut doc, Command::MoveLineEnd { extend: false });
        run(&mut doc, Command::InsertTab);
        assert_eq!(doc.as_str(), "ab  ");
    }

    #[test]
    fn every_command_reports_a_stable_name() {
        assert_eq!(Command::Undo.name(), "history.undo");
        assert_eq!(
            Command::MovePage {
                lines: 10,
                down: true,
                extend: false
            }
            .name(),
            "move.page"
        );
    }

    use crate::indent::InputStyle;

    fn styled(body: &str, style: InputStyle) -> Document {
        let mut doc = document(body);
        doc.set_input_style(style);
        doc
    }

    fn rust_style() -> InputStyle {
        InputStyle {
            indent: IndentUnit::Spaces(4),
            close_brackets: true,
            grammar: crate::syntax::Grammar::Rust,
        }
    }

    #[test]
    fn typing_an_opener_brings_its_closer_and_leaves_the_caret_between() {
        let mut doc = styled("", rust_style());
        run(&mut doc, Command::InsertText("(".to_string()));
        assert_eq!(doc.as_str(), "()");
        assert_eq!(doc.caret(), 1);
        run(&mut doc, Command::InsertText("a".to_string()));
        assert_eq!(doc.as_str(), "(a)");
    }

    /// One keystroke, one undo step: the closer came with the opener and goes
    /// back with it.
    #[test]
    fn an_auto_closed_pair_undoes_as_one_step() {
        let mut doc = styled("", rust_style());
        run(&mut doc, Command::InsertText("[".to_string()));
        run(&mut doc, Command::Undo);
        assert_eq!(doc.as_str(), "");
    }

    #[test]
    fn typing_a_closer_over_one_that_is_there_steps_past_it() {
        let mut doc = styled("", rust_style());
        run(&mut doc, Command::InsertText("(".to_string()));
        run(&mut doc, Command::InsertText(")".to_string()));
        assert_eq!(doc.as_str(), "()", "the closer was not doubled");
        assert_eq!(doc.caret(), 2);
    }

    /// Wrapping a word is what `(` before one means; a closer there would have
    /// to be deleted.
    #[test]
    fn an_opener_before_a_word_is_typed_alone() {
        let mut doc = styled("word", rust_style());
        run(&mut doc, Command::InsertText("(".to_string()));
        assert_eq!(doc.as_str(), "(word");
    }

    #[test]
    fn closing_brackets_can_be_turned_off() {
        let mut doc = styled(
            "",
            InputStyle {
                close_brackets: false,
                ..rust_style()
            },
        );
        run(&mut doc, Command::InsertText("(".to_string()));
        assert_eq!(doc.as_str(), "(");
    }

    #[test]
    fn enter_carries_the_indentation_the_line_had() {
        let mut doc = styled("    let x = 1;", rust_style());
        run(&mut doc, Command::MoveLineEnd { extend: false });
        run(&mut doc, Command::InsertNewline);
        run(&mut doc, Command::InsertText("y".to_string()));
        assert_eq!(doc.as_str(), "    let x = 1;\n    y");
    }

    #[test]
    fn enter_after_an_opener_goes_one_level_deeper() {
        let mut doc = styled("fn f() {", rust_style());
        run(&mut doc, Command::MoveLineEnd { extend: false });
        run(&mut doc, Command::InsertNewline);
        run(&mut doc, Command::InsertText("g".to_string()));
        assert_eq!(doc.as_str(), "fn f() {\n    g");
    }

    /// Enter between a pair puts the closer on its own line at the opening
    /// depth, which is the shape `{` then Enter is supposed to produce.
    #[test]
    fn enter_between_a_pair_opens_a_block() {
        let mut doc = styled("  fn f() {}", rust_style());
        run(&mut doc, Command::MoveLineEnd { extend: false });
        run(&mut doc, Command::MoveLeft { extend: false });
        run(&mut doc, Command::InsertNewline);
        assert_eq!(doc.as_str(), "  fn f() {\n      \n  }");
        run(&mut doc, Command::InsertText("g".to_string()));
        assert_eq!(doc.as_str(), "  fn f() {\n      g\n  }");
    }

    /// Python blocks are made of colons, and only Python's are.
    #[test]
    fn a_colon_opens_a_block_only_in_python() {
        let mut doc = styled(
            "if x:",
            InputStyle {
                indent: IndentUnit::Spaces(2),
                grammar: crate::syntax::Grammar::Python,
                ..rust_style()
            },
        );
        run(&mut doc, Command::MoveLineEnd { extend: false });
        run(&mut doc, Command::InsertNewline);
        run(&mut doc, Command::InsertText("y".to_string()));
        assert_eq!(doc.as_str(), "if x:\n  y");
    }

    /// Tab is the buffer's unit, not a fixed four columns: a tabbed file must
    /// not gain spaces, and a two-space file must not drift to four.
    #[test]
    fn tab_uses_the_unit_the_buffer_already_has() {
        let mut doc = styled(
            "",
            InputStyle {
                indent: IndentUnit::Tab,
                ..rust_style()
            },
        );
        run(&mut doc, Command::InsertTab);
        assert_eq!(doc.as_str(), "\t");

        let mut doc = styled(
            "",
            InputStyle {
                indent: IndentUnit::Spaces(2),
                ..rust_style()
            },
        );
        run(&mut doc, Command::InsertTab);
        run(&mut doc, Command::InsertTab);
        assert_eq!(doc.as_str(), "    ");
    }

    /// Every caret gets the keystroke, and each lands past its own insertion —
    /// not past the ones above it.
    #[test]
    fn typing_with_three_carets_edits_all_three() {
        let mut doc = document("a\nb\nc\n");
        doc.set_selection(
            Selection::caret(0)
                .with_added(crate::Cursor::caret(2))
                .with_added(crate::Cursor::caret(4)),
        );
        run(&mut doc, Command::InsertText("X".to_string()));
        assert_eq!(doc.as_str(), "Xa\nXb\nXc\n");
        assert_eq!(
            doc.selection()
                .cursors()
                .iter()
                .map(|c| c.head)
                .collect::<Vec<_>>(),
            vec![1, 4, 7]
        );
    }

    #[test]
    fn backspace_with_three_carets_deletes_at_all_three() {
        let mut doc = document("aX\nbX\ncX\n");
        doc.set_selection(
            Selection::caret(2)
                .with_added(crate::Cursor::caret(5))
                .with_added(crate::Cursor::caret(8)),
        );
        run(&mut doc, Command::DeleteBackward);
        assert_eq!(doc.as_str(), "a\nb\nc\n");
    }

    /// One gesture is one history entry, so Ctrl-Z puts back all of it.
    #[test]
    fn a_multi_caret_edit_undoes_as_one_entry() {
        let mut doc = document("a\nb\nc\n");
        doc.set_selection(
            Selection::caret(0)
                .with_added(crate::Cursor::caret(2))
                .with_added(crate::Cursor::caret(4)),
        );
        run(&mut doc, Command::InsertText("X".to_string()));
        run(&mut doc, Command::Undo);
        assert_eq!(doc.as_str(), "a\nb\nc\n");
        assert_eq!(
            doc.selection().count(),
            3,
            "undo restores the carets the edit was made with"
        );
    }

    /// Deleting across carets shifts the ones below by what the ones above
    /// removed; a caret computed against the old text would be off.
    #[test]
    fn carets_below_an_edit_land_where_the_text_moved_them() {
        let mut doc = document("aaa\nbbb\n");
        doc.set_selection(Selection::new(0, 2).with_added(crate::Cursor::new(4, 6)));
        run(&mut doc, Command::InsertText("Z".to_string()));
        assert_eq!(doc.as_str(), "Za\nZb\n");
        assert_eq!(
            doc.selection()
                .cursors()
                .iter()
                .map(|c| c.head)
                .collect::<Vec<_>>(),
            vec![1, 4]
        );
    }

    #[test]
    fn control_d_takes_the_word_then_its_next_occurrence() {
        let mut doc = document("total = total + total\n");
        run(&mut doc, Command::AddNextOccurrence);
        assert_eq!(doc.selection().count(), 1);
        assert_eq!(doc.text().slice(doc.selection().range()), "total");

        run(&mut doc, Command::AddNextOccurrence);
        assert_eq!(doc.selection().count(), 2);
        run(&mut doc, Command::AddNextOccurrence);
        assert_eq!(doc.selection().count(), 3);

        run(&mut doc, Command::InsertText("n".to_string()));
        assert_eq!(doc.as_str(), "n = n + n\n");
    }

    #[test]
    fn control_d_on_nothing_refuses_instead_of_selecting_space() {
        let mut doc = document("   \n");
        run(&mut doc, Command::MoveRight { extend: false });
        let outcome = run(&mut doc, Command::AddNextOccurrence);
        assert_eq!(outcome.refusal, Some(Refusal::NoMatch));
    }

    #[test]
    fn a_caret_can_be_added_on_the_line_below() {
        let mut doc = document("one\ntwo\nthree\n");
        run(&mut doc, Command::AddCaretVertically { down: true });
        assert_eq!(doc.selection().count(), 2);
        run(&mut doc, Command::AddCaretVertically { down: true });
        assert_eq!(doc.selection().count(), 3);
        run(&mut doc, Command::InsertText("-".to_string()));
        assert_eq!(doc.as_str(), "-one\n-two\n-three\n");
    }

    /// Past the last line there is nowhere to put one, and saying so beats a
    /// caret stacked on the one already there.
    #[test]
    fn adding_a_caret_past_the_end_does_nothing() {
        let mut doc = document("one\n");
        run(&mut doc, Command::AddCaretVertically { down: false });
        assert_eq!(doc.selection().count(), 1);
    }

    #[test]
    fn collapsing_goes_back_to_the_primary_caret() {
        let mut doc = document("a\nb\nc\n");
        run(&mut doc, Command::AddCaretVertically { down: true });
        run(&mut doc, Command::AddCaretVertically { down: true });
        assert_eq!(doc.selection().count(), 3);
        run(&mut doc, Command::CollapseCarets);
        assert_eq!(doc.selection().count(), 1);
    }

    /// A plain arrow key is a movement, not a multi-caret operation: keeping
    /// them would need a rule per direction for what the others do.
    #[test]
    fn a_plain_move_drops_the_extra_carets() {
        let mut doc = document("a\nb\n");
        run(&mut doc, Command::AddCaretVertically { down: true });
        run(&mut doc, Command::MoveRight { extend: false });
        assert_eq!(doc.selection().count(), 1);
    }
}
