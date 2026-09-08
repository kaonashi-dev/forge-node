//! Compact wire encoding for the terminal viewport (Phase 2).
//!
//! The [`CellGrid`] stays on the runtime thread (ADR-011): what crosses the IPC
//! boundary is the *painted* form of the rows that changed — adjacent cells
//! sharing a style merged into one run — never a clone of the grid. A
//! full-screen frame is ~10 000 cells, and per-cell JSON at the 62 fps the
//! `cells_only` floor allows is the defect `docs/performance.md` forbids on the
//! delta rung; a merged row is one to a few dozen runs instead.
//!
//! Runs break at wide graphemes, whose advance cannot be assumed to be exactly
//! two columns in a fallback font, so the canvas is told the column span of
//! every run rather than inferring it from the text.
//!
//! The cursor box and the selection wash are deliberately *not* encoded here.
//! The canvas paints both from coordinates it already has, which is what keeps
//! a selection drag — dozens of events per crossed cell — off this thread
//! entirely. Copying still asks the thread for the text, because only the grid
//! knows what a run's cells actually hold.

use client::CellGrid;
use domain::{Cell, CellFlags, Color, CursorShape, Row, TermModes, TerminalId};
use serde::Serialize;

/// Rows fetched per scrollback request (§11.5); `SCROLLBACK_PAGE`.
pub const SCROLLBACK_PAGE: u32 = 64;

/// Wire sentinel for the terminal's default *foreground*.
const COLOR_FG: i64 = -1;
/// Wire sentinel for the terminal's default *background*.
///
/// Two sentinels rather than one because `INVERSE` swaps the slots and the
/// defaults with them: an inverted default cell paints term_bg on term_fg, and
/// a single "default" marker could not say which of the two a slot had landed
/// on.
const COLOR_BG: i64 = -2;
/// Bit that marks the remaining 24 as a literal RGB triple, keeping indexed
/// colors as their bare 0–255 index.
const COLOR_RGB: i64 = 1 << 24;

/// One run of adjacent cells that share a style, as a positional tuple to keep
/// the frame small: `[text, columns, fg, bg, flags]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WireRun(pub String, pub u16, pub i64, pub i64, pub u16);

/// A viewport row, merged into runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WireRow {
    /// Soft line-wrap into the next row.
    #[serde(rename = "w")]
    pub wrapped: bool,
    #[serde(rename = "r")]
    pub runs: Vec<WireRun>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct WireCursor {
    pub line: u16,
    pub col: u16,
    pub shape: &'static str,
    pub visible: bool,
}

impl From<domain::Cursor> for WireCursor {
    fn from(cursor: domain::Cursor) -> Self {
        Self {
            line: cursor.line,
            col: cursor.col,
            shape: match cursor.shape {
                CursorShape::Block => "block",
                CursorShape::Underline => "underline",
                CursorShape::Beam => "beam",
                CursorShape::Hidden => "hidden",
                // `CursorShape` is `#[non_exhaustive]`: an unknown shape is a
                // block rather than an invisible cursor.
                _ => "block",
            },
            visible: cursor.visible,
        }
    }
}

/// One frame of terminal output for the canvas.
///
/// `patch` carries only the rows that changed unless `full` is set, in which
/// case it is the whole viewport and the canvas replaces what it holds.
#[derive(Clone, Debug, Serialize)]
pub struct CellsPayload {
    pub terminal: TerminalId,
    pub seq: u64,
    pub cols: u16,
    pub rows: u16,
    pub full: bool,
    /// `(viewport row index, row)`. A `null` row is history the cache has not
    /// fetched yet: blank, not absent, so the text above it does not jump when
    /// the fetch lands.
    pub patch: Vec<(u16, Option<WireRow>)>,
    pub cursor: WireCursor,
    pub modes: TermModes,
    pub scroll_offset: u64,
    pub scrollback_len: u64,
    pub title: Option<String>,
    pub bell: bool,
    /// The newest keystroke id this frame is the echo of, or `0` for none.
    ///
    /// An id rather than a duration on purpose: the WebView holds the send
    /// timestamps, so it measures the *whole* round trip — its own IPC hop out,
    /// the daemon, this hop back, and the paint — instead of the slice this
    /// thread can see. An id rather than a count because a key that encodes to
    /// nothing is never written, and a count would leave the two sides
    /// permanently one apart with no way to notice.
    pub echo_id: u64,
}

/// What of the viewport a frame has to repaint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    /// Specific visible row indices.
    Rows(Vec<u16>),
    /// The whole viewport: attach, resync, resize, or any scrolled viewport,
    /// whose rows all shift when history moves.
    Full,
}

impl Damage {
    /// Merge two damages, widening to [`Damage::Full`] when either is.
    #[must_use]
    pub fn merge(self, other: Damage) -> Damage {
        match (self, other) {
            (Damage::Full, _) | (_, Damage::Full) => Damage::Full,
            (Damage::Rows(mut a), Damage::Rows(b)) => {
                a.extend(b);
                a.sort_unstable();
                a.dedup();
                Damage::Rows(a)
            }
        }
    }
}

/// The row a viewport at `scroll_offset` paints at `line`, or `None` when it is
/// history the cache does not hold yet.
///
/// Offsets follow [`CellGrid`]'s convention: negative is history, `0` and up
/// index `visible`.
#[must_use]
pub fn viewport_row(grid: &CellGrid, scroll_offset: u64, line: usize) -> Option<&Row> {
    let position = line as i64 - scroll_offset as i64;
    if position < 0 {
        grid.scrollback_row(position)
    } else {
        grid.visible.get(position as usize)
    }
}

/// Whether every scrollback row the viewport needs is already cached.
#[must_use]
pub fn viewport_is_cached(grid: &CellGrid, scroll_offset: u64) -> bool {
    (0..grid.visible.len()).all(|line| viewport_row(grid, scroll_offset, line).is_some())
}

/// The absolute scrollback index of the topmost row a viewport at
/// `scroll_offset` needs, or `None` when the viewport is entirely live output.
#[must_use]
pub fn oldest_needed_line(grid: &CellGrid, scroll_offset: u64) -> Option<i64> {
    if scroll_offset == 0 {
        return None;
    }
    let top = -(scroll_offset as i64);
    Some(top + grid.scrollback_len as i64)
}

/// Encode one row into style runs.
#[must_use]
pub fn encode_row(row: &Row) -> WireRow {
    let mut out = RunBuilder::default();
    for cell in &row.cells {
        // The leading cell of a wide grapheme already reserved both columns.
        if cell.flags.contains(CellFlags::WIDE_SPACER) {
            continue;
        }
        out.push(cell);
    }
    WireRow {
        wrapped: row.wrapped,
        runs: out.finish(),
    }
}

/// Build the frame for a damaged viewport.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn frame(
    terminal: TerminalId,
    grid: &CellGrid,
    scroll_offset: u64,
    damage: &Damage,
    bell: bool,
    echo_id: u64,
) -> CellsPayload {
    let height = grid.visible.len();
    let full = matches!(damage, Damage::Full) || scroll_offset > 0;
    let patch = if full {
        (0..height)
            .map(|line| {
                (
                    line as u16,
                    viewport_row(grid, scroll_offset, line).map(encode_row),
                )
            })
            .collect()
    } else {
        match damage {
            Damage::Rows(rows) => rows
                .iter()
                .filter(|index| usize::from(**index) < height)
                .map(|index| {
                    (
                        *index,
                        viewport_row(grid, scroll_offset, usize::from(*index)).map(encode_row),
                    )
                })
                .collect(),
            Damage::Full => Vec::new(),
        }
    };

    CellsPayload {
        terminal,
        seq: grid.last_seq,
        cols: grid.size.cols,
        rows: height as u16,
        full,
        patch,
        cursor: grid.cursor.into(),
        modes: grid.modes,
        scroll_offset,
        scrollback_len: grid.scrollback_len,
        title: grid.title.clone(),
        bell,
        echo_id,
    }
}

/// The text of a selection, ready for the clipboard.
///
/// Trailing blanks are trimmed per line, the way every terminal copies: the
/// cells past the end of a line hold spaces the shell never wrote, and pasting
/// them back would turn a copied command into a command plus padding.
#[must_use]
pub fn selection_text(
    grid: &CellGrid,
    scroll_offset: u64,
    anchor: (i64, usize),
    head: (i64, usize),
) -> String {
    let (start, end) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    let mut lines: Vec<String> = Vec::new();
    for index in 0..grid.visible.len() {
        let line = index as i64 - scroll_offset as i64;
        let Some(row) = viewport_row(grid, scroll_offset, index) else {
            continue;
        };
        if line < start.0 || line > end.0 {
            continue;
        }
        let last = row.cells.len().saturating_sub(1);
        let from = if line == start.0 { start.1 } else { 0 };
        let to = if line == end.0 { end.1.min(last) } else { last };
        if from > to || from >= row.cells.len() {
            continue;
        }
        let mut text = String::new();
        for cell in &row.cells[from..=to] {
            if !cell.flags.contains(CellFlags::WIDE_SPACER) {
                text.push_str(&cell.text);
            }
        }
        lines.push(text.trim_end().to_string());
    }
    lines.join("\n")
}

/// Accumulates cells into runs, breaking on a style change and on either side
/// of a wide grapheme.
#[derive(Default)]
struct RunBuilder {
    runs: Vec<WireRun>,
    text: String,
    style: Option<(i64, i64, u16)>,
    cols: u16,
}

impl RunBuilder {
    fn push(&mut self, cell: &Cell) {
        let style = cell_style(cell);
        let wide = cell.flags.contains(CellFlags::WIDE_CHAR);
        if wide || self.style != Some(style) {
            self.flush();
        }
        // Hidden text still occupies its columns; it is blanked, not dropped.
        if cell.flags.contains(CellFlags::HIDDEN) || cell.text.is_empty() {
            self.text.push(' ');
        } else {
            self.text.push_str(&cell.text);
        }
        self.style = Some(style);
        self.cols += if wide { 2 } else { 1 };
        if wide {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if let Some((fg, bg, flags)) = self.style.take() {
            if self.cols > 0 {
                self.runs.push(WireRun(
                    std::mem::take(&mut self.text),
                    self.cols,
                    fg,
                    bg,
                    flags,
                ));
            }
            self.text.clear();
            self.cols = 0;
        }
    }

    /// Finish the row.
    ///
    /// A row of untouched default cells paints as the terminal backdrop the
    /// canvas has already cleared to, so it travels as *no runs at all* — an
    /// empty run list means "this row is blank", not "this row is unknown",
    /// which is what a `null` row in the patch means. A freshly cleared screen
    /// is the common case and costs two bytes a row this way.
    fn finish(mut self) -> Vec<WireRun> {
        self.flush();
        if self.runs.len() == 1 {
            let WireRun(text, _, fg, bg, flags) = &self.runs[0];
            if *fg == COLOR_FG && *bg == COLOR_BG && *flags == 0 && text.chars().all(|c| c == ' ') {
                return Vec::new();
            }
        }
        self.runs
    }
}

/// The wire style of one cell, with `INVERSE` already resolved by swapping the
/// slots — including the two *default* slots, which is what makes an inverted
/// default cell paint the terminal's own colors the other way round.
fn cell_style(cell: &Cell) -> (i64, i64, u16) {
    let inverse = cell.flags.contains(CellFlags::INVERSE);
    let (fg, bg) = if inverse {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };
    let (fg_default, bg_default) = if inverse {
        (COLOR_BG, COLOR_FG)
    } else {
        (COLOR_FG, COLOR_BG)
    };
    (
        wire_color(fg, fg_default),
        wire_color(bg, bg_default),
        cell.flags.0,
    )
}

fn wire_color(color: Color, default: i64) -> i64 {
    match color {
        Color::Default => default,
        Color::Indexed(index) => i64::from(index),
        Color::Rgb(red, green, blue) => {
            COLOR_RGB | (i64::from(red) << 16) | (i64::from(green) << 8) | i64::from(blue)
        }
        // `Color` is `#[non_exhaustive]`: an unknown color is the default one.
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_str::CompactString;
    use domain::{Cursor, PtySize, TerminalSnapshot};

    fn cell(text: &str, fg: Color, bg: Color, flags: CellFlags) -> Cell {
        Cell {
            text: CompactString::new(text),
            fg,
            bg,
            flags,
        }
    }

    fn row(cells: Vec<Cell>) -> Row {
        Row {
            cells,
            wrapped: false,
        }
    }

    fn plain(text: &str) -> Row {
        row(text
            .chars()
            .map(|c| {
                cell(
                    &c.to_string(),
                    Color::Default,
                    Color::Default,
                    CellFlags::empty(),
                )
            })
            .collect())
    }

    fn grid(visible: Vec<Row>, scrollback: Vec<Row>) -> CellGrid {
        CellGrid::from_snapshot(&TerminalSnapshot {
            seq: 1,
            size: PtySize {
                cols: visible.first().map_or(0, |r| r.cells.len()) as u16,
                rows: visible.len() as u16,
                pixel_width: 0,
                pixel_height: 0,
            },
            scrollback_len: scrollback.len() as u64,
            visible,
            scrollback_tail: scrollback,
            cursor: Cursor::default(),
            modes: TermModes::default(),
            title: None,
        })
    }

    #[test]
    fn adjacent_cells_sharing_a_style_become_one_run() {
        let encoded = encode_row(&plain("hello"));
        assert_eq!(encoded.runs.len(), 1);
        assert_eq!(encoded.runs[0].0, "hello");
        assert_eq!(encoded.runs[0].1, 5);
    }

    #[test]
    fn a_style_change_breaks_the_run() {
        let mut cells = plain("ab").cells;
        cells.push(cell(
            "c",
            Color::Indexed(2),
            Color::Default,
            CellFlags::empty(),
        ));
        let encoded = encode_row(&row(cells));
        assert_eq!(encoded.runs.len(), 2);
        assert_eq!(encoded.runs[0].0, "ab");
        assert_eq!(encoded.runs[1].0, "c");
        assert_eq!(encoded.runs[1].2, 2);
    }

    /// A fallback font cannot be assumed to advance a CJK glyph by exactly two
    /// columns, so the canvas is told the span instead of inferring it.
    #[test]
    fn a_wide_grapheme_is_its_own_run_spanning_two_columns() {
        let encoded = encode_row(&row(vec![
            cell("a", Color::Default, Color::Default, CellFlags::empty()),
            cell("漢", Color::Default, Color::Default, CellFlags::WIDE_CHAR),
            cell("", Color::Default, Color::Default, CellFlags::WIDE_SPACER),
            cell("b", Color::Default, Color::Default, CellFlags::empty()),
        ]));
        assert_eq!(encoded.runs.len(), 3);
        assert_eq!((encoded.runs[0].0.as_str(), encoded.runs[0].1), ("a", 1));
        assert_eq!((encoded.runs[1].0.as_str(), encoded.runs[1].1), ("漢", 2));
        assert_eq!((encoded.runs[2].0.as_str(), encoded.runs[2].1), ("b", 1));
        assert_eq!(
            encoded.runs.iter().map(|run| run.1).sum::<u16>(),
            4,
            "the runs must still cover every column of the row"
        );
    }

    #[test]
    fn a_blank_default_row_travels_as_no_runs() {
        assert!(encode_row(&Row::blank(80)).runs.is_empty());
    }

    /// The blank-row shortcut must not swallow a row that only *looks* blank:
    /// a selected or highlighted space still has to paint its background.
    #[test]
    fn spaces_with_a_background_still_travel() {
        let encoded = encode_row(&row(vec![cell(
            " ",
            Color::Default,
            Color::Indexed(4),
            CellFlags::empty(),
        )]));
        assert_eq!(encoded.runs.len(), 1);
        assert_eq!(encoded.runs[0].3, 4);
    }

    #[test]
    fn hidden_text_keeps_its_columns_and_loses_its_glyphs() {
        let encoded = encode_row(&row(vec![cell(
            "s",
            Color::Indexed(1),
            Color::Default,
            CellFlags::HIDDEN,
        )]));
        assert_eq!(encoded.runs[0].0, " ");
        assert_eq!(encoded.runs[0].1, 1);
    }

    #[test]
    fn inverse_swaps_the_slots_and_their_defaults() {
        let encoded = encode_row(&row(vec![cell(
            "x",
            Color::Default,
            Color::Default,
            CellFlags::INVERSE,
        )]));
        assert_eq!(encoded.runs[0].2, COLOR_BG, "fg slot takes the background");
        assert_eq!(encoded.runs[0].3, COLOR_FG, "bg slot takes the foreground");
    }

    #[test]
    fn rgb_is_tagged_and_indexed_stays_bare() {
        let encoded = encode_row(&row(vec![
            cell("a", Color::Rgb(1, 2, 3), Color::Default, CellFlags::empty()),
            cell("b", Color::Indexed(200), Color::Default, CellFlags::empty()),
        ]));
        assert_eq!(encoded.runs[0].2, COLOR_RGB | 0x01_0203);
        assert_eq!(encoded.runs[1].2, 200);
    }

    #[test]
    fn a_scrolled_viewport_reads_history_above_the_live_rows() {
        let grid = grid(vec![plain("live0"), plain("live1")], vec![plain("hist0")]);
        assert_eq!(
            viewport_row(&grid, 1, 0).map(|row| row.cells[0].text.as_str()),
            Some("h")
        );
        assert_eq!(
            viewport_row(&grid, 1, 1).map(|row| row.cells[0].text.as_str()),
            Some("l")
        );
    }

    /// History the cache does not hold is a *blank* row, not a missing one:
    /// collapsing it would make the text above jump as the fetch lands.
    #[test]
    fn history_the_cache_lacks_is_a_null_row_not_an_absent_one() {
        let grid = grid(vec![plain("a"), plain("b")], Vec::new());
        let payload = frame(TerminalId::new(), &grid, 2, &Damage::Full, false, 0);
        assert_eq!(payload.patch.len(), 2);
        assert!(payload.patch.iter().all(|(_, row)| row.is_none()));
        assert!(payload.full);
    }

    #[test]
    fn a_scrolled_viewport_always_repaints_whole() {
        let grid = grid(vec![plain("a"), plain("b")], vec![plain("h")]);
        let payload = frame(
            TerminalId::new(),
            &grid,
            1,
            &Damage::Rows(vec![0]),
            false,
            0,
        );
        assert!(payload.full, "rows shift under a scrolled viewport");
        assert_eq!(payload.patch.len(), 2);
    }

    #[test]
    fn damage_only_carries_the_rows_that_changed() {
        let grid = grid(vec![plain("a"), plain("b"), plain("c")], Vec::new());
        let payload = frame(
            TerminalId::new(),
            &grid,
            0,
            &Damage::Rows(vec![2]),
            false,
            0,
        );
        assert!(!payload.full);
        assert_eq!(payload.patch.len(), 1);
        assert_eq!(payload.patch[0].0, 2);
    }

    #[test]
    fn damage_out_of_range_is_dropped_rather_than_panicking() {
        let grid = grid(vec![plain("a")], Vec::new());
        let payload = frame(
            TerminalId::new(),
            &grid,
            0,
            &Damage::Rows(vec![9]),
            false,
            0,
        );
        assert!(payload.patch.is_empty());
    }

    #[test]
    fn merging_damage_widens_to_full() {
        assert_eq!(
            Damage::Rows(vec![1]).merge(Damage::Full),
            Damage::Full,
            "a resync in the same batch invalidates the row list"
        );
        assert_eq!(
            Damage::Rows(vec![2, 1]).merge(Damage::Rows(vec![1, 3])),
            Damage::Rows(vec![1, 2, 3])
        );
    }

    #[test]
    fn a_selection_copies_without_the_padding_after_the_line() {
        let grid = grid(vec![plain("ls -la    "), plain("done      ")], Vec::new());
        let text = selection_text(&grid, 0, (0, 0), (1, 9));
        assert_eq!(text, "ls -la\ndone");
    }

    #[test]
    fn a_selection_reads_the_same_either_way_round() {
        let grid = grid(vec![plain("ab"), plain("cd")], Vec::new());
        assert_eq!(
            selection_text(&grid, 0, (1, 1), (0, 0)),
            selection_text(&grid, 0, (0, 0), (1, 1))
        );
    }

    #[test]
    fn a_wide_spacer_is_not_copied_twice() {
        let grid = grid(
            vec![row(vec![
                cell("漢", Color::Default, Color::Default, CellFlags::WIDE_CHAR),
                cell("", Color::Default, Color::Default, CellFlags::WIDE_SPACER),
            ])],
            Vec::new(),
        );
        assert_eq!(selection_text(&grid, 0, (0, 0), (0, 1)), "漢");
    }

    #[test]
    fn the_oldest_needed_line_is_absolute_and_live_output_needs_none() {
        let grid = grid(vec![plain("a"), plain("b")], vec![plain("h")]);
        assert_eq!(oldest_needed_line(&grid, 0), None);
        assert_eq!(oldest_needed_line(&grid, 1), Some(0));
    }

    #[test]
    fn a_viewport_is_cached_only_when_every_row_it_paints_is_there() {
        let grid = grid(vec![plain("a"), plain("b")], vec![plain("h")]);
        assert!(viewport_is_cached(&grid, 0));
        assert!(viewport_is_cached(&grid, 1));
        assert!(!viewport_is_cached(&grid, 2));
    }
}
