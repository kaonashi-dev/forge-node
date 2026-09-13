//! Daemon-side PTY backend and the only VT engine (ADR-005, ADR-011).
//!
//! `forge-tauri` never depends on this crate. Grid wire types live in
//! `domain::terminal`; this crate produces snapshots and row deltas from them.

pub mod engine;
pub mod input;
pub mod pty;
pub mod snapshot;

#[cfg(feature = "engine-alacritty")]
pub mod alacritty_engine;

pub use engine::TerminalEngine;
pub use pty::{ExitStatus, PortablePtyBackend, PtyBackend, PtyError, PtyHandle, PtyReader};
pub use snapshot::DeltaBuilder;

#[cfg(feature = "engine-alacritty")]
pub use alacritty_engine::{AlacrittyEngine, DEFAULT_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES};
