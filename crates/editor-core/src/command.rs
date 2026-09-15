//! Named intentions, one per thing a person can ask the editor to do.
//!
//! A key binding, the palette and a future Vim operator all resolve to one of
//! these, so there is a single definition of what "delete word" means and one
//! list to build a help screen from. Geometry arrives as data (`lines`), never
//! as a peek at the view.

use crate::document::{Applied, Document};
use crate::metrics::{display_column, TAB_WIDTH};
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
        Command::InsertText(text) => Outcome::from_edit(document.insert(&text, Origin::Input)),
        Command::InsertNewline => {
            let terminator = newline_for(document);
            Outcome::from_edit(document.insert(&terminator, Origin::Input))
        }
        Command::InsertTab => {
            let spaces = tab_spaces(document);
            Outcome::from_edit(document.insert(&spaces, Origin::Input))
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
        selection.head,
        delta,
        selection.goal_column,
    );
    let mut moved = selection.with_head(head, extend);
    moved.goal_column = Some(goal);
    document.set_selection(moved);
    Outcome::default()
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

fn tab_spaces(document: &Document) -> String {
    let text = document.text();
    let position = text.line_col(document.caret());
    let column = display_column(text.line(position.line), position.column);
    " ".repeat(TAB_WIDTH - column % TAB_WIDTH)
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
        let (start, end) = movement::line_span(document.text(), selection);
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
    let transaction = match Transaction::new(vec![Edit::delete(range)], Origin::Input, selection) {
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
            selection.range().end.max(selection.head),
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
}
