//! Caret movement over the buffer, in graphemes and display columns.
//!
//! Every function is a pure offset → offset map, so a key binding, the palette
//! and a Vim operator can share one definition of "word left". Nothing here
//! scrolls: the viewport is the view's, and moving the caret must not cost
//! work proportional to the document.

use crate::metrics::{self, byte_column_for_display, display_column};
use crate::selection::Selection;
use crate::text::Text;

/// Move one grapheme left, crossing into the previous line at column 0.
#[must_use]
pub fn left(text: &Text, offset: usize) -> usize {
    let line = text.line_of_offset(offset);
    let start = text.line_start(line);
    if offset > start {
        start + metrics::previous_boundary(text.line(line), offset - start)
    } else if line > 0 {
        text.line_end(line - 1)
    } else {
        0
    }
}

/// Move one grapheme right, crossing into the next line past its end.
#[must_use]
pub fn right(text: &Text, offset: usize) -> usize {
    let line = text.line_of_offset(offset);
    let end = text.line_end(line);
    if offset < end {
        text.line_start(line)
            + metrics::next_boundary(text.line(line), offset - text.line_start(line))
    } else if line + 1 < text.line_count() {
        text.line_start(line + 1)
    } else {
        end
    }
}

/// Move `delta` lines, keeping the display column the caret is aiming for.
///
/// Returns the new offset and the goal column to carry into the next move; a
/// goal recomputed from the caret collapses on the first short line.
#[must_use]
pub fn vertical(text: &Text, offset: usize, delta: isize, goal: Option<usize>) -> (usize, usize) {
    let line = text.line_of_offset(offset);
    let column = text.line_col(offset).column;
    let goal = goal.unwrap_or_else(|| display_column(text.line(line), column));
    let last = text.line_count() - 1;
    let target = (line as isize)
        .saturating_add(delta)
        .clamp(0, last as isize) as usize;
    let body = text.line(target);
    (
        text.line_start(target) + byte_column_for_display(body, goal),
        goal,
    )
}

#[must_use]
pub fn line_start(text: &Text, offset: usize) -> usize {
    text.line_start(text.line_of_offset(offset))
}

/// First non-blank character of the line, or its start when already there.
#[must_use]
pub fn line_first_non_blank(text: &Text, offset: usize) -> usize {
    let line = text.line_of_offset(offset);
    let start = text.line_start(line);
    let body = text.line(line);
    let indent = body.len() - body.trim_start().len();
    if start + indent == offset {
        start
    } else {
        start + indent
    }
}

#[must_use]
pub fn line_end(text: &Text, offset: usize) -> usize {
    text.line_end(text.line_of_offset(offset))
}

/// Start of the word before `offset`, crossing at most one line break.
#[must_use]
pub fn word_left(text: &Text, offset: usize) -> usize {
    let body = text.as_str();
    let mut at = offset.min(body.len());
    while at > 0 && !is_word(char_before(body, at)) {
        let crossed = char_before(body, at);
        at = previous_char(body, at);
        if crossed == Some('\n') {
            break;
        }
    }
    while at > 0 && is_word(char_before(body, at)) {
        at = previous_char(body, at);
    }
    at
}

/// Start of the word after `offset`, crossing at most one line break.
#[must_use]
pub fn word_right(text: &Text, offset: usize) -> usize {
    let body = text.as_str();
    let mut at = offset.min(body.len());
    while at < body.len() && is_word(char_at(body, at)) {
        at = next_char(body, at);
    }
    while at < body.len() && !is_word(char_at(body, at)) {
        let crossed = char_at(body, at);
        at = next_char(body, at);
        if crossed == Some('\n') {
            break;
        }
    }
    at
}

/// The word `offset` is inside or touching, as a byte range.
///
/// Empty when there is no word there: `Ctrl-D` on a run of spaces has nothing
/// to select, and returning the spaces would make the next press search for
/// them.
#[must_use]
pub fn word_span(text: &Text, offset: usize) -> (usize, usize) {
    let body = text.as_str();
    let at = offset.min(body.len());
    let inside = char_at(body, at).is_some_and(is_word_char);
    let touching = !inside && char_before(body, at).is_some_and(is_word_char);
    if !inside && !touching {
        return (at, at);
    }
    let mut start = at;
    while start > 0 && char_before(body, start).is_some_and(is_word_char) {
        start = previous_char(body, start);
    }
    let mut end = at;
    while end < body.len() && char_at(body, end).is_some_and(is_word_char) {
        end = next_char(body, end);
    }
    (start, end)
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Whole lines covering the selection, terminator included when there is one.
#[must_use]
pub fn line_span(text: &Text, selection: Selection) -> (usize, usize) {
    let range = selection.range();
    let first = text.line_of_offset(range.start);
    let last = text.line_of_offset(range.end);
    let start = text.line_start(first);
    let end = if last + 1 < text.line_count() {
        text.line_start(last + 1)
    } else {
        text.len()
    };
    (start, end)
}

fn is_word(ch: Option<char>) -> bool {
    ch.is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}

fn char_at(body: &str, at: usize) -> Option<char> {
    body[at..].chars().next()
}

fn char_before(body: &str, at: usize) -> Option<char> {
    body[..at].chars().next_back()
}

fn next_char(body: &str, at: usize) -> usize {
    match body[at..].chars().next() {
        Some(ch) => at + ch.len_utf8(),
        None => at,
    }
}

fn previous_char(body: &str, at: usize) -> usize {
    match body[..at].chars().next_back() {
        Some(ch) => at - ch.len_utf8(),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(body: &str) -> Text {
        Text::new(body.to_string())
    }

    #[test]
    fn horizontal_movement_crosses_lines_and_whole_graphemes() {
        let body = text("a🇪🇸\nb");
        let flag = "🇪🇸".len();
        assert_eq!(right(&body, 0), 1);
        assert_eq!(right(&body, 1), 1 + flag);
        assert_eq!(right(&body, 1 + flag), 1 + flag + 1);
        assert_eq!(left(&body, 1 + flag + 1), 1 + flag);
        assert_eq!(left(&body, 0), 0);
    }

    #[test]
    fn the_goal_column_survives_a_short_line() {
        let body = text("abcdef\nx\nabcdef");
        let (offset, goal) = vertical(&body, 5, 1, None);
        assert_eq!(goal, 5);
        assert_eq!(body.line_col(offset), crate::selection::LineCol::new(1, 1));
        let (offset, _) = vertical(&body, offset, 1, Some(goal));
        assert_eq!(body.line_col(offset), crate::selection::LineCol::new(2, 5));
    }

    #[test]
    fn vertical_movement_lands_on_a_grapheme_start() {
        let body = text("a中b\nxxxx");
        let (offset, _) = vertical(&body, body.len(), -1, Some(2));
        assert_eq!(offset, 1);
    }

    #[test]
    fn a_crlf_line_end_stops_before_the_carriage_return() {
        let body = text("ab\r\ncd");
        assert_eq!(line_end(&body, 0), 2);
        assert_eq!(right(&body, 2), 4);
    }

    #[test]
    fn words_step_over_separators_but_stop_at_a_line_break() {
        let body = text("one  two\nthree");
        assert_eq!(word_right(&body, 0), 5);
        assert_eq!(word_right(&body, 5), 9);
        assert_eq!(word_right(&body, 9), 14);
        assert_eq!(word_left(&body, 8), 5);
        assert_eq!(word_left(&body, 5), 0);
        assert_eq!(word_left(&body, 9), 5);
    }

    #[test]
    fn home_toggles_between_the_indent_and_the_margin() {
        let body = text("    value");
        assert_eq!(line_first_non_blank(&body, 9), 4);
        assert_eq!(line_first_non_blank(&body, 4), 0);
    }

    #[test]
    fn a_line_span_includes_the_terminator_when_one_exists() {
        let body = text("a\nb\nc");
        assert_eq!(line_span(&body, Selection::caret(0)), (0, 2));
        assert_eq!(line_span(&body, Selection::new(0, 3)), (0, 4));
        assert_eq!(line_span(&body, Selection::caret(4)), (4, 5));
    }
}
