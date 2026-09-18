//! GUI-side protocol client and passive cell replica (ADR-011).
//!
//! Blocking `std` Unix sockets and one reader thread — not tokio. The GUI
//! never links a VT engine; [`CellGrid`] applies snapshots and row deltas.
//! Wire types are re-exported so `forge-tauri` does not depend on `protocol`.

mod ipc;
mod store;

pub use domain::{DirectoryListing, SymlinkTarget};
pub use ipc::{Branches, Client, ClientError, DaemonInfo, SendContextResult};
pub use store::{CellGrid, DeltaOutcome, EventOutcome, Store};
// Re-exported so a GUI can name what `Store::providers` holds, and what
// `Client::events` yields, without taking a direct dependency on `protocol`
// (§17: the client is the GUI's gateway to the wire types).
pub use protocol::{
    DaemonEvent, DaemonStats, EditorSurface, ErrorCode, ProtocolError, ProviderInfo,
    RemoveProjectPolicy, SendContextSpawn, PROTOCOL_VERSION,
};
pub use terminal_input::{
    encode_key, encode_mouse, paste, Key, Modifiers, MouseButton, MouseEventKind,
};
