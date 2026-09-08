//! # terminal-core
//!
//! The terminal subsystem of Forge (ForgeNode): the daemon-side PTY layer, the
//! authoritative VT engine, and the pure input mapping (§11). This crate is
//! owned entirely by the daemon (ADR-005); `ui` never depends on it (§17) — the
//! GUI keeps a passive cell replica fed by snapshots and row deltas (ADR-011).
//!
//! ## Layers (§11.1)
//!
//! ```text
//! PtyBackend ─ bytes ─► TerminalEngine ─ damage ─► TerminalDelta ─ IPC ─► GUI
//! ```
//!
//! - [`pty`] — [`PtyBackend`]/[`PtyHandle`] and the `portable-pty`-based
//!   [`PortablePtyBackend`] (§11.2).
//! - [`engine`] — the [`TerminalEngine`] trait (§11.4).
//! - [`alacritty_engine`] — [`AlacrittyEngine`], the default engine on
//!   `alacritty_terminal` (feature `engine-alacritty`, §11.4).
//! - [`snapshot`] — [`DeltaBuilder`]: authoritative grid → snapshot + row deltas
//!   (ADR-011).
//! - [`input`] — pure key/mouse → PTY byte mapping (§11.6).
//!
//! The shared wire types ([`domain::TerminalSnapshot`],
//! [`domain::TerminalDelta`], [`domain::Row`], …) live in `domain::terminal`;
//! this crate produces and consumes them but does not define them.

pub mod engine;
pub mod input;
pub mod pty;
pub mod snapshot;

#[cfg(feature = "engine-alacritty")]
pub mod alacritty_engine;

pub use engine::TerminalEngine;
pub use pty::{ExitStatus, PortablePtyBackend, PtyBackend, PtyError, PtyHandle};
pub use snapshot::DeltaBuilder;

#[cfg(feature = "engine-alacritty")]
pub use alacritty_engine::{AlacrittyEngine, DEFAULT_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES};
