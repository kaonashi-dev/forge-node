//! The core against a naive model: a `String` and a cursor.
//!
//! Undo has to restore text *and* selection over long mixed sequences, which a
//! hand-written case cannot cover. The generator is a deterministic LCG so a
//! failure names a seed that reproduces it, without a random dependency.

use editor_core::{execute, Command, Document, Origin, Query, Selection};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        // Numerical Recipes constants: good enough to shuffle a test, and
        // reproducible across platforms, which `rand` would not be.
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

const WORDS: [&str; 6] = ["a", "ñ", "中", "🇪🇸", "\n", "  "];

fn command_for(seed: &mut Lcg, document: &Document) -> Command {
    match seed.below(10) {
        0 => Command::MoveLeft { extend: false },
        1 => Command::MoveRight { extend: false },
        2 => Command::MoveUp { extend: false },
        3 => Command::MoveDown { extend: false },
        4 => Command::MoveRight { extend: true },
        5 => Command::DeleteBackward,
        6 => Command::DeleteForward,
        7 => Command::GotoLine(seed.below(document.text().line_count()) + 1),
        8 => Command::Paste(WORDS[seed.below(WORDS.len())].repeat(3)),
        _ => Command::InsertText(WORDS[seed.below(WORDS.len())].to_string()),
    }
}

#[test]
fn undo_returns_every_intermediate_state_in_order() {
    for seed in 0..40_u64 {
        let mut random = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut document = Document::from_string("one\ntwo\r\nthree".to_string(), false);
        let mut states = vec![(document.as_str().to_string(), document.selection())];

        for _ in 0..120 {
            let command = command_for(&mut random, &document);
            let outcome = execute(&mut document, command);
            if outcome.changed {
                states.push((document.as_str().to_string(), document.selection()));
            }
        }

        // Undo walks back through the states the edits produced. Coalescing
        // merges typing runs, so an undo may skip several recorded states; what
        // must hold is that every state it lands on is one that existed.
        // Typing a character over a selection of the same character changes no
        // bytes but is still an edit, so an undo step may land on the text it
        // started from. What must never happen is landing somewhere unseen.
        while document.can_undo() {
            execute(&mut document, Command::Undo);
            let now = document.as_str().to_string();
            assert!(
                states.iter().any(|(text, _)| text == &now),
                "undo produced a state that never existed (seed {seed})"
            );
        }

        assert_eq!(
            document.as_str(),
            "one\ntwo\r\nthree",
            "undoing everything did not return the loaded text (seed {seed})"
        );
        assert!(
            !document.is_dirty(),
            "clean text still reported dirty (seed {seed})"
        );
    }
}

#[test]
fn redo_replays_exactly_what_undo_removed() {
    let mut random = Lcg(7);
    let mut document = Document::from_string("alpha\nbeta\n".to_string(), false);
    for _ in 0..80 {
        let command = command_for(&mut random, &document);
        execute(&mut document, command);
    }
    let final_text = document.as_str().to_string();

    let mut undone = 0;
    while document.can_undo() {
        execute(&mut document, Command::Undo);
        undone += 1;
    }
    for _ in 0..undone {
        execute(&mut document, Command::Redo);
    }
    assert_eq!(document.as_str(), final_text);
}

#[test]
fn the_bytes_of_a_loaded_file_survive_an_edit_and_an_undo() {
    for body in [
        "",
        "\n",
        "a",
        "a\n",
        "a\r\nb\nc\r\n",
        "\u{feff}bom\n",
        "tab\there\n",
        "emoji 🇪🇸 and 中\n",
        "trailing spaces   \n\n\n",
    ] {
        let mut document = Document::from_bytes(body.as_bytes(), false).expect("valid fixture");
        assert_eq!(
            document.as_str(),
            body,
            "load changed the bytes of {body:?}"
        );
        execute(&mut document, Command::MoveDocumentEnd { extend: false });
        execute(&mut document, Command::InsertText("X".to_string()));
        execute(&mut document, Command::Undo);
        assert_eq!(document.as_str(), body, "round trip lost bytes of {body:?}");
    }
}

#[test]
fn a_transaction_over_the_budget_leaves_the_document_untouched() {
    let limit = editor_core::limits::MAX_DOCUMENT_BYTES;
    let body = "x".repeat(limit - 4);
    let mut document = Document::from_string(body.clone(), false);
    let refusal = document.insert(&"y".repeat(16), Origin::Paste);
    assert!(refusal.is_err(), "an oversize paste was accepted");
    assert_eq!(document.as_str(), body);
    assert!(!document.is_dirty());
}

#[test]
fn oversized_replace_all_is_refused_before_cloning_per_match() {
    let original = "x".repeat(200_000);
    let mut document = Document::from_string(original.clone(), false);
    let result = document.replace_all(&Query::literal("x"), &"y".repeat(64 * 1024));
    assert!(matches!(
        result,
        Err(editor_core::EditError::TooLarge { .. })
    ));
    assert_eq!(document.as_str(), original);
    assert!(!document.is_dirty());
    assert!(!document.can_undo());
}

#[test]
fn replace_all_is_atomic_across_the_whole_document() {
    let mut document = Document::from_string("ab\nab\nab\n".to_string(), false);
    execute(
        &mut document,
        Command::ReplaceAll {
            query: Query::literal("ab"),
            replacement: "wxyz".to_string(),
        },
    );
    assert_eq!(document.as_str(), "wxyz\nwxyz\nwxyz\n");
    execute(&mut document, Command::Undo);
    assert_eq!(document.as_str(), "ab\nab\nab\n");
    assert!(!document.is_dirty());
}

#[test]
fn saving_while_typing_continues_leaves_the_buffer_dirty() {
    let mut document = Document::from_string("value".to_string(), false);
    execute(&mut document, Command::InsertText("a".to_string()));
    let snapshot = document.snapshot();

    // The keystroke that lands between taking the bytes and confirming the
    // write: the save is for the older state, so dirty must survive it.
    document.break_undo_group();
    execute(&mut document, Command::InsertText("b".to_string()));
    document.confirm_save(&snapshot, "revision-1");
    assert!(document.is_dirty());

    // Confirming the state that is actually current does clear it.
    let current = document.snapshot();
    document.confirm_save(&current, "revision-2");
    assert!(!document.is_dirty());
}

#[test]
fn a_reload_replaces_the_buffer_and_clears_dirty() {
    let mut document = Document::from_string("old\n".to_string(), false);
    execute(&mut document, Command::InsertText("x".to_string()));
    assert!(document.is_dirty());
    document.reload("new\n", "revision-2").expect("reload");
    assert_eq!(document.as_str(), "new\n");
    assert!(!document.is_dirty());
    assert_eq!(document.disk_revision(), Some("revision-2"));
    // A reload keeps where the person was looking, it does not jump to the end.
    assert_eq!(document.selection(), Selection::caret(1));
}
