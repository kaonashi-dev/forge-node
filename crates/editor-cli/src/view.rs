//! Display-cell arithmetic for the viewport.
//!
//! A cell grid has no DOM: a wide grapheme occupies two cells, a combining mark
//! none, and a control byte in the file must never reach the terminal as one.
//! Everything here is pure — a window one cell off is invisible until it is not.

use editor_core::metrics::{grapheme_width, single_control};
use unicode_segmentation::UnicodeSegmentation;

/// A run of cells that share one highlight state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub text: String,
    pub selected: bool,
}

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

/// The cells `[left, left + width)` of `line`, split where the selection starts
/// and ends. `selected` is a byte range within `line`.
///
/// A grapheme straddling an edge becomes spaces: half a wide character cannot
/// be drawn, and dropping the cells would shift every column after it.
#[must_use]
pub fn window_parts(
    line: &str,
    left: usize,
    width: usize,
    selected: Option<(usize, usize)>,
) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    let mut cell = 0;
    let mut produced = 0;

    let push = |text: &str, selected: bool, parts: &mut Vec<Part>| match parts.last_mut() {
        Some(last) if last.selected == selected => last.text.push_str(text),
        _ => parts.push(Part {
            text: text.to_string(),
            selected,
        }),
    };

    for (offset, grapheme) in line.grapheme_indices(true) {
        if produced >= width {
            break;
        }
        let w = grapheme_width(grapheme, cell);
        let start = cell;
        cell += w;
        if cell <= left {
            continue;
        }
        let inside = selected.is_some_and(|(from, to)| offset >= from && offset < to);

        if start < left {
            let pad = (cell - left).min(width - produced);
            push(&" ".repeat(pad), inside, &mut parts);
            produced += pad;
        } else if produced + w > width {
            push(&" ".repeat(width - produced), inside, &mut parts);
            produced = width;
        } else if grapheme == "\t" {
            push(&" ".repeat(w), inside, &mut parts);
            produced += w;
        } else if let Some(ch) = single_control(grapheme) {
            push(&control_glyph(ch).to_string(), inside, &mut parts);
            produced += 1;
        } else {
            push(grapheme, inside, &mut parts);
            produced += w;
        }
    }
    // An empty selection at the end of a line still has to read as selected, so
    // the trailing cell is padded when the range reaches past the last grapheme.
    if let Some((from, to)) = selected {
        if to > line.len() && from <= line.len() && produced < width {
            push(" ", true, &mut parts);
        }
    }
    parts
}

/// The cells `[left, left + width)` of `line`, with no highlighting.
#[must_use]
pub fn cell_window(line: &str, left: usize, width: usize) -> String {
    window_parts(line, left, width, None)
        .into_iter()
        .map(|part| part.text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width_of(line: &str) -> usize {
        editor_core::metrics::display_column(line, line.len())
    }

    #[test]
    fn wide_graphemes_straddling_an_edge_become_spaces() {
        let line = "a中b";
        assert_eq!(cell_window(line, 0, 4), "a中b");
        assert_eq!(cell_window(line, 1, 2), "中");
        assert_eq!(cell_window(line, 2, 2), " b");
        assert_eq!(cell_window(line, 0, 2), "a ");
        assert_eq!(width_of(line), 4);
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(cell_window("a\tb", 0, 5), "a   b");
        assert_eq!(cell_window("\tb", 0, 5), "    b");
        assert_eq!(width_of("\t"), editor_core::metrics::TAB_WIDTH);
    }

    #[test]
    fn combining_marks_do_not_take_cells() {
        assert_eq!(width_of("e\u{301}"), 1);
        assert_eq!(cell_window("e\u{301}x", 0, 2), "e\u{301}x");
    }

    #[test]
    fn control_characters_render_as_pictures() {
        assert_eq!(cell_window("\u{1b}[31m", 0, 6), "␛[31m");
        assert_eq!(cell_window("\u{7f}", 0, 1), "␡");
    }

    #[test]
    fn a_narrow_window_shows_nothing_when_width_is_zero() {
        assert_eq!(cell_window("abc", 3, 0), "");
    }

    #[test]
    fn a_selection_splits_the_row_into_runs() {
        let parts = window_parts("abcdef", 0, 6, Some((2, 4)));
        assert_eq!(
            parts,
            vec![
                Part {
                    text: "ab".to_string(),
                    selected: false
                },
                Part {
                    text: "cd".to_string(),
                    selected: true
                },
                Part {
                    text: "ef".to_string(),
                    selected: false
                },
            ]
        );
    }

    #[test]
    fn a_selection_that_swallows_the_line_break_shows_one_trailing_cell() {
        let parts = window_parts("ab", 0, 8, Some((0, 3)));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text, "ab ");
        assert!(parts[0].selected);
    }
}
