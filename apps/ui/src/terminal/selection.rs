use domain::CellFlags;

use crate::terminal::pane::Viewport;

pub const MAX_COPY_LINES: i64 = 5_000;
pub const MAX_COPY_BYTES: usize = 8 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Cell,
    Word,
    Line,
}

/// Endpoints are (column, absolute line), so history that scrolls while a selection exists stays selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (u16, i64),
    pub head: (u16, i64),
    pub granularity: Granularity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyError {
    Unavailable,
    TooLarge,
}

fn is_word(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || "_-./~:@%+=#?&\\".contains(c))
}

fn word_edge(viewport: &Viewport<'_>, (col, abs): (u16, i64), forward: bool) -> u16 {
    let Some(row) = viewport.row_at_abs(abs) else {
        return col;
    };
    let cells = &row.cells;
    let at = (col as usize).min(cells.len().saturating_sub(1));
    let class = is_word(&cells[at].text);
    let blank = cells[at].text.trim().is_empty();
    let same = |i: usize| {
        let t = &cells[i].text;
        cells[i].flags.contains(CellFlags::WIDE_SPACER)
            || (if blank {
                t.trim().is_empty()
            } else {
                is_word(t) == class && !t.trim().is_empty()
            })
    };
    let mut i = at;
    if forward {
        while i + 1 < cells.len() && same(i + 1) {
            i += 1;
        }
    } else {
        while i > 0 && same(i - 1) {
            i -= 1;
        }
    }
    i as u16
}

fn logical_start(viewport: &Viewport<'_>, mut abs: i64) -> i64 {
    while abs > 0 && viewport.row_at_abs(abs - 1).is_some_and(|row| row.wrapped) {
        abs -= 1;
    }
    abs
}

fn logical_end(viewport: &Viewport<'_>, mut abs: i64) -> i64 {
    while viewport.row_at_abs(abs).is_some_and(|row| row.wrapped)
        && viewport.row_at_abs(abs + 1).is_some()
    {
        abs += 1;
    }
    abs
}

impl Selection {
    pub fn new(at: (u16, i64), granularity: Granularity) -> Self {
        Selection {
            anchor: at,
            head: at,
            granularity,
        }
    }

    /// Inclusive, ordered endpoints after word/line expansion (a line spans its soft-wrapped continuations).
    pub fn range(&self, viewport: &Viewport<'_>) -> ((u16, i64), (u16, i64)) {
        let (a, b) = if (self.anchor.1, self.anchor.0) <= (self.head.1, self.head.0) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        let last = viewport.cols().saturating_sub(1) as u16;
        match self.granularity {
            Granularity::Cell => (a, b),
            Granularity::Word => (
                (word_edge(viewport, a, false), a.1),
                (word_edge(viewport, b, true), b.1),
            ),
            Granularity::Line => (
                (0, logical_start(viewport, a.1)),
                (last, logical_end(viewport, b.1)),
            ),
        }
    }
}

/// Columns of line `abs` inside the ordered inclusive range, if any.
pub fn span_on(range: ((u16, i64), (u16, i64)), abs: i64, cols: usize) -> Option<(usize, usize)> {
    let ((c0, l0), (c1, l1)) = range;
    if abs < l0 || abs > l1 {
        return None;
    }
    let from = if abs == l0 { c0 as usize } else { 0 };
    let to = if abs == l1 { c1 as usize + 1 } else { cols };
    (to > from).then_some((from, to.min(cols)))
}

/// All-or-nothing: a missing history row or an oversized range refuses the copy, never truncates it.
pub fn copy_text(viewport: &Viewport<'_>, sel: &Selection) -> Result<String, CopyError> {
    let range = sel.range(viewport);
    let ((_, l0), (_, l1)) = range;
    if l1 - l0 + 1 > MAX_COPY_LINES {
        return Err(CopyError::TooLarge);
    }
    let cols = viewport.cols();
    let mut out = String::new();
    for abs in l0..=l1 {
        let row = viewport.row_at_abs(abs).ok_or(CopyError::Unavailable)?;
        let (from, to) = span_on(range, abs, cols).unwrap_or((0, 0));
        let start = out.len();
        for cell in &row.cells[from.min(row.cells.len())..to.min(row.cells.len())] {
            if !cell.flags.contains(CellFlags::WIDE_SPACER) {
                out.push_str(&cell.text);
            }
        }
        if !row.wrapped {
            let keep = out[start..].trim_end().len();
            out.truncate(start + keep);
            if abs != l1 {
                out.push('\n');
            }
        }
        if out.len() > MAX_COPY_BYTES {
            return Err(CopyError::TooLarge);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::fixture::titled_grid;

    const CELL: (f32, f32) = (8.0, 18.0);

    #[test]
    fn a_range_reads_the_same_either_way_round() {
        let grid = titled_grid(10, 4, "");
        let viewport = Viewport {
            grid: &grid,
            scroll: 0,
        };
        let up = Selection {
            anchor: (1, 2),
            head: (3, 0),
            granularity: Granularity::Cell,
        };
        let down = Selection {
            anchor: (3, 0),
            head: (1, 2),
            granularity: Granularity::Cell,
        };
        assert_eq!(up.range(&viewport), ((3, 0), (1, 2)));
        assert_eq!(up.range(&viewport), down.range(&viewport));
    }

    #[test]
    fn spans_run_through_lines_and_clamp_as_columns_on_does() {
        let range = ((2, 0), (4, 2));
        assert_eq!(
            span_on(range, 0, 10),
            Some((2, 10)),
            "to the end of the line"
        );
        assert_eq!(span_on(range, 1, 10), Some((0, 10)));
        assert_eq!(span_on(range, 2, 10), Some((0, 5)));
        assert_eq!(span_on(range, -1, 10), None);
        assert_eq!(span_on(range, 3, 10), None);
        assert_eq!(span_on(range, 2, 3), Some((0, 3)), "clamped to the row");
        assert_eq!(
            span_on(((5, 1), (5, 1)), 1, 10),
            Some((5, 6)),
            "the one cell a drag never left"
        );
    }

    #[test]
    fn a_word_keeps_a_path_whole_and_stops_at_shell_space() {
        let grid = titled_grid(40, 4, "cd ~/dev/forge-node && ls");
        let viewport = Viewport {
            grid: &grid,
            scroll: 0,
        };
        let top = viewport.top_abs();
        let word = |col: u16| {
            let ((from, _), (to, _)) =
                Selection::new((col, top), Granularity::Word).range(&viewport);
            (from, to)
        };
        assert_eq!(word(8), (3, 18), "~/dev/forge-node");
        assert_eq!(word(0), (0, 1), "cd");
        assert_eq!(word(2), (2, 2), "a separator selects itself");
    }

    #[test]
    fn a_point_off_the_pane_clamps_to_the_line_ends() {
        let grid = titled_grid(80, 4, "");
        let viewport = Viewport {
            grid: &grid,
            scroll: 0,
        };
        assert_eq!(viewport.cell_at(9999.0, 0.0, CELL).0, 79);
        assert_eq!(viewport.cell_at(-40.0, 0.0, CELL).0, 0);
        assert_eq!(
            viewport.cell_at(0.0, 36.0, CELL).1,
            viewport.top_abs() + 2,
            "lines count from the grid, so scrolling keeps them"
        );
    }
}
