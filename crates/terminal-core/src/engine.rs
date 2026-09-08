//! The [`TerminalEngine`] trait: the single authoritative VT emulator (§11.4).
//!
//! Under ADR-011 exactly one engine runs, in the daemon. It consumes PTY bytes
//! ([`feed`](TerminalEngine::feed)), tracks row-level damage, and yields the
//! shared wire types from [`domain::terminal`] — a full
//! [`TerminalSnapshot`](domain::TerminalSnapshot) on attach/resync and, between
//! polls, only the damaged rows (see [`crate::snapshot::DeltaBuilder`]). The GUI
//! keeps a passive cell replica and never emulates.
//!
//! The trait is behind a feature flag ([`crate::AlacrittyEngine`] is the default
//! implementation) so an alternative VT engine can be swapped in without
//! touching `SessionService` or `ui` (§11.4).

use std::ops::Range;

use domain::{Cursor, Damage, PtySize, Row, TermModes, TerminalSnapshot};

/// The authoritative terminal engine (§11.4).
///
/// Implementations are `Send` because each terminal's engine lives behind the
/// runtime lock and is driven from a dedicated PTY thread (§11.2).
pub trait TerminalEngine: Send {
    /// Feed raw PTY output bytes into the emulator. Escape sequences may be
    /// split across calls; the parser keeps state between them.
    ///
    /// Returns whether consumers should publish the resulting damage. A VT
    /// synchronized-output block (DEC private mode 2026) buffers its body in
    /// the parser and returns `false` until the closing sequence applies the
    /// complete frame. Publishing every buffered chunk defeats the mode and is
    /// visible as prompt/output flicker.
    fn feed(&mut self, bytes: &[u8]) -> bool;

    /// Resize the grid. Triggers full damage on the next
    /// [`take_damage`](Self::take_damage).
    fn resize(&mut self, size: PtySize);

    /// Damaged visible rows since the previous call, or [`Damage::Full`]. Reading
    /// damage resets it (§11.4).
    fn take_damage(&mut self) -> Damage;

    /// A complete snapshot: the visible grid plus `scrollback_tail` trailing
    /// scrollback rows (§11.4). Sent on attach and resync.
    fn snapshot(&self, scrollback_tail: usize) -> TerminalSnapshot;

    /// Rows by index. `0..rows` are visible; **negative indices address
    /// scrollback** (`-1` is the most recent scrollback line). Out-of-range
    /// indices yield blank rows.
    fn rows(&self, range: Range<i64>) -> Vec<Row>;

    /// Current cursor position, shape, and visibility.
    fn cursor(&self) -> Cursor;

    /// Modes needed by the renderer and input mapping (§11.4, §11.6).
    fn modes(&self) -> TermModes;

    /// Current window title, if the program set one (OSC 0/2).
    fn title(&self) -> Option<&str>;

    /// Take-and-clear the pending bell flag.
    fn take_bell(&mut self) -> bool;

    // --- ADR-011 delta bookkeeping ---------------------------------------
    // These two are read-only accessors for values §11.4 already models on
    // `TerminalSnapshot`/`TerminalDelta` (`seq` and `scrollback_len`). They are
    // exposed on the trait so [`crate::snapshot::DeltaBuilder`] can stamp a
    // delta's `seq` and compute `scrolled_lines` without cloning the whole grid
    // through a full [`snapshot`](Self::snapshot) on every poll.

    /// Monotonic sequence number; bumped on every [`feed`](Self::feed). Matches
    /// the `seq` carried by snapshots and deltas (§11.4).
    fn seq(&self) -> u64;

    /// Number of lines currently held in scrollback.
    fn scrollback_len(&self) -> u64;

    /// Number of visible rows (the viewport height). Lets a full repaint pull
    /// `rows(0..screen_lines)` directly instead of allocating a whole
    /// [`snapshot`](Self::snapshot) only to keep its `visible` field (C1).
    fn screen_lines(&self) -> u16;
}
