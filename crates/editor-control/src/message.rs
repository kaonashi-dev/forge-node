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
        /// Save on a pause, without being asked. The opener's preference,
        /// carried here so the editor does not have to know about GUI flags.
        #[serde(default)]
        autosave: bool,
    },
    /// Turn saving-on-a-pause on or off for a live buffer.
    SetAutosave { request_id: u64, autosave: bool },
    /// Move the caret to a line (and optionally a display column).
    Reveal {
        request_id: u64,
        line: u32,
        column: Option<u32>,
    },
    /// Answer [`EditorMessage::State`] with an uncaptured snapshot.
    GetState { request_id: u64 },
    /// [`EditorMessage::SaveRequest`] reached the disk.
    ///
    /// `revision` is what `fs-service` wrote, and it is what the editor must
    /// present on its next save: the daemon owns the checkout, so the editor
    /// learns the new revision here rather than by stat'ing the file.
    Saved { request_id: u64, revision: String },
    /// [`EditorMessage::SaveRequest`] did not reach the disk.
    ///
    /// The common case is a revision mismatch — an agent wrote the same path —
    /// which is a refusal and not an error: the buffer is intact and the
    /// editor says so rather than overwriting the other writer.
    SaveRefused { request_id: u64, reason: String },
    /// Replace the buffer with `text`, discarding the draft.
    ///
    /// The *take disk* half of a conflict: the daemon re-read the file and the
    /// person chose those bytes. The editor drops its draft and its undo
    /// history is a new document, because there is no edit that gets back to
    /// what was displaced.
    Reload {
        request_id: u64,
        text: String,
        revision: Option<String>,
    },
    /// Which lines the working tree changed, for the gutter.
    ///
    /// Computed by the daemon from `git diff --unified=0`, because the editor
    /// does not open the checkout and must not run git of its own. Sent whole
    /// rather than as a delta: a gutter is small, and a patch that arrives out
    /// of order would point at rows that moved.
    GitMarks {
        request_id: u64,
        marks: Vec<WireMark>,
    },
    /// Ask the editor to send its buffer as a [`EditorMessage::SaveRequest`].
    ///
    /// The *keep mine* half. The daemon cannot write the draft on its own — it
    /// does not have it — so the save stays a request the editor makes.
    Save { request_id: u64 },
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
    /// Write the buffer to the checkout.
    ///
    /// The editor never opens the file itself in integrated mode, so a save is
    /// a request: the whole text travels and the daemon writes it through
    /// `fs-service` at the path *it* opened. `text` is the second message that
    /// can approach the frame cap, which is why the cap is four times the
    /// document budget. The answer is [`DaemonMessage::Saved`] or
    /// [`DaemonMessage::SaveRefused`].
    SaveRequest {
        request_id: u64,
        text: String,
        /// The version `text` was captured at, echoed back so the editor can
        /// tell a confirmed save from one that raced newer keystrokes.
        document_version: u64,
    },
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

/// What one line's gutter mark says happened to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireMarkKind {
    Added,
    Modified,
    /// Something was removed *at* this line; the line itself still exists.
    Deleted,
}

/// One line and its mark, 1-based like git counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMark {
    pub line: u32,
    pub kind: WireMarkKind,
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
