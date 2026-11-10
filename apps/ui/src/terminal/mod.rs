use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use client::{CellGrid, DeltaOutcome, RowDamage};
use domain::{PtySize, ScrollbackRows, SessionId, TerminalDelta, TerminalId, TerminalSnapshot};

pub mod alloc;
pub mod atlas;
pub mod attach;
pub mod clip;
pub mod fixture;
pub mod host;
pub mod input;
pub mod links;
pub mod palette;
pub mod pane;
pub mod renderer;
pub mod selection;
pub mod shaping;
pub mod split;
pub mod stats;

/// State the worker publishes behind one short lock. `grid` is the only copy of the
/// terminal replica; the renderer never clones it, it reads rows as `Arc<Vec<Cell>>`.
pub struct Slot {
    pub generation: u64,
    pub session_id: Option<SessionId>,
    pub terminal_id: Option<TerminalId>,
    pub grid: Option<CellGrid>,
    /// PTY size the daemon answered with; the resize gate compares against it.
    pub size: Option<PtySize>,
    pub damage: RowDamage,
    pub version: u64,
}

/// The terminal half of the app. Workers write, the UI thread reads under a short
/// lock; publication is latest-wins and its damage merges until a render takes it.
pub struct SharedTerminal {
    slot: Mutex<Slot>,
    measured: Mutex<Option<(u16, u16)>>,
    measure_epoch: AtomicU64,
    grid_epoch: AtomicU64,
    resync_epoch: AtomicU64,
    render_epoch: AtomicU64,
    rendered_epoch: AtomicU64,
    notify_enabled: AtomicBool,
    notifier: Mutex<Option<flume::Sender<u64>>>,
    /// The split column's replica; only the primary has one.
    beside: Option<Arc<SharedTerminal>>,
}

pub struct Meta {
    pub generation: u64,
    pub session_id: Option<SessionId>,
    pub terminal_id: Option<TerminalId>,
    pub size: Option<PtySize>,
    pub has_grid: bool,
}

impl SharedTerminal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::bare(Some(Arc::new(Self::bare(None)))))
    }

    fn bare(beside: Option<Arc<SharedTerminal>>) -> Self {
        Self {
            slot: Mutex::new(Slot {
                generation: 0,
                session_id: None,
                terminal_id: None,
                grid: None,
                size: None,
                damage: RowDamage::Full,
                version: 0,
            }),
            measured: Mutex::new(None),
            measure_epoch: AtomicU64::new(0),
            grid_epoch: AtomicU64::new(0),
            resync_epoch: AtomicU64::new(0),
            render_epoch: AtomicU64::new(0),
            rendered_epoch: AtomicU64::new(0),
            notify_enabled: AtomicBool::new(false),
            notifier: Mutex::new(None),
            beside,
        }
    }

    /// The split column's replica, beside this primary one.
    #[must_use]
    pub fn beside(&self) -> Option<&Arc<SharedTerminal>> {
        self.beside.as_ref()
    }

    /// The replica, primary or split, that holds `terminal`.
    #[must_use]
    pub fn holding(&self, terminal: TerminalId) -> Option<&SharedTerminal> {
        if self.lock().terminal_id == Some(terminal) {
            return Some(self);
        }
        self.beside
            .as_deref()
            .filter(|beside| beside.lock().terminal_id == Some(terminal))
    }

    pub fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[must_use]
    pub fn meta(&self) -> Meta {
        let slot = self.lock();
        Meta {
            generation: slot.generation,
            session_id: slot.session_id,
            terminal_id: slot.terminal_id,
            size: slot.size,
            has_grid: slot.grid.is_some(),
        }
    }

    pub fn publish_snapshot(
        &self,
        generation: u64,
        session_id: SessionId,
        terminal_id: TerminalId,
        snapshot: &TerminalSnapshot,
    ) {
        let grid = CellGrid::from_snapshot(snapshot);
        self.publish_grid(generation, session_id, terminal_id, grid, snapshot.size);
    }

    pub fn publish_grid(
        &self,
        generation: u64,
        session_id: SessionId,
        terminal_id: TerminalId,
        grid: CellGrid,
        size: PtySize,
    ) {
        let mut slot = self.lock();
        slot.generation = generation;
        slot.session_id = Some(session_id);
        slot.terminal_id = Some(terminal_id);
        slot.size = Some(size);
        slot.grid = Some(grid);
        slot.damage = RowDamage::Full;
        slot.version = slot.version.saturating_add(1);
        let version = slot.version;
        drop(slot);
        self.grid_epoch.store(version, Ordering::Release);
        self.notify(version);
        self.render_epoch.fetch_add(1, Ordering::Release);
    }

    pub fn clear(&self, generation: u64, session_id: Option<SessionId>) {
        let mut slot = self.lock();
        slot.generation = generation;
        slot.session_id = session_id;
        slot.terminal_id = None;
        slot.size = None;
        slot.grid = None;
        slot.damage = RowDamage::Rows(Vec::new());
        slot.version = slot.version.saturating_add(1);
        let version = slot.version;
        drop(slot);
        self.grid_epoch.store(version, Ordering::Release);
        self.notify(version);
        self.render_epoch.fetch_add(1, Ordering::Release);
    }

    /// Applies one delta to the replica. The worker calls this for every delta, in
    /// order, even when no frame rendered since the last one.
    pub fn apply_delta(
        &self,
        generation: u64,
        terminal_id: TerminalId,
        delta: &TerminalDelta,
    ) -> DeltaOutcome {
        let mut slot = self.lock();
        if slot.generation != generation || slot.terminal_id != Some(terminal_id) {
            return DeltaOutcome::Stale;
        }
        let Some(grid) = slot.grid.as_mut() else {
            return DeltaOutcome::Stale;
        };
        let outcome = grid.apply_delta(delta);
        if outcome == DeltaOutcome::Applied {
            slot.damage = std::mem::take(&mut slot.damage).merge(RowDamage::of_delta(delta));
            slot.version = slot.version.saturating_add(1);
            let version = slot.version;
            self.grid_epoch.store(version, Ordering::Release);
            self.notify(version);
            self.render_epoch.fetch_add(1, Ordering::Release);
        }
        outcome
    }

    pub fn resync(&self, generation: u64, terminal_id: TerminalId, snapshot: &TerminalSnapshot) {
        let mut slot = self.lock();
        if slot.generation != generation || slot.terminal_id != Some(terminal_id) {
            return;
        }
        let Some(grid) = slot.grid.as_mut() else {
            return;
        };
        grid.apply_resync(snapshot);
        slot.size = Some(snapshot.size);
        slot.damage = RowDamage::Full;
        slot.version = slot.version.saturating_add(1);
        let version = slot.version;
        drop(slot);
        self.grid_epoch.store(version, Ordering::Release);
        self.notify(version);
        self.resync_epoch.fetch_add(1, Ordering::Release);
        self.render_epoch.fetch_add(1, Ordering::Release);
    }

    pub fn merge_scrollback(
        &self,
        generation: u64,
        terminal_id: TerminalId,
        block: &ScrollbackRows,
    ) -> bool {
        let mut slot = self.lock();
        if slot.generation != generation || slot.terminal_id != Some(terminal_id) {
            return false;
        }
        let Some(grid) = slot.grid.as_mut() else {
            return false;
        };
        grid.merge_scrollback(block);
        slot.version = slot.version.saturating_add(1);
        let version = slot.version;
        drop(slot);
        self.grid_epoch.store(version, Ordering::Release);
        self.notify(version);
        self.render_epoch.fetch_add(1, Ordering::Release);
        true
    }

    /// A benchmark-only wake for the headless echo measurement. Production
    /// never installs a notifier; the compositor is woken through Rune instead.
    pub fn subscribe(&self) -> flume::Receiver<u64> {
        self.notify_enabled.store(true, Ordering::Release);
        let (tx, rx) = flume::bounded(1);
        *self
            .notifier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(tx);
        rx
    }

    fn notify(&self, version: u64) {
        if !self.notify_enabled.load(Ordering::Acquire) {
            return;
        }
        if let Some(tx) = self
            .notifier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            let _ = tx.try_send(version);
        }
    }

    #[must_use]
    pub fn grid_epoch(&self) -> u64 {
        self.grid_epoch.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn resync_epoch(&self) -> u64 {
        self.resync_epoch.load(Ordering::Acquire)
    }

    pub fn request_redraw(&self) {
        self.render_epoch.fetch_add(1, Ordering::Release);
    }

    #[must_use]
    pub fn animating(&self) -> bool {
        self.render_epoch.load(Ordering::Acquire) != self.rendered_epoch.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn render_epoch(&self) -> u64 {
        self.render_epoch.load(Ordering::Acquire)
    }

    pub fn mark_rendered_to(&self, epoch: u64) {
        self.rendered_epoch.store(epoch, Ordering::Release);
    }

    #[cfg(test)]
    pub fn mark_rendered(&self) {
        let epoch = self.render_epoch.load(Ordering::Acquire);
        self.rendered_epoch.store(epoch, Ordering::Release);
    }

    pub fn set_measured(&self, cols: u16, rows: u16) -> bool {
        let mut measured = self
            .measured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = *measured != Some((cols, rows));
        *measured = Some((cols, rows));
        self.measure_epoch.fetch_add(1, Ordering::Release);
        changed
    }

    /// Advances on every compositor measurement, including an unchanged one, so
    /// a resync can require a frame that rendered after it.
    #[must_use]
    pub fn measure_epoch(&self) -> u64 {
        self.measure_epoch.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn measured(&self) -> Option<(u16, u16)> {
        *self
            .measured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Cell, Cursor, Row, Row as TermRow, TermModes};

    use std::sync::Arc;

    fn row(text: &str, cols: usize) -> Row {
        let mut cells = vec![Cell::default(); cols];
        for (i, c) in text.chars().take(cols).enumerate() {
            cells[i].text = c.to_string().into();
        }
        Row {
            cells: Arc::new(cells),
            wrapped: false,
        }
    }

    fn snapshot(cols: u16, rows: u16, seq: u64) -> TerminalSnapshot {
        TerminalSnapshot {
            seq,
            size: PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            },
            visible: (0..rows).map(|_| TermRow::blank(cols)).collect(),
            scrollback_tail: Vec::new(),
            scrollback_len: 0,
            scrollback_generation: 1,
            cursor: Cursor::default(),
            modes: TermModes::default(),
            title: None,
        }
    }

    fn delta(seq: u64, rows: Vec<(u16, Row)>, scrolled: u32) -> TerminalDelta {
        TerminalDelta {
            seq,
            rows,
            patches: Vec::new(),
            scrollback_len: 0,
            scrollback_generation: 1,
            scrolled_lines: scrolled,
            cursor: Cursor::default(),
            modes: TermModes::default(),
        }
    }

    #[test]
    fn every_delta_applies_in_order_even_when_publication_is_skipped() {
        let shared = SharedTerminal::new();
        let session = SessionId::new();
        let terminal = TerminalId::new();
        shared.publish_snapshot(1, session, terminal, &snapshot(4, 2, 1));
        assert_eq!(
            shared.apply_delta(1, terminal, &delta(2, vec![(0, row("ab", 4))], 0)),
            DeltaOutcome::Applied
        );
        assert_eq!(
            shared.apply_delta(1, terminal, &delta(3, vec![(1, row("cd", 4))], 1)),
            DeltaOutcome::Applied
        );
        assert_eq!(
            shared.apply_delta(1, terminal, &delta(4, vec![(0, row("ef", 4))], 0)),
            DeltaOutcome::Applied
        );
        let slot = shared.lock();
        let grid = slot.grid.as_ref().unwrap();
        assert_eq!(grid.last_seq, 4);
        assert_eq!(grid.visible[0].cells[0].text, "e");
        assert_eq!(grid.visible[1].cells[0].text, "c");
        assert_eq!(
            slot.damage,
            RowDamage::Full,
            "a scroll widened the merged damage"
        );
    }

    #[test]
    fn skipped_publication_accumulates_damage_rows() {
        let shared = SharedTerminal::new();
        let session = SessionId::new();
        let terminal = TerminalId::new();
        shared.publish_snapshot(1, session, terminal, &snapshot(4, 4, 1));
        shared.lock().damage = RowDamage::default();
        shared.apply_delta(1, terminal, &delta(2, vec![(1, row("a", 4))], 0));
        shared.apply_delta(1, terminal, &delta(3, vec![(2, row("b", 4))], 0));
        let mut slot = shared.lock();
        assert_eq!(slot.damage, RowDamage::Rows(vec![1, 2]));
        slot.damage = RowDamage::default();
        assert_eq!(slot.grid.as_ref().unwrap().last_seq, 3);
    }

    #[test]
    fn stale_generation_and_other_terminal_events_are_dropped() {
        let shared = SharedTerminal::new();
        let session = SessionId::new();
        let terminal = TerminalId::new();
        shared.publish_snapshot(2, session, terminal, &snapshot(4, 2, 1));
        assert_eq!(
            shared.apply_delta(1, terminal, &delta(2, vec![(0, row("x", 4))], 0)),
            DeltaOutcome::Stale
        );
        assert_eq!(
            shared.apply_delta(2, TerminalId::new(), &delta(2, vec![(0, row("x", 4))], 0)),
            DeltaOutcome::Stale
        );
        let slot = shared.lock();
        assert_eq!(slot.grid.as_ref().unwrap().last_seq, 1);
    }

    #[test]
    fn a_sequence_gap_is_reported_and_nothing_is_applied() {
        let shared = SharedTerminal::new();
        let session = SessionId::new();
        let terminal = TerminalId::new();
        shared.publish_snapshot(1, session, terminal, &snapshot(4, 2, 5));
        assert_eq!(
            shared.apply_delta(1, terminal, &delta(9, vec![(0, row("x", 4))], 0)),
            DeltaOutcome::NeedsResync
        );
        let slot = shared.lock();
        assert_eq!(slot.grid.as_ref().unwrap().last_seq, 5);
        assert!(
            slot.grid.as_ref().unwrap().visible[0].cells[0]
                .text
                .trim()
                .is_empty()
        );
    }

    #[test]
    fn one_redraw_epoch_is_consumed_once() {
        let shared = SharedTerminal::new();
        assert!(!shared.animating());
        shared.request_redraw();
        assert!(shared.animating());
        shared.mark_rendered();
        assert!(!shared.animating());
        shared.request_redraw();
        shared.mark_rendered();
        assert!(!shared.animating());
    }
}
