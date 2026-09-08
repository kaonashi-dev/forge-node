//! Snapshot and row-delta construction — the ADR-011 core.
//!
//! The authoritative engine ([`TerminalEngine`]) is the single source of truth
//! for the grid. On attach/resync the daemon sends a full
//! [`TerminalSnapshot`]; between polls it sends a [`TerminalDelta`] carrying
//! only the rows the engine marked damaged, plus the cursor, modes, and how
//! many lines scrolled into history. The GUI keeps a passive cell replica and
//! applies these (§10.5, §11.4). This module turns "engine + damage since the
//! last poll" into those two messages.

use domain::{Damage, Row, TerminalDelta, TerminalSnapshot};

use crate::engine::TerminalEngine;

/// Builds snapshots and incremental deltas for one terminal, tracking the
/// scrollback length needed to compute `scrolled_lines` across polls.
///
/// One builder per attached terminal. Take a [`snapshot`](Self::snapshot) at
/// attach/resync to (re)establish the baseline, then call [`delta`](Self::delta)
/// after each engine poll.
#[derive(Debug, Default)]
pub struct DeltaBuilder {
    /// Scrollback length observed at the previous snapshot/delta.
    last_scrollback_len: u64,
}

impl DeltaBuilder {
    /// Create a builder with no baseline yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_scrollback_len: 0,
        }
    }

    /// Produce a full snapshot (visible grid + `scrollback_tail` trailing
    /// scrollback rows) and (re)set the delta baseline. Use on attach and on
    /// resync (§11.4).
    pub fn snapshot<E: TerminalEngine + ?Sized>(
        &mut self,
        engine: &E,
        scrollback_tail: usize,
    ) -> TerminalSnapshot {
        let snap = engine.snapshot(scrollback_tail);
        self.last_scrollback_len = snap.scrollback_len;
        snap
    }

    /// Produce the delta since the previous poll: only the damaged visible rows,
    /// the current cursor and modes, and how many lines entered scrollback.
    ///
    /// Reads and clears the engine's damage ([`take_damage`]). On
    /// [`Damage::Full`] every visible row is emitted (e.g. after a resize or
    /// alt-screen switch).
    ///
    /// [`take_damage`]: TerminalEngine::take_damage
    pub fn delta<E: TerminalEngine + ?Sized>(&mut self, engine: &mut E) -> TerminalDelta {
        let seq = engine.seq();
        let cursor = engine.cursor();
        let modes = engine.modes();
        let scrollback_len = engine.scrollback_len();

        let damage = engine.take_damage();
        let rows: Vec<(u16, Row)> = match damage {
            // `Partial` is the common path: pull just the damaged rows.
            Damage::Partial(indices) => indices
                .into_iter()
                .filter_map(|i| {
                    engine
                        .rows(i64::from(i)..i64::from(i) + 1)
                        .into_iter()
                        .next()
                        .map(|row| (i, row))
                })
                .collect(),
            // `Full` (resize, alt-screen switch, clear/reset, DECALN) falls back
            // to a full repaint. Pull the visible rows directly rather than
            // building a whole `snapshot(0)` and discarding everything but its
            // `visible` field: `seq`, `cursor`, `modes` and `scrollback_len` were
            // already read above, and a snapshot re-derives all of them, clones
            // the title, and allocates a `TerminalSnapshot` only to drop it (C1).
            //
            // NOTE: this is deliberately *not* narrowed to the scrolled rows.
            // A line feed at the bottom does mark the grid fully damaged
            // (alacritty's `scroll_up` → `mark_fully_damaged`), but once that
            // flag is set `Term::damage()` returns `Full` and collapses the
            // per-line damage that a mid-screen edit in the same chunk would
            // otherwise carry (e.g. a status line above a DECSTBM scroll
            // region). Emitting only the newly scrolled-in rows would silently
            // drop that edit, and recovering the true damage would need a shadow
            // grid, which the replica model forbids. So a scroll still costs a
            // full repaint here; only the wasteful snapshot alloc is removed.
            _ => {
                let screen = i64::from(engine.screen_lines());
                engine
                    .rows(0..screen)
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| (i as u16, row))
                    .collect()
            }
        };

        // `scrolled_lines` is derived from the growth of scrollback length. Once
        // scrollback saturates at its configured maximum this reports 0 even
        // though content keeps scrolling; that is acceptable for the GUI replica,
        // which fetches out-of-window scrollback on demand (§11.5).
        let scrolled_lines = scrollback_len.saturating_sub(self.last_scrollback_len);
        self.last_scrollback_len = scrollback_len;

        TerminalDelta {
            seq,
            rows,
            scrolled_lines: scrolled_lines.min(u64::from(u32::MAX)) as u32,
            cursor,
            modes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ops::Range;

    use domain::{Cell, CellFlags, Color, Cursor, CursorShape, MouseMode, PtySize, TermModes};

    /// A one-cell row whose text identifies it, so a delta's rows can be
    /// matched back to the grid position they were pulled from.
    fn row(text: &str) -> Row {
        Row {
            cells: vec![Cell {
                text: text.into(),
                fg: Color::Default,
                bg: Color::Default,
                flags: CellFlags::empty(),
            }],
            wrapped: false,
        }
    }

    fn text_of(row: &Row) -> String {
        row.cells.iter().map(|c| c.text.as_str()).collect()
    }

    /// A scriptable [`TerminalEngine`] with no VT parsing: the test sets the
    /// grid, the damage, and the scrollback length directly, so a failure points
    /// at [`DeltaBuilder`] rather than at the emulator.
    struct FakeEngine {
        visible: Vec<Row>,
        scrollback: Vec<Row>,
        damage: Damage,
        /// How many times [`TerminalEngine::take_damage`] was called.
        damage_reads: usize,
        seq: u64,
        scrollback_len: u64,
        cursor: Cursor,
        modes: TermModes,
        /// When set, out-of-range indices return nothing instead of a blank row.
        omit_out_of_range: bool,
    }

    impl FakeEngine {
        fn new(rows: usize) -> Self {
            Self {
                visible: (0..rows).map(|i| row(&format!("v{i}"))).collect(),
                scrollback: Vec::new(),
                damage: Damage::Partial(Vec::new()),
                damage_reads: 0,
                seq: 0,
                scrollback_len: 0,
                cursor: Cursor::default(),
                modes: TermModes::default(),
                omit_out_of_range: false,
            }
        }

        fn damaged(mut self, indices: &[u16]) -> Self {
            self.damage = Damage::Partial(indices.to_vec());
            self
        }

        fn scrolled_to(mut self, len: u64) -> Self {
            self.scrollback_len = len;
            self
        }
    }

    impl TerminalEngine for FakeEngine {
        fn feed(&mut self, _bytes: &[u8]) -> bool {
            self.seq += 1;
            true
        }

        fn resize(&mut self, _size: PtySize) {
            self.damage = Damage::Full;
        }

        fn take_damage(&mut self) -> Damage {
            self.damage_reads += 1;
            std::mem::replace(&mut self.damage, Damage::Partial(Vec::new()))
        }

        fn snapshot(&self, scrollback_tail: usize) -> TerminalSnapshot {
            let tail_start = self.scrollback.len().saturating_sub(scrollback_tail);
            TerminalSnapshot {
                seq: self.seq,
                size: PtySize {
                    cols: 1,
                    rows: u16::try_from(self.visible.len()).unwrap(),
                    pixel_width: 0,
                    pixel_height: 0,
                },
                visible: self.visible.clone(),
                scrollback_tail: self.scrollback[tail_start..].to_vec(),
                scrollback_len: self.scrollback_len,
                cursor: self.cursor,
                modes: self.modes,
                title: None,
            }
        }

        fn rows(&self, range: Range<i64>) -> Vec<Row> {
            range
                .filter_map(|i| {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| self.visible.get(i).cloned())
                        .or_else(|| (!self.omit_out_of_range).then(|| Row::blank(1)))
                })
                .collect()
        }

        fn cursor(&self) -> Cursor {
            self.cursor
        }

        fn modes(&self) -> TermModes {
            self.modes
        }

        fn title(&self) -> Option<&str> {
            None
        }

        fn take_bell(&mut self) -> bool {
            false
        }

        fn seq(&self) -> u64 {
            self.seq
        }

        fn scrollback_len(&self) -> u64 {
            self.scrollback_len
        }

        fn screen_lines(&self) -> u16 {
            u16::try_from(self.visible.len()).unwrap()
        }
    }

    #[test]
    fn partial_damage_emits_only_the_damaged_rows_with_their_indices() {
        let mut engine = FakeEngine::new(5).damaged(&[1, 3]);
        let delta = DeltaBuilder::new().delta(&mut engine);

        let seen: Vec<(u16, String)> = delta.rows.iter().map(|(i, r)| (*i, text_of(r))).collect();
        assert_eq!(
            seen,
            vec![(1, "v1".to_owned()), (3, "v3".to_owned())],
            "each damaged index must carry that row's contents"
        );
    }

    #[test]
    fn no_damage_emits_no_rows_but_still_carries_cursor_and_modes() {
        let mut engine = FakeEngine::new(3);
        engine.seq = 17;
        engine.cursor = Cursor {
            line: 2,
            col: 9,
            shape: CursorShape::Beam,
            visible: true,
        };
        engine.modes = TermModes {
            alt_screen: true,
            bracketed_paste: true,
            mouse_mode: MouseMode::AnyEvent,
            ..TermModes::default()
        };

        let delta = DeltaBuilder::new().delta(&mut engine);

        assert!(delta.rows.is_empty());
        assert_eq!(delta.seq, 17, "the delta is stamped with the engine's seq");
        assert_eq!(delta.cursor.line, 2);
        assert_eq!(delta.cursor.col, 9);
        assert_eq!(delta.cursor.shape, CursorShape::Beam);
        assert!(delta.modes.alt_screen);
        assert_eq!(delta.modes.mouse_mode, MouseMode::AnyEvent);
    }

    #[test]
    fn full_damage_repaints_every_visible_row_in_order() {
        let mut engine = FakeEngine::new(4);
        engine.damage = Damage::Full;

        let delta = DeltaBuilder::new().delta(&mut engine);

        let seen: Vec<(u16, String)> = delta.rows.iter().map(|(i, r)| (*i, text_of(r))).collect();
        assert_eq!(
            seen,
            vec![
                (0, "v0".to_owned()),
                (1, "v1".to_owned()),
                (2, "v2".to_owned()),
                (3, "v3".to_owned()),
            ]
        );
    }

    /// Damage is take-and-clear: a second poll with nothing new must be empty,
    /// or the daemon would resend the same rows every frame.
    #[test]
    fn damage_is_consumed_exactly_once_per_delta() {
        let mut engine = FakeEngine::new(3).damaged(&[0]);
        let mut builder = DeltaBuilder::new();

        assert_eq!(builder.delta(&mut engine).rows.len(), 1);
        assert_eq!(builder.delta(&mut engine).rows.len(), 0);
        assert_eq!(engine.damage_reads, 2);
    }

    /// The engine contract says an out-of-range index yields a blank row; the
    /// builder must pass it through rather than shifting the remaining indices.
    #[test]
    fn an_out_of_range_damaged_index_is_reported_blank() {
        let mut engine = FakeEngine::new(2).damaged(&[0, 99]);
        let delta = DeltaBuilder::new().delta(&mut engine);

        assert_eq!(delta.rows.len(), 2);
        assert_eq!(delta.rows[0].0, 0);
        assert_eq!(delta.rows[1], (99, Row::blank(1)));
    }

    /// An engine that returns nothing for an out-of-range index must not panic
    /// or misalign the rest: the row is simply dropped.
    #[test]
    fn a_row_the_engine_cannot_produce_is_dropped_not_panicked_on() {
        let mut engine = FakeEngine::new(2).damaged(&[99, 1]);
        engine.omit_out_of_range = true;

        let delta = DeltaBuilder::new().delta(&mut engine);

        assert_eq!(delta.rows.len(), 1);
        assert_eq!(delta.rows[0].0, 1);
        assert_eq!(text_of(&delta.rows[0].1), "v1");
    }

    #[test]
    fn scrolled_lines_counts_growth_since_the_previous_poll_only() {
        let mut engine = FakeEngine::new(2);
        let mut builder = DeltaBuilder::new();

        // First poll with no baseline: everything already in scrollback counts.
        engine.scrollback_len = 10;
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 10);

        // Standing still reports nothing.
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 0);

        engine.scrollback_len = 13;
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 3);
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 0);
    }

    /// A snapshot re-establishes the baseline, so the rows it already carried
    /// are not reported again as freshly scrolled by the next delta (§11.4).
    #[test]
    fn a_snapshot_resets_the_delta_baseline() {
        let mut engine = FakeEngine::new(2).scrolled_to(40);
        let mut builder = DeltaBuilder::new();

        let snapshot = builder.snapshot(&engine, 0);
        assert_eq!(snapshot.scrollback_len, 40);
        assert_eq!(
            builder.delta(&mut engine).scrolled_lines,
            0,
            "a resync must not replay the scrollback the snapshot already held"
        );

        engine.scrollback_len = 42;
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 2);
    }

    /// Scrollback shrinks when the buffer is reset (`clear`, alt-screen exit).
    /// The subtraction saturates instead of wrapping into a huge count.
    #[test]
    fn a_shrinking_scrollback_reports_zero_rather_than_underflowing() {
        let mut engine = FakeEngine::new(2).scrolled_to(100);
        let mut builder = DeltaBuilder::new();
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 100);

        engine.scrollback_len = 5;
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 0);

        // The baseline follows the shrink, so growth from there is exact again.
        engine.scrollback_len = 9;
        assert_eq!(builder.delta(&mut engine).scrolled_lines, 4);
    }

    /// `scrollback_len` is a `u64` but the wire field is a `u32`; growth past
    /// that clamps rather than truncating to a small number.
    #[test]
    fn scrolled_lines_clamps_to_the_wire_width() {
        let mut engine = FakeEngine::new(2).scrolled_to(u64::from(u32::MAX) + 5);
        let delta = DeltaBuilder::new().delta(&mut engine);
        assert_eq!(delta.scrolled_lines, u32::MAX);
    }

    #[test]
    fn a_snapshot_passes_the_requested_tail_length_to_the_engine() {
        let mut engine = FakeEngine::new(2);
        engine.scrollback = (0..5).map(|i| row(&format!("s{i}"))).collect();
        engine.scrollback_len = 5;

        let mut builder = DeltaBuilder::new();
        let snapshot = builder.snapshot(&engine, 2);

        let tail: Vec<String> = snapshot.scrollback_tail.iter().map(text_of).collect();
        assert_eq!(tail, vec!["s3".to_owned(), "s4".to_owned()]);
        assert_eq!(snapshot.visible.len(), 2);
    }

    #[test]
    fn a_fresh_builder_has_no_baseline() {
        let builder = DeltaBuilder::default();
        assert_eq!(builder.last_scrollback_len, 0);
    }
}
