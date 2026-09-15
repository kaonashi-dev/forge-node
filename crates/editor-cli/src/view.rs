//! Display-cell arithmetic for the viewport.
//!
//! A cell grid has no DOM: a wide grapheme occupies two cells, a combining mark
//! none, and a control byte in the file must never reach the terminal as one.
//! Everything here is pure — a window one cell off is invisible until it is not.

use editor_core::metrics::{grapheme_width, single_control};
use editor_core::{Scope, Span};
use unicode_segmentation::UnicodeSegmentation;

/// A run of cells that share one highlight state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub text: String,
    pub selected: bool,
    /// The syntax scope these cells carry. `Plain` when the grammar is unknown
    /// or has nothing to say about them.
    pub scope: Scope,
    pub decoration: Decoration,
}

/// What a run carries on top of its colour and the selection.
///
/// One slot and not a set: a cell is a search hit or a bracket, never both, and
/// a terminal has few enough attributes that stacking them would stop reading
/// as either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decoration {
    #[default]
    None,
    /// A hit for the live query, or another occurrence of the selected word.
    Match,
    /// One half of the pair the caret is next to.
    Bracket,
    /// A caret that is not the primary one, drawn as a cell.
    Caret,
}

/// A byte range inside one line and what it carries.
pub type Mark = (usize, usize, Decoration);

/// Everything a row needs besides its text and its window.
#[derive(Clone, Copy, Default)]
pub struct RowDecor<'a> {
    /// Byte range of the selection within this line, if it has one.
    pub selected: Option<(usize, usize)>,
    /// Ranges of every *other* caret's selection on this line.
    pub also_selected: &'a [(usize, usize)],
    /// Byte columns the non-primary carets sit at. A terminal has one hardware
    /// cursor, so the rest are painted as cells.
    pub carets: &'a [usize],
    pub scopes: &'a [Span],
    /// Ascending, non-overlapping byte ranges within this line.
    pub marks: &'a [Mark],
    /// Draw a placeholder where a tab or a no-break space is.
    pub special_chars: bool,
}

/// The stand-in for a whitespace character that is easy to mistake.
///
/// A tab reads as its arrow and then the spaces it bought, so the columns still
/// line up; a no-break space and a zero-width one are shown because the whole
/// problem with them is that they look like what they are not.
fn special_glyph(grapheme: &str) -> Option<char> {
    match grapheme {
        "\t" => Some('\u{2192}'),
        "\u{a0}" => Some('\u{b7}'),
        "\u{200b}" | "\u{feff}" => Some('\u{2423}'),
        _ => None,
    }
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
pub fn window_parts(line: &str, left: usize, width: usize, decor: RowDecor<'_>) -> Vec<Part> {
    let RowDecor {
        selected,
        also_selected,
        carets,
        scopes,
        marks,
        special_chars,
    } = decor;
    let mut parts: Vec<Part> = Vec::new();
    let mut cell = 0;
    let mut produced = 0;

    // Runs merge on selection, scope *and* decoration: any of the three
    // changing starts a new part, so the painter never has to split a string it
    // was handed.
    let push = |text: &str,
                selected: bool,
                scope: Scope,
                decoration: Decoration,
                parts: &mut Vec<Part>| match parts.last_mut() {
        Some(last)
            if last.selected == selected
                && last.scope == scope
                && last.decoration == decoration =>
        {
            last.text.push_str(text);
        }
        _ => parts.push(Part {
            text: text.to_string(),
            selected,
            scope,
            decoration,
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
        let inside = selected.is_some_and(|(from, to)| offset >= from && offset < to)
            || also_selected
                .iter()
                .any(|(from, to)| offset >= *from && offset < *to);
        let scope = scope_of(scopes, offset);
        let decoration = if carets.contains(&offset) {
            Decoration::Caret
        } else {
            mark_of(marks, offset)
        };

        if start < left {
            let pad = (cell - left).min(width - produced);
            push(&" ".repeat(pad), inside, scope, decoration, &mut parts);
            produced += pad;
        } else if produced + w > width {
            push(
                &" ".repeat(width - produced),
                inside,
                scope,
                decoration,
                &mut parts,
            );
            produced = width;
        } else if let Some(glyph) = special_chars.then(|| special_glyph(grapheme)).flatten() {
            // The glyph takes the grapheme's first cell; the rest of a tab's
            // width is still spaces, so nothing after it shifts.
            let mut shown = glyph.to_string();
            shown.push_str(&" ".repeat(w.saturating_sub(1)));
            push(&shown, inside, scope, decoration, &mut parts);
            produced += w.max(1);
        } else if grapheme == "\t" {
            push(&" ".repeat(w), inside, scope, decoration, &mut parts);
            produced += w;
        } else if let Some(ch) = single_control(grapheme) {
            push(
                &control_glyph(ch).to_string(),
                inside,
                scope,
                decoration,
                &mut parts,
            );
            produced += 1;
        } else {
            push(grapheme, inside, scope, decoration, &mut parts);
            produced += w;
        }
    }
    // An empty selection at the end of a line still has to read as selected, so
    // the trailing cell is padded when the range reaches past the last grapheme.
    if let Some((from, to)) = selected {
        if to > line.len() && from <= line.len() && produced < width {
            push(" ", true, Scope::Plain, Decoration::None, &mut parts);
        }
    }
    parts
}

/// The decoration covering a byte offset, `None` when none does.
fn mark_of(marks: &[Mark], offset: usize) -> Decoration {
    marks
        .iter()
        .find(|(from, to, _)| offset >= *from && offset < *to)
        .map_or(Decoration::None, |(_, _, what)| *what)
}

/// The scope covering a byte offset, `Plain` when none does.
fn scope_of(scopes: &[Span], offset: usize) -> Scope {
    scopes
        .iter()
        .find(|span| offset >= span.start && offset < span.end)
        .map_or(Scope::Plain, |span| span.scope)
}

/// The cells `[left, left + width)` of `line`, with no highlighting.
#[must_use]
pub fn cell_window(line: &str, left: usize, width: usize) -> String {
    window_parts(line, left, width, RowDecor::default())
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

    /// A tab keeps its width when it is shown: the arrow takes its first cell
    /// and the spaces it bought take the rest, so nothing after it shifts.
    #[test]
    fn special_characters_are_drawn_without_moving_the_columns() {
        let decor = RowDecor {
            special_chars: true,
            ..RowDecor::default()
        };
        let shown: String = window_parts("a\tb", 0, 5, decor)
            .into_iter()
            .map(|part| part.text)
            .collect();
        assert_eq!(shown, "a→  b");
        assert_eq!(
            shown.chars().count(),
            cell_window("a\tb", 0, 5).chars().count()
        );

        let nbsp: String = window_parts("a\u{a0}b", 0, 3, decor)
            .into_iter()
            .map(|part| part.text)
            .collect();
        assert_eq!(nbsp, "a·b");
        assert_eq!(
            cell_window("a\u{a0}b", 0, 3),
            "a\u{a0}b",
            "off, it is the space"
        );
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
        let parts = window_parts(
            "abcdef",
            0,
            6,
            RowDecor {
                selected: Some((2, 4)),
                ..RowDecor::default()
            },
        );
        assert_eq!(
            parts,
            vec![
                Part {
                    text: "ab".to_string(),
                    selected: false,
                    scope: Scope::Plain,
                    decoration: Decoration::None,
                },
                Part {
                    text: "cd".to_string(),
                    selected: true,
                    scope: Scope::Plain,
                    decoration: Decoration::None,
                },
                Part {
                    text: "ef".to_string(),
                    selected: false,
                    scope: Scope::Plain,
                    decoration: Decoration::None,
                },
            ]
        );
    }

    /// A colour change starts a run, and a selection edge inside one splits it
    /// again: the painter is handed strings it never has to cut.
    #[test]
    fn a_scope_splits_the_row_and_composes_with_the_selection() {
        let scopes = [Span {
            start: 0,
            end: 2,
            scope: Scope::Keyword,
        }];
        let plain = window_parts(
            "fn main",
            0,
            7,
            RowDecor {
                scopes: &scopes,
                ..RowDecor::default()
            },
        );
        assert_eq!(plain.len(), 2);
        assert_eq!(plain[0].text, "fn");
        assert_eq!(plain[0].scope, Scope::Keyword);
        assert_eq!(plain[1].scope, Scope::Plain);

        let split = window_parts(
            "fn main",
            0,
            7,
            RowDecor {
                selected: Some((1, 4)),
                scopes: &scopes,
                ..RowDecor::default()
            },
        );
        let runs: Vec<_> = split
            .iter()
            .map(|part| (part.text.as_str(), part.selected, part.scope))
            .collect();
        assert_eq!(
            runs,
            [
                ("f", false, Scope::Keyword),
                ("n", true, Scope::Keyword),
                (" m", true, Scope::Plain),
                ("ain", false, Scope::Plain),
            ]
        );
    }

    #[test]
    fn a_selection_that_swallows_the_line_break_shows_one_trailing_cell() {
        let parts = window_parts(
            "ab",
            0,
            8,
            RowDecor {
                selected: Some((0, 3)),
                ..RowDecor::default()
            },
        );
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text, "ab ");
        assert!(parts[0].selected);
    }
}
