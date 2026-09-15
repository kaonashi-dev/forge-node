//! The text corpus the document model has to survive, generated here.
//!
//! The shapes that break editors are size, one enormous line, mixed
//! terminators and the byte exactly on the limit. Generating them keeps the
//! gate self-contained — a fixture directory that is not committed cannot be
//! what CI runs.

use editor_core::limits::MAX_DOCUMENT_BYTES;
use editor_core::{execute, Command, Document, LoadError, Query};

fn lines(count: usize) -> String {
    let mut body = String::with_capacity(count * 40);
    for index in 0..count {
        body.push_str(&format!("fn line_{index}() {{ /* ñ 中 🇪🇸 */ }}\n"));
    }
    body
}

#[test]
fn a_twenty_thousand_line_file_round_trips_through_an_edit_and_an_undo() {
    let body = lines(20_000);
    assert!(
        body.len() <= MAX_DOCUMENT_BYTES,
        "the corpus must fit the budget it tests: {} bytes",
        body.len()
    );
    let mut document = Document::from_bytes(body.as_bytes(), false).expect("valid");
    assert_eq!(document.text().line_count(), 20_001);

    execute(&mut document, Command::GotoLine(10_000));
    execute(&mut document, Command::InsertText("X".to_string()));
    assert_eq!(document.text().line_count(), 20_001);
    assert!(document.as_str().contains("Xfn line_9999"));

    execute(&mut document, Command::Undo);
    assert_eq!(document.as_str(), body);
    assert!(!document.is_dirty());
}

#[test]
fn an_edit_in_the_middle_of_a_large_file_reports_only_the_lines_it_touched() {
    let mut document = Document::from_string(lines(20_000), false);
    execute(&mut document, Command::GotoLine(10_000));
    let outcome = execute(&mut document, Command::InsertText("X".to_string()));
    let applied = outcome.applied.expect("an edit");
    assert_eq!(applied.first_line, 9_999);
    assert_eq!(applied.last_line, 9_999);
    assert_eq!(applied.line_delta, 0);

    let outcome = execute(&mut document, Command::InsertNewline);
    let applied = outcome.applied.expect("an edit");
    assert_eq!(applied.line_delta, 1, "a split adds a line");
}

#[test]
fn one_enormous_line_moves_and_edits_without_losing_a_byte() {
    let body = format!("{}\n", "abcdé中🇪🇸\t".repeat(20_000));
    let mut document = Document::from_bytes(body.as_bytes(), false).expect("valid");
    assert_eq!(document.text().line_count(), 2);

    execute(&mut document, Command::MoveLineEnd { extend: false });
    execute(&mut document, Command::InsertText("Z".to_string()));
    assert!(document.as_str().contains("\tZ\n"));
    execute(&mut document, Command::Undo);
    assert_eq!(document.as_str(), body);
}

#[test]
fn the_budget_is_a_limit_on_the_byte_not_on_the_line() {
    let at_limit = vec![b'a'; MAX_DOCUMENT_BYTES];
    assert!(Document::from_bytes(&at_limit, false).is_ok());

    let over = vec![b'a'; MAX_DOCUMENT_BYTES + 1];
    assert!(matches!(
        Document::from_bytes(&over, false),
        Err(LoadError::TooLarge { .. })
    ));

    // One byte under: a single character still fits, the next one does not.
    let under = vec![b'a'; MAX_DOCUMENT_BYTES - 1];
    let mut document = Document::from_bytes(&under, false).expect("valid");
    execute(&mut document, Command::InsertText("b".to_string()));
    assert_eq!(document.text().len(), MAX_DOCUMENT_BYTES);
    let refused = execute(&mut document, Command::InsertText("c".to_string()));
    assert!(refused.refusal.is_some());
    assert_eq!(document.text().len(), MAX_DOCUMENT_BYTES);
}

#[test]
fn invalid_utf8_is_refused_rather_than_repaired() {
    // Latin-1 "café": no NUL, so a probe would call it text.
    assert!(matches!(
        Document::from_bytes(&[b'c', b'a', b'f', 0xe9], false),
        Err(LoadError::NotUtf8)
    ));
}

#[test]
fn a_mixed_terminator_file_keeps_every_terminator_it_arrived_with() {
    let body = "lf\ncrlf\r\nlf\nnone";
    let mut document = Document::from_bytes(body.as_bytes(), false).expect("valid");
    assert_eq!(document.text().line_count(), 4);

    // Editing one line must not normalize the others.
    execute(&mut document, Command::GotoLine(2));
    execute(&mut document, Command::MoveLineEnd { extend: false });
    execute(&mut document, Command::InsertText("!".to_string()));
    assert_eq!(document.as_str(), "lf\ncrlf!\r\nlf\nnone");

    // And a new line takes the terminator of the line it splits.
    execute(&mut document, Command::InsertNewline);
    assert_eq!(document.as_str(), "lf\ncrlf!\r\n\r\nlf\nnone");
}

#[test]
fn replace_all_across_a_large_file_is_one_transaction() {
    let mut document = Document::from_string(lines(20_000), false);
    let outcome = execute(
        &mut document,
        Command::ReplaceAll {
            query: Query::literal("fn line_"),
            replacement: "fn row_".to_string(),
        },
    );
    assert_eq!(outcome.replaced, Some(20_000));
    assert!(!document.as_str().contains("fn line_"));
    execute(&mut document, Command::Undo);
    assert!(document.as_str().contains("fn line_19999"));
    assert!(!document.is_dirty());
}
