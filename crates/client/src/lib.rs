//! # client
//!
//! The GUI-side protocol client and passive replica of daemon state (§17).
//! `client` is the only crate that knows the wire protocol on the GUI side; it
//! sits on the `forge-tauri → client → protocol → domain` spine and deliberately
//! depends on neither a GUI toolkit nor `tokio`, so the GUI stays thin and the daemon
//! stays authoritative.
//!
//! Two modules:
//! - [`ipc`] — a synchronous [`Client`] over a Unix domain socket: it performs
//!   the §9.2 handshake, spawns one reader thread, correlates requests with
//!   responses (§10.1) and hands the GUI a stream of [`protocol::DaemonEvent`]s
//!   (§10.3). No `tokio`; just `std` sockets, threads and `flume` channels.
//! - [`store`] — the [`Store`] replica the GUI reads from. Per ADR-011 the
//!   daemon owns the single terminal engine and the client keeps a *cell*
//!   replica ([`CellGrid`]), never a second emulator: it renders a
//!   [`domain::TerminalSnapshot`] and applies [`domain::TerminalDelta`] row
//!   diffs, following the sequence/resync rules of §10.5 exactly (see
//!   [`DeltaOutcome`]).

mod ipc;
mod store;

pub use ipc::{Branches, Client, ClientError, DaemonInfo, SendContextResult};
pub use store::{CellGrid, DeltaOutcome, EventOutcome, Store};
// Re-exported so a GUI can name what `Store::providers` holds, and what
// `Client::events` yields, without taking a direct dependency on `protocol`
// (§17: the client is the GUI's gateway to the wire types).
pub use protocol::{
    DaemonEvent, DaemonStats, ErrorCode, ProtocolError, ProviderInfo, RemoveProjectPolicy,
    SendContextSpawn, PROTOCOL_VERSION,
};
pub use terminal_input::{
    encode_key, encode_mouse, paste, Key, Modifiers, MouseButton, MouseEventKind,
};
