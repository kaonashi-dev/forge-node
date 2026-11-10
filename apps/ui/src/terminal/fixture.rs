use std::sync::Arc;

use client::CellGrid;
use domain::agent::PtySize;
use domain::{
    Cell, CellFlags, Color, Cursor, CursorShape, DEFAULT_SCROLLBACK_TAIL, MouseMode, Row,
    ScrollbackRows, TermModes, TerminalDelta, TerminalSnapshot,
};

pub const HISTORY_LINES: u64 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "fixture modes are exercised by tests")
)]
pub enum ModeSet {
    Plain,
    Mouse,
    App,
}

impl ModeSet {
    pub fn modes(self) -> TermModes {
        let base = TermModes {
            bracketed_paste: true,
            ..TermModes::default()
        };
        match self {
            Self::Plain => base,
            Self::Mouse => TermModes {
                mouse_mode: MouseMode::ButtonEvent,
                mouse_sgr: true,
                focus_events: true,
                ..base
            },
            Self::App => TermModes {
                app_cursor_keys: true,
                bracketed_paste: false,
                ..base
            },
        }
    }
}

pub fn is_wide(ch: char) -> bool {
    matches!(ch as u32, 0x1100..=0x115f | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xff00..=0xff60 | 0x1f300..=0x1faff)
}

/// A grapheme is two columns when its first scalar is wide or it asks for emoji presentation.
pub fn is_wide_cluster(text: &str) -> bool {
    text.chars().next().is_some_and(is_wide) || text.contains('\u{fe0f}')
}

pub struct RowBuilder {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

impl RowBuilder {
    pub fn new(cols: usize) -> Self {
        RowBuilder {
            cells: vec![Cell::default(); cols],
            wrapped: false,
        }
    }

    pub fn put(
        &mut self,
        mut col: usize,
        text: &str,
        fg: Color,
        bg: Color,
        flags: CellFlags,
    ) -> usize {
        let mut buf = [0u8; 4];
        for ch in text.chars() {
            if col >= self.cells.len() {
                break;
            }
            let s: &str = ch.encode_utf8(&mut buf);
            if is_wide(ch) && col + 1 < self.cells.len() {
                self.cells[col] = Cell {
                    text: s.into(),
                    fg,
                    bg,
                    flags: flags | CellFlags::WIDE_CHAR,
                };
                self.cells[col + 1] = Cell {
                    text: "".into(),
                    fg,
                    bg,
                    flags: flags | CellFlags::WIDE_SPACER,
                };
                col += 2;
            } else {
                self.cells[col] = Cell {
                    text: s.into(),
                    fg,
                    bg,
                    flags,
                };
                col += 1;
            }
        }
        col
    }

    /// Each string is one grapheme cluster in one cell (two columns when wide), as the daemon's engine reports it.
    pub fn put_clusters(
        &mut self,
        mut col: usize,
        clusters: &[&str],
        fg: Color,
        bg: Color,
    ) -> usize {
        for text in clusters {
            let wide = is_wide_cluster(text);
            if col + usize::from(wide) >= self.cells.len() {
                break;
            }
            let flags = if wide {
                CellFlags::WIDE_CHAR
            } else {
                CellFlags::empty()
            };
            self.cells[col] = Cell {
                text: (*text).into(),
                fg,
                bg,
                flags,
            };
            if wide {
                self.cells[col + 1] = Cell {
                    text: "".into(),
                    fg,
                    bg,
                    flags: CellFlags::WIDE_SPACER,
                };
            }
            col += 1 + usize::from(wide);
        }
        col
    }

    pub fn plain(&mut self, col: usize, text: &str) -> usize {
        self.put(
            col,
            text,
            Color::Default,
            Color::Default,
            CellFlags::empty(),
        )
    }

    pub fn build(self) -> Row {
        Row {
            cells: Arc::new(self.cells),
            wrapped: self.wrapped,
        }
    }
}

/// A full row of mixed graphemes (CJK, combining, emoji, ASCII), every cell going through the shaped-run cache.
pub fn rich_row(cols: usize, n: u64) -> Row {
    const CYCLE: [&str; 9] = [
        "漢", "字", "e\u{301}", "😀", "か", "a", "b", " ", "x\u{301}",
    ];
    let mut b = RowBuilder::new(cols);
    let mut col = 0;
    let mut k = n as usize;
    while col + 2 < cols {
        let next = b.put_clusters(
            col,
            &[CYCLE[k % CYCLE.len()]],
            Color::Indexed(1 + (k % 6) as u8),
            Color::Default,
        );
        col = next.max(col + 1);
        k += 1;
    }
    b.build()
}

pub fn log_row(cols: usize, n: u64) -> Row {
    let mut b = RowBuilder::new(cols);
    let line = format!(
        "{n:06} INFO  forge::daemon::registry  session={:04x} delta seq={} rows={} bytes={}  [{}]",
        n * 7919 % 0xffff,
        n * 3,
        n % 50,
        n * 131 % 9000,
        "#".repeat((n % 40) as usize)
    );
    let level = Color::Indexed(1 + (n % 6) as u8);
    b.put(
        0,
        &line[..7.min(line.len())],
        Color::Indexed(8),
        Color::Default,
        CellFlags::empty(),
    );
    b.put(7, "INFO ", level, Color::Default, CellFlags::empty());
    let rest = line.get(12..).unwrap_or("");
    b.plain(12, rest);
    b.build()
}

fn history_row(cols: usize, abs: i64) -> Row {
    let mut b = RowBuilder::new(cols);
    b.put(
        0,
        &format!("history line {abs:05}"),
        Color::Indexed(8),
        Color::Default,
        CellFlags::empty(),
    );
    b.plain(24, "older output scrolled out of the live screen");
    b.build()
}

pub fn scrollback_rows(cols: usize, from_line: i64, count: u32, generation: u64) -> ScrollbackRows {
    let from = from_line.clamp(0, HISTORY_LINES as i64);
    let to = (from + count as i64).min(HISTORY_LINES as i64);
    ScrollbackRows {
        generation,
        snapshot: None,
        from_line: from,
        rows: (from..to).map(|abs| history_row(cols, abs)).collect(),
    }
}

pub const GRAPHEMES: [&str; 12] = [
    "e\u{301}",
    " ",
    "x\u{301}",
    " ",
    "a\u{308}",
    " ",
    "漢",
    "😀",
    "👨\u{200d}👩\u{200d}👧",
    "❤\u{fe0f}",
    "🇯🇵",
    "👍\u{1f3fd}",
];

fn showcase(cols: usize, rows: usize) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::with_capacity(rows);
    let mut r = RowBuilder::new(cols);
    r.put(
        0,
        "client::CellGrid fixture: every domain::Cell attribute",
        Color::Default,
        Color::Default,
        CellFlags::BOLD,
    );
    out.push(r.build());

    let mut r = RowBuilder::new(cols);
    let mut c = 0;
    for (name, flag) in [
        ("bold", CellFlags::BOLD),
        ("dim", CellFlags::DIM),
        ("italic", CellFlags::ITALIC),
        ("underline", CellFlags::UNDERLINE),
        ("strike", CellFlags::STRIKEOUT),
        ("inverse", CellFlags::INVERSE),
        ("hidden", CellFlags::HIDDEN),
        ("bold+italic", CellFlags::BOLD | CellFlags::ITALIC),
    ] {
        c = r.put(c, name, Color::Default, Color::Default, flag) + 2;
    }
    out.push(r.build());

    let mut r = RowBuilder::new(cols);
    let mut c = 0;
    for i in 0..16u8 {
        c = r.put(
            c,
            "ab",
            Color::Indexed(i),
            Color::Default,
            CellFlags::empty(),
        );
    }
    c += 2;
    for i in 0..16u8 {
        c = r.put(
            c,
            "  ",
            Color::Default,
            Color::Indexed(i),
            CellFlags::empty(),
        );
    }
    let _ = c;
    out.push(r.build());

    for band in 0..6u8 {
        let mut r = RowBuilder::new(cols);
        for k in 0..36u8 {
            r.put(
                k as usize,
                "▀",
                Color::Indexed(16 + band * 36 + k),
                Color::Indexed(16 + band * 36 + k),
                CellFlags::empty(),
            );
        }
        out.push(r.build());
    }
    let mut r = RowBuilder::new(cols);
    for k in 0..24u8 {
        r.put(
            k as usize,
            " ",
            Color::Default,
            Color::Indexed(232 + k),
            CellFlags::empty(),
        );
    }
    out.push(r.build());

    let mut r = RowBuilder::new(cols);
    let span = cols.min(72);
    for k in 0..span {
        let t = k as f32 / span as f32;
        r.put(
            k,
            " ",
            Color::Default,
            Color::Rgb((255.0 * t) as u8, (180.0 * (1.0 - t)) as u8, 160),
            CellFlags::empty(),
        );
    }
    out.push(r.build());

    let mut r = RowBuilder::new(cols);
    r.put(
        0,
        "truecolor fg: ",
        Color::Default,
        Color::Default,
        CellFlags::empty(),
    );
    r.put(
        14,
        "RGB(255,140,0) text on RGB(20,40,80)",
        Color::Rgb(255, 140, 0),
        Color::Rgb(20, 40, 80),
        CellFlags::UNDERLINE,
    );
    out.push(r.build());

    let mut r = RowBuilder::new(cols);
    r.put(
        0,
        "wide: ",
        Color::Default,
        Color::Default,
        CellFlags::empty(),
    );
    r.put(
        6,
        "あいう 漢字 한글 😀 ok",
        Color::Indexed(11),
        Color::Default,
        CellFlags::empty(),
    );
    r.put_clusters(36, &GRAPHEMES, Color::Indexed(14), Color::Default);
    out.push(r.build());

    let mut r = RowBuilder::new(cols);
    r.plain(
        0,
        "word select: /Users/me/dev/forge-node/src/main.rs:42 https://example.com/a?b=c done",
    );
    out.push(r.build());

    let sentence = "a soft wrapped logical line that continues onto the next screen row and is joined without a newline when copied, ending here.";
    let mut long = sentence.to_string();
    while long.chars().count() <= cols + 10 {
        long = format!("{long} {sentence}");
    }
    let width = cols;
    let mut chars = long.chars().peekable();
    while chars.peek().is_some() {
        let mut r = RowBuilder::new(cols);
        let chunk: String = chars.by_ref().take(width).collect();
        r.put(
            0,
            &chunk,
            Color::Indexed(14),
            Color::Default,
            CellFlags::empty(),
        );
        r.wrapped = chars.peek().is_some();
        out.push(r.build());
    }
    let mut n = 0;
    while out.len() < rows - 1 {
        n += 1;
        out.push(log_row(cols, n));
    }
    let mut r = RowBuilder::new(cols);
    r.put(
        0,
        "~/forge-node",
        Color::Indexed(4),
        Color::Default,
        CellFlags::BOLD,
    );
    r.put(
        13,
        "$ ",
        Color::Indexed(2),
        Color::Default,
        CellFlags::empty(),
    );
    out.push(r.build());
    out.truncate(rows);
    out
}

pub fn snapshot(cols: u16, rows: u16, mode: ModeSet, shell: bool) -> TerminalSnapshot {
    let (c, r) = (cols as usize, rows as usize);
    let mut visible = if shell {
        showcase(c, r)
    } else {
        let mut v: Vec<Row> = (1..r as u64).map(|n| log_row(c, 500 + n)).collect();
        let mut b = RowBuilder::new(c);
        b.put(
            0,
            "agent> ",
            Color::Indexed(5),
            Color::Default,
            CellFlags::BOLD,
        );
        v.push(b.build());
        v
    };
    visible.truncate(r);
    let tail: Vec<Row> = (HISTORY_LINES as i64 - DEFAULT_SCROLLBACK_TAIL as i64
        ..HISTORY_LINES as i64)
        .map(|abs| history_row(c, abs))
        .collect();
    let line = rows - 1;
    let col = if shell { 15 } else { 7 };
    TerminalSnapshot {
        seq: 1,
        size: PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        },
        visible,
        scrollback_tail: tail,
        scrollback_len: HISTORY_LINES,
        scrollback_generation: 1,
        cursor: Cursor {
            line,
            col,
            shape: CursorShape::Block,
            visible: true,
        },
        modes: mode.modes(),
        title: Some(if shell {
            "shell".into()
        } else {
            "agent".into()
        }),
    }
}

pub fn grid(cols: u16, rows: u16, mode: ModeSet, shell: bool) -> CellGrid {
    CellGrid::from_snapshot(&snapshot(cols, rows, mode, shell))
}

pub fn titled_grid(cols: u16, rows: u16, title: &str) -> CellGrid {
    let mut grid = grid(cols, rows, ModeSet::Plain, true);
    if let Some(row) = grid.visible.first_mut() {
        let cells = std::sync::Arc::make_mut(&mut row.cells);
        let text: Vec<char> = title.chars().collect();
        for (i, cell) in cells.iter_mut().enumerate() {
            cell.text = text.get(i).copied().unwrap_or(' ').to_string().into();
            cell.fg = Color::Indexed(11);
            cell.bg = Color::Default;
            cell.flags = CellFlags::BOLD;
        }
    }
    grid
}

/// A one-row delta that echoes what the fixture "shell" received. The worker uses it
/// only to stand in for a PTY round trip; a real daemon owns that in live mode.
pub fn echo_delta(grid: &CellGrid, line: &str, seq: u64) -> TerminalDelta {
    let cols = grid.visible.first().map_or(0, |row| row.cells.len());
    let rows = grid.visible.len();
    let mut builder = RowBuilder::new(cols);
    builder.put(
        0,
        "fixture> ",
        Color::Indexed(2),
        Color::Default,
        CellFlags::BOLD,
    );
    builder.put(
        9,
        line,
        Color::Indexed(15),
        Color::Default,
        CellFlags::empty(),
    );
    TerminalDelta {
        seq,
        rows: vec![((rows.saturating_sub(1)) as u16, builder.build())],
        patches: Vec::new(),
        scrollback_len: grid.scrollback_len,
        scrollback_generation: grid.scrollback_generation,
        scrolled_lines: 0,
        cursor: domain::Cursor {
            line: rows.saturating_sub(1) as u16,
            col: (9 + line.chars().count()).min(cols.saturating_sub(1)) as u16,
            shape: CursorShape::Block,
            visible: true,
        },
        modes: grid.modes,
    }
}

/// Stands in for the daemon: each call makes the next `TerminalDelta` for a grid.
pub struct Feed {
    pub scroll: bool,
    pub rich: bool,
    line: u64,
    tick: u64,
}

impl Feed {
    pub fn new(scroll: bool, rich: bool) -> Self {
        Feed {
            scroll,
            rich,
            line: 1000,
            tick: 0,
        }
    }

    pub fn delta(&mut self, grid: &CellGrid) -> TerminalDelta {
        self.tick += 1;
        let (cols, rows) = (grid.visible[0].cells.len(), grid.visible.len());
        let mut changed: Vec<(u16, Row)> = Vec::new();
        let mut scrolled = 0;
        if self.scroll {
            for line in 0..rows {
                self.line += 1;
                changed.push((
                    line as u16,
                    if self.rich {
                        rich_row(cols, self.line)
                    } else {
                        log_row(cols, self.line)
                    },
                ));
            }
            scrolled = 1;
        } else {
            let t = self.tick as usize;
            let mut status = RowBuilder::new(cols);
            status.put(
                0,
                &format!("tick {:>8}  {}", self.tick, ["|", "/", "-", "\\"][t % 4]),
                Color::Indexed(2),
                Color::Default,
                CellFlags::empty(),
            );
            changed.push((rows as u16 - 2, status.build()));
            let mut bar = RowBuilder::new(cols);
            let filled = t % cols.saturating_sub(10).max(1);
            for c in 0..cols {
                let (ch, fg) = if c < filled {
                    ("█", Color::Indexed(4))
                } else {
                    ("░", Color::Indexed(8))
                };
                bar.put(c, ch, fg, Color::Default, CellFlags::empty());
            }
            changed.push((rows as u16 - 3, bar.build()));
            self.line += 1;
            changed.push((
                20 + (t % rows.saturating_sub(24).max(1)) as u16,
                log_row(cols, self.line),
            ));
        }
        TerminalDelta {
            seq: grid.last_seq + 1,
            rows: changed,
            patches: Vec::new(),
            scrollback_len: grid.scrollback_len + scrolled as u64,
            scrollback_generation: grid.scrollback_generation,
            scrolled_lines: scrolled,
            cursor: grid.cursor,
            modes: grid.modes,
        }
    }
}
