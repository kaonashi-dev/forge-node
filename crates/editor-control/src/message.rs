//! The messages that travel the editor control channel.
//!
//! Directions are named from the daemon's point of view: [`DaemonMessage`] is
//! what the daemon sends, [`EditorMessage`] what the editor sends back. The
//! handshake is the first exchange in each direction (`Hello` then `Welcome`);
//! every request after it carries a `request_id` that its answer echoes.

use crate::CONTROL_VERSION;
use serde::{Deserialize, Serialize};

/// Whether a peer's advertised control version is this build's.
#[must_use]
pub fn version_matches(version: u16) -> bool {
    version == CONTROL_VERSION
}

/// What the daemon sends to one editor process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DaemonMessage {
    /// Handshake answer to [`EditorMessage::Hello`].
    Welcome {
        version: u16,
        /// Opaque identity the daemon minted for this instance; echoed so a
        /// stale connection from a previous editor cannot be mistaken for the
        /// live one.
        session_id: String,
        /// Buffer the daemon is about to open.
        buffer_id: u64,
    },
    /// The buffer: text and the disk revision the daemon read it at.
    ///
    /// `text` is the only message that can approach the frame cap; the daemon
    /// reads the file through `fs-service` and the editor never touches the
    /// checkout. `path` is display metadata only.
    Open {
        request_id: u64,
        buffer_id: u64,
        /// Workspace-relative, as the daemon resolved it.
        path: String,
        text: String,
        /// `fs_service` revision this text was read at; the editor records it
        /// and future save requests must present it.
        revision: Option<String>,
        /// 1-based line to reveal, if the opener asked for one.
        line: Option<u32>,
        read_only: bool,
    },
    /// Move the caret to a line (and optionally a display column).
    Reveal {
        request_id: u64,
        line: u32,
        column: Option<u32>,
    },
    /// Answer [`EditorMessage::State`] with an uncaptured snapshot.
    GetState { request_id: u64 },
    /// Replace byte ranges, refused when the document moved under the caller.
    ///
    /// Reserved for a preview surface; H1's daemon does not send it yet, so an
    /// editor that receives one may refuse it.
    ApplyPreviewEdit {
        request_id: u64,
        expected_document_version: u64,
        edits: Vec<WireEdit>,
    },
}

/// What the editor sends to the daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorMessage {
    /// First message on a fresh connection.
    Hello {
        version: u16,
        /// The identity the daemon named in `FORGE_SESSION_ID`, echoed back.
        session_id: String,
        /// The editor's process id, for a log line that ties a socket to a pid.
        pid: u32,
    },
    /// [`DaemonMessage::Open`] succeeded.
    Opened {
        request_id: u64,
        document_version: u64,
    },
    /// [`DaemonMessage::Reveal`] succeeded.
    Revealed { request_id: u64 },
    /// [`DaemonMessage::ApplyPreviewEdit`] succeeded.
    Applied {
        request_id: u64,
        document_version: u64,
    },
    /// A request was understood and refused; `reason` is display text.
    Refused { request_id: u64, reason: String },
    /// State after a change, or in answer to [`DaemonMessage::GetState`].
    ///
    /// A notification carries no `request_id` and coalesces; the answer to
    /// `GetState` echoes its id and is an uncaptured snapshot.
    State {
        request_id: Option<u64>,
        state: EditorStateWire,
    },
    /// The editor is exiting (quit command, fatal error). The daemon treats the
    /// socket EOF the same way, so this is a courtesy reason, not the signal.
    Closed { reason: String },
}

/// The buffer metadata an editor publishes.
///
/// Mirrors `domain::EditorState` without depending on it: the editor binary
/// carries no Forge types. The daemon converts at the boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorStateWire {
    /// Workspace-relative path.
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based.
    pub column: u32,
    pub dirty: bool,
    pub read_only: bool,
    /// Monotonic document version from `editor-core`.
    pub document_version: u64,
}

/// One byte-range replacement on the wire.
///
/// `from`/`to` are UTF-8 byte offsets into the document; an empty range
/// inserts. Defined here instead of reusing `editor_core::Edit` so the wire
/// stays independent of the core's internals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEdit {
    pub from: u64,
    pub to: u64,
    pub insert: String,
}
