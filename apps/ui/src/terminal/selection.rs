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
