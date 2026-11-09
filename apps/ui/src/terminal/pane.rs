use std::collections::HashMap;
use std::sync::Arc;

use client::{CellGrid, RowDamage};
use domain::{Cursor, Row, SessionId, TerminalId};
use rune::KeyEvent;

use crate::terminal::SharedTerminal;
use crate::terminal::attach::{Attachment, HeldInput};
use crate::terminal::fixture;
use crate::terminal::selection::Selection;

pub const FETCH_BLOCK: i64 = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FetchRequest {
    pub from_line: i64,
    pub count: u32,
}

/// A selection drag's pointer. `moved` stays false for a click held still,
/// which never scrolls, as in the original client's `SelectionDrag`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragPoint {
    pub x: f32,
    pub y: f32,
    pub moved: bool,
    /// When the last edge step was taken, on the poll clock.
    pub stepped_at: f64,
}

/// A plain click that may move the shell's cursor, as the original client's `CursorClick`:
/// it lands only on the cell it started on, against the grid it saw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorClick {
    pub row: u16,
    pub col: u16,
    pub seq: u64,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Select,
    Report,
}

/// A viewport over the shared grid: rows above the live screen are negative offsets
/// into `CellGrid::scrollback_row`.
pub struct Viewport<'a> {
    pub grid: &'a CellGrid,
    pub scroll: usize,
}

impl<'a> Viewport<'a> {
    pub fn rows(&self) -> usize {
        self.grid.visible.len()
    }

    pub fn cols(&self) -> usize {
        self.grid.visible.first().map_or(0, |row| row.cells.len())
    }

    /// Absolute line (0 = oldest history) of the live screen's first row.
    pub fn base(&self) -> i64 {
        self.grid.scrollback_len as i64
    }

    pub fn top_abs(&self) -> i64 {
        self.base() - self.scroll as i64
    }

    pub fn row_at_abs(&self, abs: i64) -> Option<&Row> {
        let base = self.base();
        if abs < 0 {
            None
        } else if abs >= base {
            self.grid.visible.get((abs - base) as usize)
        } else {
            self.grid.scrollback_row(abs - base)
        }
    }

    pub fn viewport_row(&self, v: usize) -> Option<&Row> {
        self.row_at_abs(self.top_abs() + v as i64)
    }

    pub fn max_scroll(&self) -> usize {
        self.grid.scrollback_len as usize
    }

    pub fn cell_at(&self, x: f32, y: f32, cell: (f32, f32)) -> (u16, i64) {
        let col = (x / cell.0)
            .floor()
            .clamp(0.0, self.cols().saturating_sub(1) as f32) as u16;
        let v = (y / cell.1)
            .floor()
            .clamp(0.0, self.rows().saturating_sub(1) as f32) as usize;
        (col, self.top_abs() + v as i64)
    }

    pub fn viewport_cell(&self, x: f32, y: f32, cell: (f32, f32)) -> (u16, u16) {
        let col = (x / cell.0)
            .floor()
            .clamp(0.0, self.cols().saturating_sub(1) as f32) as u16;
        let row = (y / cell.1)
            .floor()
            .clamp(0.0, self.rows().saturating_sub(1) as f32) as u16;
        (col, row)
    }
}

/// UI-thread terminal state: scroll, selection, preedit, focus and the a11y mirror.
/// The grid itself stays in [`SharedTerminal`].
pub struct TerminalUi {
    pub shared: Arc<SharedTerminal>,
    pub scroll: usize,
    pub selection: Option<Selection>,
    pub preedit: String,
    pub gesture: Option<Gesture>,
    /// Where a selection drag's pointer is, pane-local, while it is down.
    pub drag: Option<DragPoint>,
    pub last_cell: Option<(u16, u16)>,
    pub wheel_rem: f32,
    pub fetch_pending: Option<FetchRequest>,
    pub mirror: String,
    pub mirror_cursor: String,
    pub mirror_version: u64,
    pub mirror_scroll: usize,
    pub built_cursor: Cursor,
    pub cursor_on: bool,
    pub focus_terminal: bool,
    pub attach: Attachment,
    pub attach_generation: u64,
    seen_resync: u64,
    pub requested: Option<((u16, u16), std::time::Instant)>,
    /// The pane's session is an agent's; its alternate screen still takes a cursor click.
    pub agent: bool,
    pub cursor_click: Option<CursorClick>,
    /// The pointer over the pane, pane-local, for a ⌘ pressed without moving.
    pub hover: Option<(f32, f32)>,
    /// The link under a ⌘-hover, underlined by the compositor.
    pub link: Option<crate::terminal::links::Link>,
    /// Where a ⌘-click on a link went down; a drag past the slop cancels it.
    pub link_press: Option<(f32, f32)>,
    fixture: Option<FixtureCache>,
}

struct FixtureCache {
    grids: HashMap<TerminalId, CellGrid>,
    titles: HashMap<TerminalId, String>,
}

impl TerminalUi {
    pub fn new(shared: Arc<SharedTerminal>, fixture: bool) -> Self {
        let mut ui = Self {
            shared,
            scroll: 0,
            selection: None,
            preedit: String::new(),
            gesture: None,
            drag: None,
            last_cell: None,
            wheel_rem: 0.0,
            fetch_pending: None,
            mirror: String::new(),
            mirror_cursor: String::new(),
            mirror_version: u64::MAX,
            mirror_scroll: usize::MAX,
            built_cursor: Cursor::default(),
            cursor_on: true,
            focus_terminal: false,
            attach: Attachment::default(),
            attach_generation: 0,
            seen_resync: 0,
            requested: None,
            agent: false,
            cursor_click: None,
            hover: None,
            link: None,
            link_press: None,
            fixture: None,
        };
        if fixture {
            ui.fixture = Some(FixtureCache {
                grids: HashMap::new(),
                titles: HashMap::new(),
            });
        }
        ui
    }

    pub fn is_fixture(&self) -> bool {
        self.fixture.is_some()
    }

    pub fn is_live(&self) -> bool {
        self.attach.is_live()
    }

    /// The worker/UI attachment target for this pane. A new target or generation
    /// clears the queue; the same target keeps it while a resync is pending.
    pub fn begin_attach(&mut self, generation: u64, terminal: TerminalId) {
        self.attach.begin(generation, terminal);
    }

    pub fn note_snapshot(
        &mut self,
        generation: u64,
        terminal: TerminalId,
        size: (u16, u16),
    ) -> bool {
        self.attach.on_snapshot(generation, terminal, size)
    }

    /// A `TerminalResync` moved the grid under the pane: hold new keys until a
    /// compositor measurement *after* the resync confirms the replacement grid.
    pub fn observe_resync(&mut self, epoch: u64, measure_epoch: u64) -> bool {
        if epoch == self.seen_resync {
            return false;
        }
        self.seen_resync = epoch;
        self.attach.resync(measure_epoch);
        true
    }

    /// A compositor measurement counts only if it rendered after the resync.
    pub fn note_measured_if_fresh(&mut self) -> bool {
        let Some(size) = self.shared.measured() else {
            return false;
        };
        self.attach.on_measured(self.shared.measure_epoch(), size)
    }

    pub fn on_disconnect(&mut self) {
        self.attach.fail();
        self.attach_generation = u64::MAX;
    }

    pub fn on_generation(&mut self, generation: u64) {
        self.attach.new_generation(generation);
        self.attach_generation = u64::MAX;
    }

    #[must_use]
    pub fn hold_key(&mut self, ev: &KeyEvent) -> bool {
        self.attach.hold_key(ev)
    }

    #[must_use]
    pub fn hold_paste(&mut self, text: &str) -> bool {
        self.attach.hold_paste(text)
    }

    pub fn take_held(&mut self) -> Vec<HeldInput> {
        self.attach.take_held()
    }

    #[must_use]
    pub fn attach_notice(&self) -> Option<String> {
        self.attach.notice()
    }

    #[must_use]
    pub fn cursor(&self) -> Option<Cursor> {
        let slot = self.shared.lock();
        slot.grid.as_ref().map(|grid| grid.cursor)
    }

    /// The fixture pool prebuilds every session's grid, so a scripted switch does not
    /// wait on a wall-clock worker while the headless clock runs virtual.
    pub fn set_fixture_titles(&mut self, titles: impl Iterator<Item = (TerminalId, String)>) {
        if let Some(cache) = &mut self.fixture {
            cache.titles.extend(titles);
        }
    }

    pub fn activate_fixture(
        &mut self,
        generation: u64,
        session_id: SessionId,
        terminal_id: TerminalId,
        size: (u16, u16),
    ) -> bool {
        let Some(cache) = &mut self.fixture else {
            return false;
        };
        let pty = domain::PtySize {
            cols: size.0,
            rows: size.1,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut slot = self.shared.lock();
        if slot.generation == generation
            && slot.terminal_id == Some(terminal_id)
            && slot.size == Some(pty)
        {
            return false;
        }
        if let Some(grid) = slot.grid.take()
            && let Some(old) = slot.terminal_id
            && old != terminal_id
        {
            cache.grids.insert(old, grid);
        }
        let title = cache
            .titles
            .get(&terminal_id)
            .cloned()
            .unwrap_or_else(|| "fixture shell".to_string());
        let grid = cache
            .grids
            .remove(&terminal_id)
            .unwrap_or_else(|| fixture::titled_grid(size.0, size.1, &title));
        slot.generation = generation;
        slot.session_id = Some(session_id);
        slot.terminal_id = Some(terminal_id);
        slot.size = Some(pty);
        slot.grid = Some(grid);
        slot.damage = RowDamage::Full;
        slot.version = slot.version.saturating_add(1);
        let version = slot.version;
        drop(slot);
        self.scroll = 0;
        self.selection = None;
        self.preedit.clear();
        self.mirror_version = u64::MAX;
        self.attach_generation = generation;
        self.shared.request_redraw();
        let _ = version;
        true
    }

    pub fn detach(&mut self, generation: u64, session: Option<SessionId>) {
        self.attach.fail();
        self.shared.clear(generation, session);
        self.mirror_version = u64::MAX;
        self.mirror.clear();
        self.mirror_cursor.clear();
        self.scroll = 0;
        self.selection = None;
        self.preedit.clear();
    }

    pub fn request_frame(&self) {
        self.shared.request_redraw();
    }

    /// Rebuild the a11y mirror at most every 125 ms from the current grid and viewport.
    pub fn refresh_mirror(&mut self, version: u64, cols: u16, rows: u16) {
        let mut slot = self.shared.lock();
        let Some(grid) = slot.grid.as_mut() else {
            return;
        };
        let cursor = grid.cursor;
        let mut mirror = std::mem::take(&mut self.mirror);
        mirror.clear();
        let viewport = Viewport {
            grid,
            scroll: self.scroll.min(grid.scrollback_len as usize),
        };
        for v in 0..rows as usize {
            if let Some(row) = viewport.viewport_row(v) {
                let start = mirror.len();
                for cell in row.cells.iter() {
                    if !cell.flags.contains(domain::CellFlags::WIDE_SPACER) {
                        mirror.push_str(&cell.text);
                    }
                }
                let keep = mirror[start..].trim_end().len();
                mirror.truncate(start + keep);
            }
            mirror.push('\n');
        }
        self.mirror = mirror;
        self.mirror_cursor.clear();
        if let Some(row) = grid.visible.get(cursor.line as usize) {
            for cell in row.cells.iter() {
                self.mirror_cursor.push_str(&cell.text);
            }
            let keep = self.mirror_cursor.trim_end().len();
            self.mirror_cursor.truncate(keep);
        }
        self.mirror_version = version;
        self.mirror_scroll = self.scroll;
        self.built_cursor = cursor;
        let _ = cols;
    }
}
