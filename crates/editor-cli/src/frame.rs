//! Builds the window the headless host publishes.
//!
//! The DOM surface mounts one node per visible line, so what leaves here is a
//! *window of lines* — text, scopes and ranges — never a cell grid. Positions
//! travel as UTF-16 code units because the only consumer is a browser, and a
//! byte offset into UTF-8 is not something `String.prototype.slice` can take.
//!
//! Every count is capped while the frame is built, not after it is encoded:
//! the caps in `editor_control::view` are what keep one long line from
//! deciding the size of a frame.

use editor_control::{
    ViewFrame, ViewRow, WireCaret, WireDecoration, WireDecorationKind, WireFold, WireRange,
    WireScope, WireSeverity, WireSeverityLevel, WireSpan, MAX_DECORATIONS, MAX_ROW_BYTES,
    MAX_ROW_SPANS, MAX_VIEW_ROWS, SPAN_OVERHEAD_BYTES, VIEW_ROW_BUDGET,
};
use editor_core::Scope;

use crate::app::App;
use crate::view::Decoration;

/// The window `app` currently shows, as the GUI paints it.
#[must_use]
pub fn build(app: &App, buffer_id: u64) -> ViewFrame {
    let text = app.document().text();
    let total_lines = text.line_count();
    let first_line = app.top().min(total_lines.saturating_sub(1));
    let wanted = app.content_height().min(MAX_VIEW_ROWS);

    let mut rows = Vec::with_capacity(wanted);
    let mut folded = Vec::new();
    let mut decorations = Vec::new();
    let mut spent = 0;
    let mut clipped = false;
    let mut line = first_line;
    while rows.len() < wanted && line < total_lines {
        // A folded block contributes its header and nothing else; the GUI
        // needs the count to draw "… 12 lines", not the lines themselves.
        if app.is_hidden(line) {
            line += 1;
            continue;
        }
        if app.fold_state(line) == Some(true) {
            folded.push(WireFold {
                header: line as u32,
                hidden: app.folded_line_count(line) as u32,
            });
        }
        let source = text.line(line);
        let (shown, truncated) = clamp_row(source);
        let spans = spans_of(shown, app.syntax_line(line));
        // Charged before the row is kept, not after the frame is encoded: the
        // per-row caps bound one line, this is what bounds the frame.
        let cost = shown.len() + spans.len() * SPAN_OVERHEAD_BYTES;
        if !rows.is_empty() && spent + cost > VIEW_ROW_BUDGET {
            clipped = true;
            break;
        }
        spent += cost;
        rows.push(ViewRow {
            line: line as u32,
            truncated,
            spans,
            // The gutter counts from 1, the way git and every checker do.
            mark: app.mark_at(line + 1),
            diagnostic: app.diagnostic_at(line + 1).map(|item| match item.severity {
                WireSeverity::Error => WireSeverityLevel::Error,
                WireSeverity::Warning => WireSeverityLevel::Warning,
                WireSeverity::Info => WireSeverityLevel::Info,
            }),
            fold: app.fold_state(line).map(|folded| {
                if folded {
                    app.folded_line_count(line) as u32
                } else {
                    0
                }
            }),
        });
        push_decorations(app, line, shown, &mut decorations);
        line += 1;
    }

    let caret_offset = app.document().caret();
    let selection = app.document().selection();
    let mut carets = Vec::new();
    let mut ranges = Vec::new();
    for cursor in selection.cursors() {
        if cursor.head != caret_offset {
            carets.push(place(app, cursor.head));
        }
        if !cursor.is_empty() {
            let range = cursor.range();
            ranges.push(WireRange {
                from: place(app, range.start),
                to: place(app, range.end),
            });
        }
    }

    ViewFrame {
        buffer_id,
        doc_version: app.document().version().0,
        first_line: first_line as u32,
        total_lines: total_lines as u32,
        rows,
        clipped,
        folded,
        caret: place(app, caret_offset),
        selection: ranges,
        extra_carets: carets,
        decorations,
    }
}

/// The prefix of `line` that travels, and whether anything was cut.
///
/// Cut on a char boundary: half a code point is not text, and the GUI would
/// have to guess what the tail was.
fn clamp_row(line: &str) -> (&str, bool) {
    if line.len() <= MAX_ROW_BYTES {
        return (line, false);
    }
    let mut end = MAX_ROW_BYTES;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    (&line[..end], true)
}

/// Split `line` where its colouring changes.
///
/// Past [`MAX_ROW_SPANS`] the row is sent as one plain run: text without
/// highlighting still reads, and a row with a thousand spans is a minified
/// file, not something a person is looking at.
fn spans_of(line: &str, scopes: &[editor_core::Span]) -> Vec<WireSpan> {
    if line.is_empty() {
        return Vec::new();
    }
    if scopes.is_empty() {
        return vec![WireSpan::plain(line)];
    }
    let mut spans: Vec<WireSpan> = Vec::new();
    let mut at = 0;
    for span in scopes {
        let start = span.start.min(line.len());
        let end = span.end.min(line.len());
        if end <= at || start >= line.len() {
            continue;
        }
        if start > at {
            push_run(&mut spans, &line[at..start], WireScope::Plain);
        }
        push_run(&mut spans, &line[start.max(at)..end], scope_of(span.scope));
        at = end;
        if spans.len() > MAX_ROW_SPANS {
            return vec![WireSpan::plain(line)];
        }
    }
    if at < line.len() {
        push_run(&mut spans, &line[at..], WireScope::Plain);
    }
    spans
}

/// Append a run, merging it into the last one when the scope did not change.
fn push_run(spans: &mut Vec<WireSpan>, text: &str, scope: WireScope) {
    if text.is_empty() {
        return;
    }
    match spans.last_mut() {
        Some(last) if last.scope == scope => last.text.push_str(text),
        _ => spans.push(WireSpan {
            text: text.to_string(),
            scope,
        }),
    }
}

fn scope_of(scope: Scope) -> WireScope {
    match scope {
        Scope::Comment => WireScope::Comment,
        Scope::Keyword => WireScope::Keyword,
        Scope::ControlKeyword => WireScope::ControlKeyword,
        Scope::String => WireScope::String,
        Scope::Number => WireScope::Number,
        Scope::Type => WireScope::Type,
        Scope::Function => WireScope::Function,
        Scope::Property => WireScope::Property,
        Scope::Constant => WireScope::Constant,
        Scope::Plain => WireScope::Plain,
    }
}

/// The find hits and bracket pairs on one row, in the window's coordinates.
fn push_decorations(app: &App, line: usize, shown: &str, out: &mut Vec<WireDecoration>) {
    for (from, to, what) in app.marks_in_line(line) {
        if out.len() >= MAX_DECORATIONS {
            return;
        }
        let kind = match what {
            Decoration::Match => WireDecorationKind::Match,
            Decoration::Bracket => WireDecorationKind::Bracket,
            // An extra caret is a caret, not a decoration: the DOM draws it as
            // one, and the terminal only painted it as a cell because it has a
            // single hardware cursor.
            Decoration::Caret | Decoration::None => continue,
        };
        out.push(WireDecoration {
            kind,
            range: WireRange {
                from: WireCaret {
                    line: line as u32,
                    column: utf16_column(shown, from),
                },
                to: WireCaret {
                    line: line as u32,
                    column: utf16_column(shown, to),
                },
            },
        });
    }
}

/// A document offset as the line and UTF-16 column a browser can index.
fn place(app: &App, offset: usize) -> WireCaret {
    let text = app.document().text();
    let position = text.line_col(offset);
    WireCaret {
        line: position.line as u32,
        column: utf16_column(text.line(position.line), position.column),
    }
}

/// UTF-16 code units in `line` before byte offset `column`.
fn utf16_column(line: &str, column: usize) -> u32 {
    let end = column.min(line.len());
    let counted: usize = line
        .get(..end)
        .unwrap_or(line)
        .chars()
        .map(char::len_utf16)
        .sum();
    counted as u32
}

/// The wire's caret cap is the document model's, so nothing here has to
/// enforce a second one: `extra_carets` and `selection` both come from a
/// `Selection` the core already bounded.
const _: () = assert!(editor_control::MAX_CARETS == editor_core::limits::MAX_CURSORS);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_row_is_cut_on_a_char_boundary() {
        let line = "é".repeat(MAX_ROW_BYTES);
        let (shown, truncated) = clamp_row(&line);
        assert!(truncated);
        assert!(shown.len() <= MAX_ROW_BYTES);
        assert!(line.starts_with(shown), "the prefix is still the line's");
    }

    #[test]
    fn spans_reassemble_into_the_row() {
        let line = "fn main() {}";
        let scopes = [
            editor_core::Span {
                start: 0,
                end: 2,
                scope: Scope::Keyword,
            },
            editor_core::Span {
                start: 3,
                end: 7,
                scope: Scope::Function,
            },
        ];
        let spans = spans_of(line, &scopes);
        let joined: String = spans.iter().map(|span| span.text.as_str()).collect();
        assert_eq!(joined, line);
        assert_eq!(spans[0].scope, WireScope::Keyword);
        assert_eq!(spans[1].scope, WireScope::Plain);
        assert_eq!(spans[2].scope, WireScope::Function);
    }

    /// A minified line is sent plain rather than as a thousand nodes: the row
    /// still reads, and the frame stays the size of a viewport.
    #[test]
    fn a_row_past_the_span_cap_is_sent_plain() {
        let line = "ab".repeat(MAX_ROW_SPANS + 8);
        let scopes: Vec<_> = (0..line.len() / 2)
            .map(|i| editor_core::Span {
                start: i * 2,
                end: i * 2 + 1,
                scope: Scope::Keyword,
            })
            .collect();
        let spans = spans_of(&line, &scopes);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].scope, WireScope::Plain);
        assert_eq!(spans[0].text, line);
    }

    #[test]
    fn a_column_counts_utf16_units_not_bytes() {
        // An astral character is two UTF-16 units and four bytes; a browser
        // indexes the first number.
        assert_eq!(utf16_column("a😀b", 5), 3);
        assert_eq!(utf16_column("a😀b", 1), 1);
        assert_eq!(utf16_column("héllo", 3), 2);
    }
}
