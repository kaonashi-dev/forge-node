//! [`AlacrittyEngine`]: the default [`TerminalEngine`] built on
//! `alacritty_terminal` 0.26 (§11.4).
//!
//! We drive `alacritty_terminal` as a *library*: a [`Term`] plus the standalone
//! `vte::ansi::Processor`. We deliberately do **not** use its `EventLoop`/`tty`
//! — the PTY is ours (§11.2, [`crate::pty`]). Byte input goes through
//! [`Processor::advance`](alacritty_terminal::vte::ansi::Processor::advance),
//! row damage comes from [`Term::damage`]/[`Term::reset_damage`], and a small
//! [`EventProxy`] captures title/bell/PTY-reply events (§11.4).
//!
//! All emulator types are translated into the shared [`domain::terminal`] wire
//! types so nothing downstream depends on `alacritty_terminal`.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Row as AlacRow};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell as AlacCell, Flags};
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{self, Color as AnsiColor, CursorShape as AnsiCursorShape};
use compact_str::CompactString;
use domain::{
    Cell, CellFlags, Color, Cursor, CursorShape, Damage, MouseMode, PtySize, Row, TermModes,
    TerminalSnapshot,
};

use crate::engine::TerminalEngine;

/// Default scrollback lines per terminal (§11.4).
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
/// Hard cap on configurable scrollback (§11.4).
pub const MAX_SCROLLBACK_LINES: usize = 100_000;

/// Events captured from the emulator between polls. `send_event` takes `&self`,
/// so the state lives behind a mutex and is drained by the engine after each
/// mutating operation.
#[derive(Default)]
struct ProxyState {
    /// Latest window title (`None` after a reset).
    title: Option<String>,
    /// Whether a bell rang since the last drain.
    bell: bool,
    /// Bytes the emulator wants written back to the PTY (DA/DSR replies, etc.).
    pty_writes: Vec<u8>,
}

/// Cloneable handle to the shared [`ProxyState`], installed as the [`Term`]'s
/// event listener.
#[derive(Clone, Default)]
struct EventProxy(Arc<Mutex<ProxyState>>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let mut st = self.0.lock().expect("terminal event proxy poisoned");
        match event {
            Event::Title(title) => st.title = Some(title),
            Event::ResetTitle => st.title = None,
            Event::Bell => st.bell = true,
            Event::PtyWrite(text) => st.pty_writes.extend_from_slice(text.as_bytes()),
            _ => {}
        }
    }
}

/// Minimal [`Dimensions`] carrier for constructing/resizing a [`Term`].
struct TermDimensions {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// The default authoritative engine (§11.4, ADR-011).
pub struct AlacrittyEngine {
    term: Term<EventProxy>,
    processor: ansi::Processor,
    proxy: EventProxy,
    size: PtySize,
    seq: u64,
    title: Option<String>,
    bell: bool,
    pty_writes: Vec<u8>,
}

impl AlacrittyEngine {
    /// Construct an engine with the default scrollback (§11.4).
    #[must_use]
    pub fn new(size: PtySize) -> Self {
        Self::with_scrollback(size, DEFAULT_SCROLLBACK_LINES)
    }

    /// Construct an engine with an explicit scrollback limit (clamped to
    /// [`MAX_SCROLLBACK_LINES`]).
    #[must_use]
    pub fn with_scrollback(size: PtySize, scrollback_lines: usize) -> Self {
        let proxy = EventProxy::default();
        let config = Config {
            scrolling_history: scrollback_lines.min(MAX_SCROLLBACK_LINES),
            ..Config::default()
        };
        let dims = TermDimensions {
            columns: (size.cols.max(1)) as usize,
            screen_lines: (size.rows.max(1)) as usize,
        };
        let term = Term::new(config, &dims, proxy.clone());
        Self {
            term,
            processor: ansi::Processor::new(),
            proxy,
            size,
            seq: 0,
            title: None,
            bell: false,
            pty_writes: Vec::new(),
        }
    }

    /// Bytes the emulator wants echoed back to the PTY (primary/secondary
    /// device-attribute and cursor-position replies, etc.). Not part of
    /// [`TerminalEngine`]; the daemon's PTY loop drains and writes these so
    /// programs that query the terminal don't stall. Take-and-clear.
    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pty_writes)
    }

    /// Fold the proxy state accumulated during a feed/resize into engine fields
    /// so [`title`](TerminalEngine::title) can hand out a borrow and bell/PTY
    /// writes survive until taken.
    fn drain_proxy(&mut self) {
        let mut st = self.proxy.0.lock().expect("terminal event proxy poisoned");
        self.title = st.title.clone();
        if st.bell {
            self.bell = true;
            st.bell = false;
        }
        if !st.pty_writes.is_empty() {
            self.pty_writes.append(&mut st.pty_writes);
        }
    }

    fn build_cursor(&self) -> Cursor {
        let grid = self.term.grid();
        let mode = *self.term.mode();
        let mut point: Point = grid.cursor.point;
        // Report the lead cell if the cursor rests on a wide-char spacer.
        if point.column.0 > 0 && grid[point].flags.contains(Flags::WIDE_CHAR_SPACER) {
            point.column -= 1;
        }
        let visible = mode.contains(TermMode::SHOW_CURSOR);
        let shape = if visible {
            match self.term.cursor_style().shape {
                AnsiCursorShape::Block | AnsiCursorShape::HollowBlock => CursorShape::Block,
                AnsiCursorShape::Underline => CursorShape::Underline,
                AnsiCursorShape::Beam => CursorShape::Beam,
                AnsiCursorShape::Hidden => CursorShape::Hidden,
            }
        } else {
            CursorShape::Hidden
        };
        Cursor {
            line: point.line.0.max(0) as u16,
            col: point.column.0 as u16,
            shape,
            visible,
        }
    }

    fn build_modes(&self) -> TermModes {
        let m = *self.term.mode();
        let mouse_mode = if m.contains(TermMode::MOUSE_MOTION) {
            MouseMode::AnyEvent // 1003
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseMode::ButtonEvent // 1002
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Normal // 1000
        } else {
            MouseMode::Off
        };
        TermModes {
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            app_cursor_keys: m.contains(TermMode::APP_CURSOR),
            app_keypad: m.contains(TermMode::APP_KEYPAD),
            mouse_mode,
            mouse_sgr: m.contains(TermMode::SGR_MOUSE),
            focus_events: m.contains(TermMode::FOCUS_IN_OUT),
        }
    }
}

/// Map a `vte` color to the shared [`Color`]. Named 0–15 fold into `Indexed`;
/// the terminal default fg/bg (and every other special named slot) folds into
/// `Color::Default`; indexed and RGB pass through (§11.4).
fn convert_color(color: AnsiColor) -> Color {
    match color {
        AnsiColor::Named(named) => {
            let idx = named as usize;
            if idx < 16 {
                Color::Indexed(idx as u8)
            } else {
                Color::Default
            }
        }
        AnsiColor::Indexed(i) => Color::Indexed(i),
        AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

/// Map `alacritty` cell flags onto the shared [`CellFlags`] bitset (§11.4).
fn convert_flags(flags: Flags) -> CellFlags {
    let mut out = CellFlags::empty();
    if flags.contains(Flags::BOLD) {
        out.insert(CellFlags::BOLD);
    }
    if flags.contains(Flags::ITALIC) {
        out.insert(CellFlags::ITALIC);
    }
    if flags.intersects(Flags::ALL_UNDERLINES) {
        out.insert(CellFlags::UNDERLINE);
    }
    if flags.contains(Flags::INVERSE) {
        out.insert(CellFlags::INVERSE);
    }
    if flags.contains(Flags::DIM) {
        out.insert(CellFlags::DIM);
    }
    if flags.contains(Flags::STRIKEOUT) {
        out.insert(CellFlags::STRIKEOUT);
    }
    if flags.contains(Flags::HIDDEN) {
        out.insert(CellFlags::HIDDEN);
    }
    if flags.contains(Flags::WIDE_CHAR) {
        out.insert(CellFlags::WIDE_CHAR);
    }
    if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
        out.insert(CellFlags::WIDE_SPACER);
    }
    out
}

/// Convert one emulator cell. Wide-char continuation cells carry no text but
/// keep the `WIDE_SPACER` flag (§11.4).
fn convert_cell(cell: &AlacCell) -> Cell {
    let flags = convert_flags(cell.flags);
    let is_spacer = cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
    let text = if is_spacer {
        CompactString::default()
    } else {
        let mut s = CompactString::default();
        s.push(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            for &c in zerowidth {
                s.push(c);
            }
        }
        s
    };
    Cell {
        text,
        fg: convert_color(cell.fg),
        bg: convert_color(cell.bg),
        flags,
    }
}

/// Convert an emulator row. `wrapped` reflects the `WRAPLINE` flag on the last
/// cell (a soft wrap into the next row).
fn convert_row(row: &AlacRow<AlacCell>, cols: usize) -> Row {
    let mut cells = Vec::with_capacity(cols);
    for c in 0..cols {
        cells.push(convert_cell(&row[Column(c)]));
    }
    let wrapped = cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    Row { cells, wrapped }
}

impl TerminalEngine for AlacrittyEngine {
    fn feed(&mut self, bytes: &[u8]) -> bool {
        self.seq += 1;
        self.processor.advance(&mut self.term, bytes);
        // Match alacritty_terminal's own event loop: when every byte in this
        // chunk was absorbed by a synchronized-update block, the terminal grid
        // has deliberately not changed yet and no renderer should wake. The
        // closing ESU applies the buffered bytes, drops this count below the
        // chunk length and publishes one complete frame.
        let publish_damage = !bytes.is_empty() && self.processor.sync_bytes_count() < bytes.len();
        self.drain_proxy();
        publish_damage
    }

    fn resize(&mut self, size: PtySize) {
        self.size = size;
        let dims = TermDimensions {
            columns: (size.cols.max(1)) as usize,
            screen_lines: (size.rows.max(1)) as usize,
        };
        self.term.resize(dims);
        self.drain_proxy();
    }

    fn take_damage(&mut self) -> Damage {
        let mut rows = Vec::new();
        let full = match self.term.damage() {
            TermDamage::Full => true,
            TermDamage::Partial(iter) => {
                for bounds in iter {
                    rows.push(bounds.line as u16);
                }
                false
            }
        };
        self.term.reset_damage();
        if full {
            Damage::Full
        } else {
            rows.sort_unstable();
            rows.dedup();
            Damage::Partial(rows)
        }
    }

    fn snapshot(&self, scrollback_tail: usize) -> TerminalSnapshot {
        let grid = self.term.grid();
        let cols = grid.columns();
        let screen = grid.screen_lines();
        let hist = grid.history_size();

        let mut visible = Vec::with_capacity(screen);
        for i in 0..screen as i32 {
            visible.push(convert_row(&grid[Line(i)], cols));
        }

        let tail_n = scrollback_tail.min(hist);
        let mut scrollback = Vec::with_capacity(tail_n);
        // Oldest-to-newest: lines `-tail_n ..= -1` sit just above the viewport.
        for i in -(tail_n as i32)..0 {
            scrollback.push(convert_row(&grid[Line(i)], cols));
        }

        TerminalSnapshot {
            seq: self.seq,
            size: self.size,
            visible,
            scrollback_tail: scrollback,
            scrollback_len: hist as u64,
            cursor: self.build_cursor(),
            modes: self.build_modes(),
            title: self.title.clone(),
        }
    }

    fn rows(&self, range: Range<i64>) -> Vec<Row> {
        let grid = self.term.grid();
        let cols = grid.columns();
        let screen = grid.screen_lines() as i64;
        let hist = grid.history_size() as i64;
        let mut out = Vec::new();
        for i in range {
            if i < -hist || i >= screen {
                out.push(Row::blank(cols as u16));
            } else {
                out.push(convert_row(&grid[Line(i as i32)], cols));
            }
        }
        out
    }

    fn cursor(&self) -> Cursor {
        self.build_cursor()
    }

    fn modes(&self) -> TermModes {
        self.build_modes()
    }

    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn take_bell(&mut self) -> bool {
        let bell = self.bell;
        self.bell = false;
        bell
    }

    fn seq(&self) -> u64 {
        self.seq
    }

    fn scrollback_len(&self) -> u64 {
        self.term.grid().history_size() as u64
    }

    fn screen_lines(&self) -> u16 {
        self.term.grid().screen_lines() as u16
    }
}
