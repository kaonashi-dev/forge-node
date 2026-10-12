use editor_core::{Cursor, Document, EditError, Origin, Selection};

#[test]
fn a_reload_preserves_undo_and_maps_carets_in_the_unchanged_suffix() {
    let mut doc = Document::from_string("first\ntarget\ntail\n".into(), false);
    doc.set_selection(Selection::caret(7));
    let applied = doc
        .reload_clean("longer first\ntarget\ntail\n", "new")
        .unwrap()
        .unwrap();
    assert_eq!(applied.first_line, 0);
    assert_eq!(doc.caret(), 14);
    assert!(!doc.is_dirty());
    assert_eq!(doc.disk_revision(), Some("new"));
    doc.undo().unwrap();
    assert_eq!(doc.as_str(), "first\ntarget\ntail\n");
    assert_eq!(doc.caret(), 7);
    assert!(doc.is_dirty());
    doc.redo().unwrap();
    assert!(!doc.is_dirty());
}

#[test]
fn matching_text_changes_neither_version_nor_history() {
    let mut doc = Document::from_string("unchanged\r\n".into(), false);
    let version = doc.version();
    assert_eq!(doc.reload_clean("unchanged\r\n", "new").unwrap(), None);
    assert_eq!(doc.version(), version);
    assert!(!doc.can_undo());
    assert_eq!(doc.disk_revision(), Some("new"));
}

#[test]
fn inserting_lines_at_the_caret_keeps_it_with_the_original_text() {
    let mut doc = Document::from_string("target\n".into(), false);
    doc.reload_clean("intro\ntarget\n", "new").unwrap();
    assert_eq!(doc.caret(), "intro\n".len());
    doc.undo().unwrap();
    assert_eq!(doc.caret(), 0);
}

#[test]
fn dirty_buffers_are_refused_and_read_only_buffers_still_follow_disk() {
    let mut dirty = Document::from_string("original".into(), false);
    dirty.insert("X", Origin::Input).unwrap();
    assert_eq!(
        dirty.reload_clean("external", "new"),
        Err(EditError::UnsavedChanges)
    );
    assert_eq!(dirty.as_str(), "Xoriginal");
    let mut read_only = Document::from_string("original".into(), true);
    read_only.reload_clean("external", "new").unwrap();
    assert_eq!(read_only.as_str(), "external");
    assert!(read_only.is_read_only());
    assert!(!read_only.is_dirty());
    assert_eq!(
        read_only.insert("X", Origin::Input),
        Err(EditError::ReadOnly)
    );
}

#[test]
fn unicode_boundaries_and_multiple_carets_survive_a_reload() {
    for (old, new) in [
        ("aéz", "aêz"),
        ("a中z", "a文z"),
        ("éz", "êz"),
        ("a", ""),
        ("", "中"),
    ] {
        let mut doc = Document::from_string(old.into(), false);
        doc.set_selection(Selection::many(
            vec![Cursor::caret(0), Cursor::caret(old.len())],
            0,
        ));
        doc.reload_clean(new, "new").unwrap();
        assert_eq!(doc.as_str(), new);
        for cursor in doc.selection().cursors() {
            assert!(new.is_char_boundary(cursor.head));
        }
        doc.undo().unwrap();
        assert_eq!(doc.as_str(), old);
    }
}

#[test]
fn the_budget_is_checked_before_building_a_reload_transaction() {
    let mut doc = Document::from_string("original".into(), false);
    let oversized = "x".repeat(editor_core::limits::MAX_DOCUMENT_BYTES + 1);
    assert!(matches!(
        doc.reload_clean(&oversized, "new"),
        Err(EditError::TooLarge { .. })
    ));
    assert_eq!(doc.as_str(), "original");
    assert!(!doc.can_undo());
}
