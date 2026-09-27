//! GUI-side protocol client and passive cell replica (ADR-011).
//!
//! Blocking `std` Unix sockets and one reader thread — not tokio. The GUI
//! never links a VT engine; [`CellGrid`] applies snapshots and row deltas.
//! Wire types are re-exported so `forge-tauri` does not depend on `protocol`.

mod clamp;
mod ipc;
mod paths;
mod store;

pub use clamp::{read_capped, CappedBytes};
pub use paths::{resolve_socket, socket_path, SocketQuery, MAX_SOCKET_PATH_LEN};

pub use domain::{DirectoryListing, SymlinkTarget};
pub use ipc::{Branches, Client, ClientError, DaemonInfo, SendContextResult};
pub use protocol::{
    DaemonEvent, DaemonStats, EditorSurface, ErrorCode, ProtocolError, ProviderInfo,
    RemoveProjectPolicy, SendContextSpawn, PROTOCOL_VERSION,
};
pub use store::{CellGrid, DeltaOutcome, EventOutcome, Store};
pub use terminal_input::{
    encode_key, encode_mouse, paste, Key, Modifiers, MouseButton, MouseEventKind,
};
