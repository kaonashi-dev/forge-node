//! Display-cell arithmetic for the viewport.
//!
//! A cell grid has no DOM: a wide grapheme occupies two cells, a combining mark
//! none, and a control byte in the file must never reach the terminal as one
//! (plan §11). Every function here is pure, because a window one cell off is
//! invisible until it is not.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Columns a tab advances to. Four is the terminal convention; the web editor's
/// two-space `indentUnit` never applied to tabs.
pub const TAB_WIDTH: usize = 4;

/// The printable stand-in for a control character.
///
/// Rendering the byte itself is how file content becomes a terminal command.
/// C0 maps to Unicode's control pictures (ESC becomes ␛), DEL and C1 to U+FFFD.
fn control_glyph(ch: char) -> char {
    match ch {
        '\u{7f}' => '\u{2421}',
        c if c < ' ' => char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}'),
        _ => '\u{fffd}',
    }
}

fn single_control(grapheme: &str) -> Option<char> {
    let mut chars = grapheme.chars();
    let first = chars.next()?;
    if chars.next().is_none() && first.is_control() {
        Some(first)
    } else {
        None
    }
}

/// Width of one grapheme starting at display column `cell`, tabs and controls
/// included.
pub fn grapheme_width(grapheme: &str, cell: usize) -> usize {
    if grapheme == "\t" {
        return TAB_WIDTH - cell % TAB_WIDTH;
    }
    if single_control(grapheme).is_some() {
        return 1;
    }
    UnicodeWidthStr::width(grapheme)
}

/// Number of graphemes in `line` — the cursor's unit of movement.
pub fn grapheme_count(line: &str) -> usize {
    line.graphemes(true).count()
}

/// Display column of a grapheme index within `line`.
pub fn display_col(line: &str, grapheme_index: usize) -> usize {
    let mut cell = 0;
    for grapheme in line.graphemes(true).take(grapheme_index) {
        cell += grapheme_width(grapheme, cell);
    }
    cell
}

/// The cells `[left, left + width)` of `line`, ready to write.
///
/// A grapheme straddling an edge becomes spaces: half a wide character cannot
/// be drawn, and silently dropping cells would shift every column after it.
pub fn cell_window(line: &str, left: usize, width: usize) -> String {
    let mut out = String::new();
    let mut cell = 0;
    let mut produced = 0;

    for grapheme in line.graphemes(true) {
        let w = grapheme_width(grapheme, cell);
        let start = cell;
        cell += w;

        if cell <= left {
            continue;
        }
        if start < left {
            let pad = (cell - left).min(width - produced);
            push_spaces(&mut out, pad);
            produced += pad;
        } else if produced + w > width {
            push_spaces(&mut out, width - produced);
            produced = width;
        } else if grapheme == "\t" {
            push_spaces(&mut out, w);
            produced += w;
        } else if let Some(ch) = single_control(grapheme) {
            out.push(control_glyph(ch));
            produced += 1;
        } else {
            out.push_str(grapheme);
            produced += w;
        }

        if produced >= width {
            break;
        }
    }
    out
}

fn push_spaces(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(line: &str) -> usize {
        display_col(line, usize::MAX)
    }

    #[test]
    fn wide_graphemes_straddling_an_edge_become_spaces() {
        let line = "a中b";
        assert_eq!(cell_window(line, 0, 4), "a中b");
        assert_eq!(cell_window(line, 1, 2), "中");
        assert_eq!(cell_window(line, 2, 2), " b");
        assert_eq!(cell_window(line, 0, 2), "a ");
        assert_eq!(width(line), 4);
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(cell_window("a\tb", 0, 5), "a   b");
        assert_eq!(cell_window("\tb", 0, 5), "    b");
        assert_eq!(width("\t"), TAB_WIDTH);
    }

    #[test]
    fn combining_marks_do_not_take_cells() {
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(grapheme_count("e\u{301}"), 1);
        assert_eq!(cell_window("e\u{301}x", 0, 2), "e\u{301}x");
    }

    #[test]
    fn control_characters_render_as_pictures() {
        assert_eq!(cell_window("\u{1b}[31m", 0, 6), "␛[31m");
        assert_eq!(cell_window("\u{7f}", 0, 1), "␡");
        assert_eq!(grapheme_width("\u{1b}", 0), 1);
    }

    #[test]
    fn a_narrow_window_shows_nothing_when_width_is_zero() {
        assert_eq!(cell_window("abc", 3, 0), "");
    }
}
