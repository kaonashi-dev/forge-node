//! Grapheme and display-cell conversions over byte offsets.
//!
//! A cell grid has no DOM: a wide grapheme takes two cells, a combining mark
//! none, and a tab as many as the next stop. These are conversions only — they
//! never paint, and the painter's window arithmetic lives with the view.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Columns a tab advances to. Four is the terminal convention; the web
/// editor's two-space `indentUnit` never applied to tabs.
pub const TAB_WIDTH: usize = 4;

/// Cells one grapheme occupies when it starts at display column `cell`.
///
/// A control character counts as one because it is drawn as a control picture,
/// never emitted: a file's `ESC` must not become a terminal command.
#[must_use]
pub fn grapheme_width(grapheme: &str, cell: usize) -> usize {
    if grapheme == "\t" {
        return TAB_WIDTH - cell % TAB_WIDTH;
    }
    if single_control(grapheme).is_some() {
        return 1;
    }
    UnicodeWidthStr::width(grapheme)
}

/// The control `char` of a single-control grapheme, or `None`.
#[must_use]
pub fn single_control(grapheme: &str) -> Option<char> {
    let mut chars = grapheme.chars();
    let first = chars.next()?;
    if chars.next().is_none() && first.is_control() {
        Some(first)
    } else {
        None
    }
}

#[must_use]
pub fn grapheme_count(line: &str) -> usize {
    line.graphemes(true).count()
}

/// Display column of a byte column within `line`.
#[must_use]
pub fn display_column(line: &str, byte_column: usize) -> usize {
    let mut cell = 0;
    for (offset, grapheme) in line.grapheme_indices(true) {
        if offset >= byte_column {
            break;
        }
        cell += grapheme_width(grapheme, cell);
    }
    cell
}

/// Byte column whose display column is nearest to `target`, rounded down so a
/// vertical move never lands inside a wide grapheme.
#[must_use]
pub fn byte_column_for_display(line: &str, target: usize) -> usize {
    let mut cell = 0;
    for (offset, grapheme) in line.grapheme_indices(true) {
        let width = grapheme_width(grapheme, cell);
        if cell + width > target {
            return offset;
        }
        cell += width;
    }
    line.len()
}

/// Byte offset of the grapheme boundary before `offset` within `line`.
#[must_use]
pub fn previous_boundary(line: &str, offset: usize) -> usize {
    line.grapheme_indices(true)
        .map(|(at, _)| at)
        .take_while(|&at| at < offset)
        .last()
        .unwrap_or(0)
}

/// Byte offset of the grapheme boundary after `offset` within `line`.
#[must_use]
pub fn next_boundary(line: &str, offset: usize) -> usize {
    line.grapheme_indices(true)
        .map(|(at, _)| at)
        .find(|&at| at > offset)
        .unwrap_or(line.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_grapheme_takes_two_cells_and_a_mark_none() {
        assert_eq!(display_column("a中b", "a中b".len()), 4);
        assert_eq!(display_column("e\u{301}x", "e\u{301}x".len()), 2);
        assert_eq!(grapheme_count("e\u{301}x"), 2);
    }

    #[test]
    fn a_tab_advances_to_the_next_stop_from_where_it_starts() {
        assert_eq!(grapheme_width("\t", 0), 4);
        assert_eq!(grapheme_width("\t", 1), 3);
        assert_eq!(display_column("a\tb", 3), 5);
    }

    #[test]
    fn a_display_column_inside_a_wide_grapheme_rounds_down_to_its_start() {
        let line = "a中b";
        assert_eq!(byte_column_for_display(line, 2), 1);
        assert_eq!(byte_column_for_display(line, 3), 4);
        assert_eq!(byte_column_for_display(line, 999), line.len());
    }

    #[test]
    fn boundaries_step_over_a_whole_flag() {
        let line = "a🇪🇸b";
        let flag = "🇪🇸".len();
        assert_eq!(next_boundary(line, 0), 1);
        assert_eq!(next_boundary(line, 1), 1 + flag);
        assert_eq!(previous_boundary(line, 1 + flag), 1);
    }
}
