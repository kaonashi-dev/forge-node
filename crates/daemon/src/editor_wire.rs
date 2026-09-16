//! The boundary between the editor's control wire and Forge's own types.
//!
//! `editor-control` carries no Forge types and `domain` carries no editor
//! ones — that is what keeps `forge-editor` independently distributable — so
//! something has to convert, and this is it. Nothing here decides anything;
//! the clamps that matter already happened in the producer.

use domain::{
    EditorDecoration, EditorDecorationKind, EditorFold, EditorFrame, EditorInputEvent, EditorKey,
    EditorMark, EditorPlace, EditorPointer, EditorRange, EditorRow, EditorScope, EditorSeverity,
    EditorSpan, MAX_EDITOR_INPUT_EVENTS, MAX_EDITOR_TEXT_BYTES,
};
use editor_control::{
    EditorInput, ViewFrame, WireCaret, WireDecorationKind, WireKey, WireMarkKind, WireMouseKind,
    WireRange, WireScope, WireSeverityLevel,
};
use std::sync::Arc;

/// One published window, as the client wire carries it.
#[must_use]
pub fn frame_to_domain(frame: ViewFrame) -> EditorFrame {
    EditorFrame {
        buffer_id: frame.buffer_id,
        doc_version: frame.doc_version,
        first_line: frame.first_line,
        total_lines: frame.total_lines,
        clipped: frame.clipped,
        rows: Arc::new(
            frame
                .rows
                .into_iter()
                .map(|row| EditorRow {
                    line: row.line,
                    truncated: row.truncated,
                    mark: row.mark.and_then(mark),
                    diagnostic: row.diagnostic.and_then(severity),
                    fold: row.fold,
                    spans: Arc::new(
                        row.spans
                            .into_iter()
                            .map(|span| EditorSpan {
                                text: span.text,
                                scope: scope(span.scope),
                            })
                            .collect(),
                    ),
                })
                .collect(),
        ),
        folded: frame
            .folded
            .into_iter()
            .map(|fold| EditorFold {
                header: fold.header,
                hidden: fold.hidden,
            })
            .collect(),
        caret: place(frame.caret),
        selection: frame.selection.into_iter().map(range).collect(),
        extra_carets: frame.extra_carets.into_iter().map(place).collect(),
        decorations: frame
            .decorations
            .into_iter()
            .filter_map(|decoration| {
                Some(EditorDecoration {
                    kind: match decoration.kind {
                        WireDecorationKind::Match => EditorDecorationKind::Match,
                        WireDecorationKind::ActiveMatch => EditorDecorationKind::ActiveMatch,
                        WireDecorationKind::Bracket => EditorDecorationKind::Bracket,
                        // A decoration a newer editor knows and this daemon
                        // does not is dropped, never relabelled: a wrong
                        // colour is worse than no colour.
                        _ => return None,
                    },
                    range: range(decoration.range),
                })
            })
            .collect(),
    }
}

/// What a client sent, clamped before it reaches the editor's socket.
///
/// Both caps bite here and not only in the editor: the daemon is what a
/// client can reach, so a burst or a paste over budget is truncated at the
/// boundary rather than allocated and forwarded (`docs/performance.md`).
#[must_use]
pub fn input_to_wire(events: Vec<EditorInputEvent>) -> Vec<EditorInput> {
    events
        .into_iter()
        .take(MAX_EDITOR_INPUT_EVENTS)
        .filter_map(|event| {
            Some(match event {
                EditorInputEvent::Key { key, modifiers } => EditorInput::Key {
                    key: wire_key(key)?,
                    modifiers,
                },
                EditorInputEvent::Text(text) => EditorInput::Text(clamp_text(text)),
                EditorInputEvent::Pointer {
                    kind,
                    line,
                    column,
                    modifiers,
                } => EditorInput::Mouse {
                    kind: match kind {
                        EditorPointer::Down => WireMouseKind::Down,
                        EditorPointer::Drag => WireMouseKind::Drag,
                        EditorPointer::Up => WireMouseKind::Up,
                        _ => return None,
                    },
                    line,
                    column,
                    modifiers,
                },
                EditorInputEvent::Wheel { lines } => EditorInput::Wheel { lines },
                EditorInputEvent::Focus(has) => EditorInput::Focus(has),
                _ => return None,
            })
        })
        .collect()
}

/// Cut committed text on a char boundary, never mid-code-point.
fn clamp_text(mut text: String) -> String {
    if text.len() <= MAX_EDITOR_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_EDITOR_TEXT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

fn wire_key(key: EditorKey) -> Option<WireKey> {
    Some(match key {
        EditorKey::Char(ch) => WireKey::Char(ch),
        EditorKey::Enter => WireKey::Enter,
        EditorKey::Tab => WireKey::Tab,
        EditorKey::Backspace => WireKey::Backspace,
        EditorKey::Delete => WireKey::Delete,
        EditorKey::Escape => WireKey::Escape,
        EditorKey::Left => WireKey::Left,
        EditorKey::Right => WireKey::Right,
        EditorKey::Up => WireKey::Up,
        EditorKey::Down => WireKey::Down,
        EditorKey::Home => WireKey::Home,
        EditorKey::End => WireKey::End,
        EditorKey::PageUp => WireKey::PageUp,
        EditorKey::PageDown => WireKey::PageDown,
        EditorKey::Insert => WireKey::Insert,
        EditorKey::Function(n) => WireKey::Function(n),
        _ => return None,
    })
}

/// A gutter mark, or nothing when a newer editor named one this build does
/// not know: an unrecognised mark is better absent than shown as the wrong
/// colour next to a line somebody is about to trust.
fn mark(kind: WireMarkKind) -> Option<EditorMark> {
    Some(match kind {
        WireMarkKind::Added => EditorMark::Added,
        WireMarkKind::Modified => EditorMark::Modified,
        WireMarkKind::Deleted => EditorMark::Deleted,
    })
}

fn severity(level: WireSeverityLevel) -> Option<EditorSeverity> {
    Some(match level {
        WireSeverityLevel::Error => EditorSeverity::Error,
        WireSeverityLevel::Warning => EditorSeverity::Warning,
        WireSeverityLevel::Info => EditorSeverity::Info,
        _ => return None,
    })
}

fn scope(scope: WireScope) -> EditorScope {
    match scope {
        WireScope::Comment => EditorScope::Comment,
        WireScope::Keyword => EditorScope::Keyword,
        WireScope::ControlKeyword => EditorScope::ControlKeyword,
        WireScope::String => EditorScope::String,
        WireScope::Number => EditorScope::Number,
        WireScope::Type => EditorScope::Type,
        WireScope::Function => EditorScope::Function,
        WireScope::Property => EditorScope::Property,
        WireScope::Constant => EditorScope::Constant,
        // Plain, and anything a newer editor added: an unknown colour is no
        // colour, which still reads.
        _ => EditorScope::Plain,
    }
}

fn place(caret: WireCaret) -> EditorPlace {
    EditorPlace {
        line: caret.line,
        column: caret.column,
    }
}

fn range(range: WireRange) -> EditorRange {
    EditorRange {
        from: place(range.from),
        to: place(range.to),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_control::{ViewRow, WireSpan};

    #[test]
    fn a_window_survives_the_conversion() {
        let frame = ViewFrame {
            buffer_id: 3,
            doc_version: 11,
            first_line: 5,
            total_lines: 90,
            clipped: true,
            rows: vec![ViewRow {
                line: 5,
                truncated: false,
                mark: Some(WireMarkKind::Added),
                diagnostic: Some(WireSeverityLevel::Error),
                fold: Some(0),
                spans: vec![
                    WireSpan {
                        text: "fn".into(),
                        scope: WireScope::Keyword,
                    },
                    WireSpan::plain(" go"),
                ],
            }],
            folded: Vec::new(),
            caret: WireCaret { line: 5, column: 2 },
            selection: Vec::new(),
            extra_carets: Vec::new(),
            decorations: Vec::new(),
        };
        let converted = frame_to_domain(frame);
        assert_eq!(converted.rows[0].text(), "fn go");
        assert_eq!(converted.rows[0].spans[0].scope, EditorScope::Keyword);
        assert_eq!(converted.caret.column, 2);
        assert_eq!(converted.rows[0].mark, Some(EditorMark::Added));
        assert_eq!(converted.rows[0].diagnostic, Some(EditorSeverity::Error));
        assert_eq!(converted.rows[0].fold, Some(0), "foldable and open");
        assert!(converted.clipped);
    }

    #[test]
    fn a_burst_over_the_cap_is_truncated_at_the_boundary() {
        let events: Vec<EditorInputEvent> = (0..MAX_EDITOR_INPUT_EVENTS * 2)
            .map(|_| EditorInputEvent::Key {
                key: EditorKey::Char('a'),
                modifiers: 0,
            })
            .collect();
        assert_eq!(input_to_wire(events).len(), MAX_EDITOR_INPUT_EVENTS);
    }

    /// A paste larger than the cap is cut, and cut where a code point ends:
    /// half of a character is not text the editor can insert.
    #[test]
    fn committed_text_is_clamped_on_a_char_boundary() {
        let text = "é".repeat(MAX_EDITOR_TEXT_BYTES);
        let wire = input_to_wire(vec![EditorInputEvent::Text(text)]);
        match &wire[0] {
            EditorInput::Text(clamped) => {
                assert!(clamped.len() <= MAX_EDITOR_TEXT_BYTES);
                assert!(clamped.chars().all(|ch| ch == 'é'));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }
}
