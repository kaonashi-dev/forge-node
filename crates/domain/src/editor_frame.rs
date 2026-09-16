//! The window a headless editor publishes, as the GUI receives it.
//!
//! The DOM surface mounts one node per visible line, so this is a *window of
//! lines* — text, scopes and ranges — and never a cell grid. It mirrors
//! `editor_control::view` the way [`crate::EditorState`] mirrors
//! `EditorStateWire`: the editor binary carries no Forge types, so the daemon
//! converts at the socket and this is what crosses the client wire.
//!
//! Columns are UTF-16 code units, because the only consumer indexes strings
//! the way a browser does.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The colour class a run of text carries.
///
/// The GUI maps each one to a theme token; an unknown variant paints plain, so
/// a newer daemon never makes an older surface guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorScope {
    #[default]
    Plain,
    Comment,
    Keyword,
    ControlKeyword,
    String,
    Number,
    Type,
    Function,
    Property,
    Constant,
}

/// A run of one row's text that shares a scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSpan {
    pub text: String,
    #[serde(default)]
    pub scope: EditorScope,
}

/// One line of the window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorRow {
    /// 0-based line in the document.
    pub line: u32,
    /// The row was longer than the editor's per-row cap and is cut.
    #[serde(default)]
    pub truncated: bool,
    /// Concatenating the spans reproduces the row's text.
    ///
    /// Behind an `Arc` for the same reason `Row::cells` is: a frame is
    /// broadcast to every client and this is the only part of it that scales
    /// with what is on screen, so a second surface costs a refcount and not a
    /// copy of the window (`docs/performance.md`).
    pub spans: Arc<Vec<EditorSpan>>,
    /// What the working tree did to this line.
    ///
    /// Per row rather than as its own list: the gutter is drawn with the row,
    /// and a mark list arriving on its own would point at lines that moved.
    #[serde(default)]
    pub mark: Option<EditorMark>,
    /// The worst thing a checker said about this line.
    #[serde(default)]
    pub diagnostic: Option<EditorSeverity>,
    /// A foldable block starts here; the number is how many lines it is
    /// currently hiding, so `Some(0)` is open and `None` is not foldable.
    #[serde(default)]
    pub fold: Option<u32>,
}

/// What one line's gutter mark says happened to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorMark {
    Added,
    Modified,
    /// Something was removed *at* this line; the line itself still exists.
    Deleted,
}

/// How much one row's diagnostic matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorSeverity {
    Error,
    Warning,
    Info,
}

/// A position in the document, as a browser would index it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorPlace {
    /// 0-based line.
    pub line: u32,
    /// 0-based, in UTF-16 code units of the line's text.
    pub column: u32,
}

/// A range between two places; `from` precedes `to`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorRange {
    pub from: EditorPlace,
    pub to: EditorPlace,
}

/// What a decoration says about the range it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorDecorationKind {
    Match,
    ActiveMatch,
    Bracket,
}

/// A decorated range inside the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorDecoration {
    pub kind: EditorDecorationKind,
    pub range: EditorRange,
}

/// A collapsed block: the header line, and how many lines it hides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorFold {
    /// 0-based header line, which stays visible.
    pub header: u32,
    /// Lines below it the window does not carry.
    pub hidden: u32,
}

/// The window the editor chose, and everything painted in it.
///
/// Runtime-only like [`crate::TerminalDelta`]: no column, no migration, and
/// never stored on a [`crate::Session`]. A frame older than the one a surface
/// already has is dropped on `doc_version`, because a coalesced input burst
/// can produce two frames whose order on a socket is not the order they were
/// built in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorFrame {
    pub buffer_id: u64,
    pub doc_version: u64,
    /// 0-based first line of `rows`.
    pub first_line: u32,
    /// Lines in the document, so the scroll container has a height.
    pub total_lines: u32,
    /// Ascending, contiguous except across a fold.
    pub rows: Arc<Vec<EditorRow>>,
    /// The window stopped short of what was asked for, against the editor's
    /// byte budget. Fewer rows, not an empty window.
    #[serde(default)]
    pub clipped: bool,
    #[serde(default)]
    pub folded: Vec<EditorFold>,
    pub caret: EditorPlace,
    /// Every non-empty selected range.
    #[serde(default)]
    pub selection: Vec<EditorRange>,
    /// Carets beyond `caret`, in document order.
    #[serde(default)]
    pub extra_carets: Vec<EditorPlace>,
    #[serde(default)]
    pub decorations: Vec<EditorDecoration>,
}

/// A key the surface captured, named rather than encoded as bytes.
///
/// There is no PTY under a DOM surface. The set is what a browser's
/// `KeyboardEvent.key` distinguishes; an unknown key does nothing rather than
/// being guessed at, because a wrong guess is an edit nobody asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorKey {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    /// `F1`..`F12`; a larger number is ignored.
    Function(u8),
}

/// Modifier bits on an [`EditorInputEvent`].
pub mod editor_modifiers {
    pub const SHIFT: u8 = 1 << 0;
    pub const CONTROL: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;
    /// Command on macOS, Super elsewhere.
    pub const META: u8 = 1 << 3;
}

/// What a pointer did, in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorPointer {
    Down,
    Drag,
    Up,
}

/// One thing that happened in the surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorInputEvent {
    Key {
        key: EditorKey,
        #[serde(default)]
        modifiers: u8,
    },
    /// Committed text: a paste, or what an IME composition resolved to.
    Text(String),
    Pointer {
        kind: EditorPointer,
        /// 0-based line.
        line: u32,
        /// 0-based display column within the line.
        column: u32,
        #[serde(default)]
        modifiers: u8,
    },
    /// Whole lines, positive downward.
    Wheel {
        lines: i32,
    },
    Focus(bool),
}

/// Most input events one `SendEditorInput` may carry.
///
/// Must not exceed `editor_control::MAX_INPUT_EVENTS`: the daemon clamps
/// before it forwards, so a client that ignores this gets truncated rather
/// than refused mid-burst.
pub const MAX_EDITOR_INPUT_EVENTS: usize = 256;

/// Longest committed text one [`EditorInputEvent::Text`] may carry.
///
/// A paste larger than this is the document, not an edit; clamped before the
/// allocation on the daemon side, never after.
pub const MAX_EDITOR_TEXT_BYTES: usize = 1024 * 1024;

impl EditorRow {
    /// The row's text, reassembled from its spans.
    #[must_use]
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_as_json() {
        let frame = EditorFrame {
            buffer_id: 2,
            doc_version: 7,
            first_line: 10,
            total_lines: 900,
            rows: Arc::new(vec![EditorRow {
                line: 10,
                truncated: false,
                spans: Arc::new(vec![
                    EditorSpan {
                        text: "let ".into(),
                        scope: EditorScope::Keyword,
                    },
                    EditorSpan {
                        text: "x".into(),
                        scope: EditorScope::Plain,
                    },
                ]),
                mark: Some(EditorMark::Modified),
                diagnostic: Some(EditorSeverity::Warning),
                fold: None,
            }]),
            clipped: false,
            folded: vec![EditorFold {
                header: 11,
                hidden: 4,
            }],
            caret: EditorPlace {
                line: 10,
                column: 4,
            },
            selection: vec![EditorRange::default()],
            extra_carets: Vec::new(),
            decorations: Vec::new(),
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(serde_json::from_str::<EditorFrame>(&json).unwrap(), frame);
        assert_eq!(frame.rows[0].text(), "let x");
    }

    /// Every optional field defaults, so a surface built against an older
    /// daemon still decodes a frame from a newer one.
    #[test]
    fn the_optional_fields_default() {
        let minimal = r#"{"buffer_id":1,"doc_version":1,"first_line":0,"total_lines":1,
            "rows":[],"caret":{"line":0,"column":0}}"#;
        let frame: EditorFrame = serde_json::from_str(minimal).unwrap();
        assert!(!frame.clipped);
        assert!(frame.folded.is_empty());
        assert!(frame.decorations.is_empty());
    }
}
