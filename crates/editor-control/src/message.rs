//! The messages that travel the editor control channel.
//!
//! Directions are named from the daemon's point of view: [`DaemonMessage`] is
//! what the daemon sends, [`EditorMessage`] what the editor sends back. The
//! handshake is the first exchange in each direction (`Hello` then `Welcome`);
//! every request after it carries a `request_id` that its answer echoes.

use crate::view::{EditorInput, ViewFrame, ViewRequest};
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
    /// Change display metadata without replacing the document or its undo history.
    Retarget { path: String },
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
    /// Candidate declarations for a symbol the editor asked about.
    ///
    /// The answer to [`EditorMessage::FindDefinition`]. *Candidates*, not a
    /// resolution: `fs-service` ranks them with a heuristic, so the editor
    /// offers the list rather than jumping somewhere it cannot justify. Capped
    /// by the daemon before it is sent.
    Definitions {
        request_id: u64,
        /// Echoed, because the editor may have moved on.
        symbol: String,
        places: Vec<WirePlace>,
    },
    /// What the changed block at a line replaced.
    ///
    /// The answer to [`EditorMessage::ChangeDetails`]. The gutter says *which*
    /// lines changed; this is the question a person asks about one of them, so
    /// it is answered on demand rather than carried with every mark.
    ChangeDetails {
        request_id: u64,
        /// 1-based line the block starts at, or 0 when there is no block.
        line: u32,
        before: Vec<String>,
        after: Vec<String>,
        /// Either side was longer than the daemon's budget.
        truncated: bool,
    },
    /// What a checker said about the open file.
    ///
    /// The answer to [`EditorMessage::RunDiagnostics`], and also how they are
    /// cleared: an empty list is "it found nothing", which is a result. A
    /// `command: false` means no checker is configured, which is not.
    Diagnostics {
        request_id: u64,
        /// Whether `[editor] diagnostics_command` named one at all.
        command: bool,
        items: Vec<WireDiagnostic>,
    },
    /// What the person did in the GUI's surface.
    ///
    /// The DOM surface has no PTY, so a keystroke is a named key and not an
    /// escape sequence. Batched: one message per input burst, never one per
    /// key. `request_id` is optional because typing is not a request — it is
    /// there for the caller that wants to know an ordered edit landed.
    Input {
        request_id: Option<u64>,
        events: Vec<EditorInput>,
    },
    /// Which lines the GUI is showing.
    ///
    /// The GUI owns the line height and the scroll container, so it is the
    /// only side that can say what fits; the host answers with a
    /// [`EditorMessage::ViewFrame`] for that window plus its overscan.
    SetView { request_id: u64, view: ViewRequest },
    /// Replace byte ranges, refused when the document moved under the caller.
    ///
    /// Reserved for a preview surface; H1's daemon does not send it yet, so an
    /// editor that receives one may refuse it.
    ApplyPreviewEdit {
        request_id: u64,
        expected_document_version: u64,
        edits: Vec<WireEdit>,
    },
    /// Drive the GUI's find panel. The editor keeps owning the query and the
    /// search; the answer is the `find` on its next [`EditorMessage::State`].
    Find { command: WireFindCommand },
}

/// What the GUI's find panel asks of the editor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WireFindCommand {
    /// Search for `pattern` from where the panel opened, as it is typed.
    /// A pattern over [`MAX_FIND_PATTERN_BYTES`] is refused whole.
    Set {
        pattern: String,
        case_sensitive: bool,
        whole_word: bool,
        regex: bool,
    },
    Next,
    Previous,
    /// Close the panel and clear its highlights; the pattern is kept.
    Close,
}

/// The find panel, while it is open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFind {
    /// Bumped by every open gesture, so the GUI can focus its field again.
    pub focus: u32,
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Matches in the buffer, counted up to [`MAX_FIND_COUNT`].
    pub total: u32,
    /// There are more than [`MAX_FIND_COUNT`].
    pub capped: bool,
    /// 1-based match the caret sits on; 0 when no find gesture has placed it.
    pub index: u32,
    /// Why the pattern cannot be searched with (only a regex can be invalid).
    pub error: Option<String>,
}

/// Longest pattern the find panel may send. Matches `editor_core`'s own limit,
/// which `editor-cli` asserts.
pub const MAX_FIND_PATTERN_BYTES: usize = 1024;

/// Most matches the panel counts before it says "more than".
pub const MAX_FIND_COUNT: u32 = 5_000;

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
    /// Where is this symbol declared?
    ///
    /// The daemon owns the checkout, so the editor cannot grep it. `symbol` is
    /// validated as an identifier on both sides before it reaches `git grep`:
    /// a name that arrives from a click must never be able to become a regex
    /// (`SearchKind::Definition`, AGENTS.md).
    FindDefinition { request_id: u64, symbol: String },
    /// Open another file in the workbench, at a line.
    ///
    /// The editor holds one buffer per process and cannot open a second, so
    /// following a definition is a request: the daemon opens the file the way
    /// the GUI would have.
    OpenPath {
        request_id: u64,
        /// Workspace-relative, as [`DaemonMessage::Definitions`] gave it.
        path: String,
        line: u32,
    },
    /// What did the changed block at this line replace?
    ///
    /// The editor does not open the checkout and never runs git, so the diff
    /// that produced its gutter marks is the daemon's to read again.
    ChangeDetails { request_id: u64, line: u32 },
    /// Run the configured checker and say what it found here.
    ///
    /// On demand: a checker that ran by itself would be a subprocess per
    /// keystroke. The editor never spawns anything — the daemon owns the
    /// checkout and every process in it.
    RunDiagnostics { request_id: u64 },
    /// The window the host chose, and everything painted in it.
    ///
    /// The DOM surface's frame: lines and scopes, never cells. Sent after a
    /// mutation, a caret move or a [`DaemonMessage::SetView`], coalesced by
    /// the host's own emit floor the way the PTY path coalesces damage.
    ViewFrame { frame: ViewFrame },
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
    /// 1-based first line on screen. The TUI owns the viewport; this is what
    /// lets the GUI draw a scrollbar thumb without a second copy of the text.
    #[serde(default)]
    pub top_line: u32,
    /// Logical lines the viewport currently shows, at least 1.
    #[serde(default)]
    pub visible_lines: u32,
    /// Lines in the buffer, so a thumb has a denominator.
    #[serde(default)]
    pub total_lines: u32,
    /// The caret's line as text, clamped to [`MAX_CARET_LINE_BYTES`].
    ///
    /// The one piece of document text on this wire, and it is here for the
    /// screen reader: the GUI paints a passive cell grid, so without this the
    /// only way to say what line a person is on would be to read it back out of
    /// the cells the editor just drew.
    #[serde(default)]
    pub caret_line: String,
    /// Bytes the primary caret has selected. A length and not the text: a
    /// selection can be the whole buffer.
    #[serde(default)]
    pub selection_length: u32,
    /// How many carets there are. More than one is state a person can forget
    /// they are in.
    #[serde(default)]
    pub cursor_count: u32,
    /// The editor's transient message, when it has one.
    ///
    /// The same string its status row shows — `alpha: 3/41`, `no match`, a save
    /// refusal. It is here because a person who cannot see the canvas cannot
    /// see that row, and it is the answer to the gesture they just made.
    #[serde(default)]
    pub status: String,
    /// The GUI's find panel, when it is open.
    #[serde(default)]
    pub find: Option<WireFind>,
}

/// Longest caret line that travels on the state.
///
/// A line can be as long as the document; a screen reader announcing one is
/// reading a sentence, not a file. Clamped before the copy, never after.
pub const MAX_CARET_LINE_BYTES: usize = 2 * 1024;

/// How much one diagnostic matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireSeverity {
    Error,
    Warning,
    Info,
}

/// One line a checker had something to say about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDiagnostic {
    /// 1-based, as every checker counts.
    pub line: u32,
    pub severity: WireSeverity,
    pub message: String,
}

/// One place a symbol is declared, or a diagnostic points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WirePlace {
    /// Workspace-relative.
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// The line's text, so a list reads without a second read per row.
    pub text: String,
}

/// Most places one answer carries.
///
/// A list is chosen from, not scrolled: past this the symbol was too common to
/// be a question, and the daemon says so by truncating rather than by sending
/// a thousand rows through a socket sized for a buffer.
pub const MAX_PLACES: usize = 64;

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
