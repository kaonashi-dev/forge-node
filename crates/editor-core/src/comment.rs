//! Line comments follow the buffer grammar and preserve every selection.

use crate::{Applied, Cursor, Document, Edit, EditError, Grammar, Origin, Range, Transaction};

pub(crate) fn toggle(document: &mut Document) -> Result<Option<Applied>, EditError> {
    if document.is_read_only() {
        return Err(EditError::ReadOnly);
    }
    let prefix = match document.input_style().grammar {
        Grammar::Rust | Grammar::CLike => "//",
        Grammar::Python | Grammar::Shell | Grammar::Keyed => "#",
        Grammar::Json | Grammar::Markdown | Grammar::Html | Grammar::None => return Ok(None),
    };
    let selection = document.selection();
    let text = document.text();
    let mut lines = std::collections::BTreeSet::new();
    for cursor in selection.cursors() {
        let range = cursor.range();
        let first = text.line_col(range.start).line;
        let end = text.line_col(range.end);
        // A selection ending at column zero excludes that next line.
        let last = if !range.is_empty() && end.column == 0 {
            end.line.saturating_sub(1)
        } else {
            end.line
        };
        lines.extend(first..=last);
    }
    let targets: Vec<_> = lines
        .into_iter()
        .filter_map(|line| {
            let body = text.line(line);
            let content = body.trim_start_matches([' ', '\t']);
            (!content.trim().is_empty())
                .then_some((text.line_start(line) + body.len() - content.len(), content))
        })
        .collect();
    let uncomment = targets.iter().all(|(_, body)| body.starts_with(prefix));
    let edits: Vec<_> = targets
        .into_iter()
        .map(|(at, body)| {
            if uncomment {
                let len = prefix.len() + usize::from(body[prefix.len()..].starts_with(' '));
                Edit::delete(Range::new(at, at + len))
            } else {
                Edit::insert(at, format!("{prefix} "))
            }
        })
        .collect();
    if edits.is_empty() {
        return Ok(None);
    }
    let map_offset = |offset: usize| {
        let mut shift = 0_isize;
        for edit in &edits {
            if offset < edit.range.start {
                break;
            }
            if offset <= edit.range.end {
                return (edit.range.start as isize + shift) as usize + edit.insert.len();
            }
            shift += edit.insert.len() as isize - edit.range.len() as isize;
        }
        (offset as isize + shift) as usize
    };
    let after =
        selection.mapped(|cursor| Cursor::new(map_offset(cursor.anchor), map_offset(cursor.head)));
    let transaction =
        Transaction::new(edits, Origin::Replace, selection)?.with_selection_after(after);
    document.break_undo_group();
    document.apply(transaction).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{execute, Command, InputStyle, Selection};

    fn doc(body: &str) -> Document {
        let mut document = Document::from_string(body.into(), false);
        document.set_input_style(InputStyle {
            grammar: Grammar::CLike,
            ..InputStyle::default()
        });
        document
    }

    #[test]
    fn end_of_line_toggles_and_undo_restores_clean_state() {
        let mut document = doc("  const café = 1;");
        let end = document.as_str().len();
        document.set_selection(Selection::caret(end));
        toggle(&mut document).unwrap();
        assert_eq!(document.as_str(), "  // const café = 1;");
        assert_eq!(document.caret(), end + 3);
        assert!(document.is_dirty());
        execute(&mut document, Command::Undo);
        assert!(!document.is_dirty());
        execute(&mut document, Command::Redo);
        toggle(&mut document).unwrap();
        assert_eq!(document.as_str(), "  const café = 1;");
        assert_eq!(document.caret(), end);
    }

    #[test]
    fn reversed_selection_preserves_indent_crlf_and_excludes_next_line() {
        let mut document = doc("  one\r\n\r\n\ttwo\r\nthree");
        let end = document.text().line_start(3);
        document.set_selection(Selection::new(end, 0));
        toggle(&mut document).unwrap();
        assert_eq!(document.as_str(), "  // one\r\n\r\n\t// two\r\nthree");
        toggle(&mut document).unwrap();
        assert_eq!(document.as_str(), "  one\r\n\r\n\ttwo\r\nthree");
        assert_eq!(document.selection(), Selection::new(end, 0));
    }

    #[test]
    fn overlapping_carets_comment_each_line_once() {
        let mut document = doc("one\ntwo");
        document.set_selection(Selection::new(0, 5).with_added(Cursor::caret(6)));
        toggle(&mut document).unwrap();
        assert_eq!(document.as_str(), "// one\n// two");
        execute(&mut document, Command::Undo);
        assert_eq!(document.as_str(), "one\ntwo");
    }

    #[test]
    fn hash_languages_and_unsupported_and_read_only_buffers() {
        let mut document = doc("print(1)");
        document.set_input_style(InputStyle {
            grammar: Grammar::Python,
            ..InputStyle::default()
        });
        toggle(&mut document).unwrap();
        assert_eq!(document.as_str(), "# print(1)");
        let mut plain = Document::from_string("text".into(), false);
        assert!(toggle(&mut plain).unwrap().is_none());
        let mut html = Document::from_string("<div></div>".into(), false);
        html.set_input_style(InputStyle {
            grammar: Grammar::Html,
            ..InputStyle::default()
        });
        assert!(toggle(&mut html).unwrap().is_none());
        let mut read_only = Document::from_string("text".into(), true);
        assert_eq!(toggle(&mut read_only), Err(EditError::ReadOnly));
    }
}
