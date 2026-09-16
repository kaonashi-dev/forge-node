//! The windowed view the host publishes, and the input the GUI sends back.
//!
//! The DOM surface is not a cell grid: the GUI mounts one node per visible
//! line and the browser composites the scroll, so what travels here is a
//! *window of lines* — text plus scopes — never a framebuffer. Every count on
//! this wire is clamped by the producer before it reserves anything
//! (`docs/performance.md`), because both ends are reachable from a resize the
//! other one chose.

use serde::{Deserialize, Serialize};

/// Most rows one [`ViewFrame`] may carry.
///
/// A visible window plus two overscans; past this the requester asked for a
/// file rather than a viewport, and the host answers with the cap instead of
/// with the file.
pub const MAX_VIEW_ROWS: usize = 512;

/// Rows the host keeps mounted above and below the visible window.
///
/// Fixed, not a fraction of the file: the point of the window is that a 10 000
/// line buffer costs the same as a 100 line one.
pub const VIEW_OVERSCAN: u32 = 24;

/// Longest row text that travels, in bytes.
///
/// A longer line is sent truncated with [`ViewRow::truncated`] set: a row that
/// does not fit on any screen is not read, and the alternative is one line
/// deciding the size of every frame.
pub const MAX_ROW_BYTES: usize = 8 * 1024;

/// Most spans one row carries.
///
/// Past this the colouring is dropped for that row rather than the row being
/// dropped: text without highlighting still reads.
pub const MAX_ROW_SPANS: usize = 256;

/// Most decorations one frame carries.
pub const MAX_DECORATIONS: usize = 1024;

/// Most carets one frame reports.
///
/// Matches `editor_core::limits::MAX_CURSORS`; `editor-cli` asserts the
/// equality, the way it does for the document budget.
pub const MAX_CARETS: usize = 64;

/// Hard cap for one encoded [`ViewFrame`].
///
/// Well under [`crate::MAX_CONTROL_FRAME`]: a window is a screenful, and a
/// producer that needs four megabytes to describe one is describing a file.
pub const MAX_VIEW_FRAME_BYTES: usize = 512 * 1024;

/// What the rows of one frame may cost before the producer stops adding them.
///
/// The per-row caps do **not** compose — [`MAX_VIEW_ROWS`] rows of
/// [`MAX_ROW_BYTES`] is four megabytes — so the budget is the thing that
/// actually bounds a frame, and the row caps only bound one pathological line
/// inside it. Half of [`MAX_VIEW_FRAME_BYTES`] leaves room for the selection,
/// the decorations and MessagePack's own field names.
pub const VIEW_ROW_BUDGET: usize = MAX_VIEW_FRAME_BYTES / 2;

/// What one span is charged against [`VIEW_ROW_BUDGET`] on top of its text.
///
/// A named-map encoding writes `text` and `scope` for every span, so a row of
/// one-character runs costs far more than its text. Measured against
/// `rmp_serde::to_vec_named`, rounded up.
pub const SPAN_OVERHEAD_BYTES: usize = 32;

// The reason [`VIEW_ROW_BUDGET`] exists, asserted where it cannot drift: the
// row caps alone allow a frame the control channel would refuse, so a producer
// that honoured only them would build one nobody can send.
const _: () = assert!(MAX_VIEW_ROWS * MAX_ROW_BYTES >= crate::MAX_CONTROL_FRAME);
const _: () = assert!(VIEW_ROW_BUDGET < MAX_VIEW_FRAME_BYTES);

/// The colour class a run of text carries.
///
/// Mirrors `editor_core::Scope` without depending on it, like
/// [`crate::EditorStateWire`] mirrors `domain::EditorState`. The GUI maps each
/// one to a theme token; a build that does not know a variant paints it plain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WireScope {
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
///
/// The text is carried, not an offset pair: the GUI builds one DOM node per
/// span and would otherwise have to slice the row itself, and a byte offset
/// into UTF-8 is not a JavaScript string index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireSpan {
    pub text: String,
    #[serde(default)]
    pub scope: WireScope,
}

/// One line of the window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewRow {
    /// 0-based line in the document.
    pub line: u32,
    /// The row's text was longer than [`MAX_ROW_BYTES`] and is cut.
    #[serde(default)]
    pub truncated: bool,
    /// Concatenating the spans reproduces the row's text.
    pub spans: Vec<WireSpan>,
    /// What the working tree did to this line, for the gutter.
    ///
    /// Carried per row rather than as a separate list: the gutter is drawn
    /// with the row, and a mark list that arrived on its own would point at
    /// lines that had already moved.
    #[serde(default)]
    pub mark: Option<crate::WireMarkKind>,
    /// The worst thing a checker said about this line, if it said anything.
    #[serde(default)]
    pub diagnostic: Option<WireSeverityLevel>,
    /// A collapsed block starts here, hiding this many lines.
    ///
    /// `Some(0)` is a foldable block that is open; `None` is a line that
    /// starts no block at all, and the difference is what the arrow shows.
    #[serde(default)]
    pub fold: Option<u32>,
}

/// How much one row's diagnostic matters.
///
/// A level and not the message: a window carries what the gutter paints, and
/// the text of a diagnostic is what a person asks for by putting the caret on
/// the line — it already travels on the editor's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WireSeverityLevel {
    Error,
    Warning,
    Info,
}

/// One caret, in document coordinates.
///
/// Byte offsets never travel: the GUI positions a caret in a DOM row, so it
/// needs the line and the column in *characters* of that row's text, which is
/// what a browser can index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireCaret {
    /// 0-based line.
    pub line: u32,
    /// 0-based, in chars of the line's text.
    pub column: u32,
}

/// One selected range, in the same coordinates as [`WireCaret`].
///
/// Empty ranges are dropped by the producer: a caret is already reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRange {
    pub from: WireCaret,
    pub to: WireCaret,
}

/// What a decoration says about the range it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WireDecorationKind {
    /// A hit for the live query, or another occurrence of the selected word.
    Match,
    /// The hit the caret is on.
    ActiveMatch,
    /// One half of the pair the caret is next to.
    Bracket,
}

/// A decorated range inside the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDecoration {
    pub kind: WireDecorationKind,
    pub range: WireRange,
}

/// The window the host chose, and everything painted in it.
///
/// `doc_version` is `editor_core`'s monotonic counter: the GUI drops a frame
/// older than the one it has, because a coalesced input burst can produce two
/// frames whose order on the socket is not the order they were built in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewFrame {
    pub buffer_id: u64,
    pub doc_version: u64,
    /// 0-based first line of `rows`.
    pub first_line: u32,
    /// Lines in the document, so the scroll container has a height.
    pub total_lines: u32,
    /// `line` is ascending and contiguous except across a fold.
    pub rows: Vec<ViewRow>,
    /// The window stopped short of what was asked for, against
    /// [`VIEW_ROW_BUDGET`]. The GUI shows what arrived rather than blanking
    /// the rest: a shorter window is not an empty one.
    #[serde(default)]
    pub clipped: bool,
    /// Lines hidden inside a fold, and the header that hides them. 0-based.
    #[serde(default)]
    pub folded: Vec<WireFold>,
    pub caret: WireCaret,
    /// Every non-empty selected range, primary first.
    #[serde(default)]
    pub selection: Vec<WireRange>,
    /// Extra carets beyond `caret`, in document order.
    #[serde(default)]
    pub extra_carets: Vec<WireCaret>,
    #[serde(default)]
    pub decorations: Vec<WireDecoration>,
}

/// A collapsed block: the header line, and how many lines it hides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFold {
    /// 0-based header line, which stays visible.
    pub header: u32,
    /// Lines below it that the window does not carry.
    pub hidden: u32,
}

/// Which window the GUI is showing, so the host knows what to build.
///
/// Lines, not pixels: the GUI owns the line height and is the only side that
/// can measure it, so it converts before it asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewRequest {
    /// 0-based first line the GUI wants mounted.
    pub first_line: u32,
    /// How many lines it can show. Clamped to [`MAX_VIEW_ROWS`] by the host.
    pub line_count: u32,
}

/// A key the GUI captured, named rather than encoded as bytes.
///
/// There is no PTY under a DOM surface, so a key is not an escape sequence any
/// more. The set is what a browser's `KeyboardEvent.key` distinguishes; a key
/// this build does not know does nothing rather than being guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WireKey {
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

/// Modifier bits on a [`WireKey`] or a [`EditorInput::Mouse`].
///
/// A bitfield and not four `bool`s: it is the shape every browser and terminal
/// already reports, and four positional booleans is how a caller swaps
/// `shift` for `ctrl`.
pub mod modifiers {
    pub const SHIFT: u8 = 1 << 0;
    pub const CONTROL: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;
    /// Command on macOS, Super elsewhere.
    pub const META: u8 = 1 << 3;
}

/// What a pointer did, in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WireMouseKind {
    Down,
    Drag,
    Up,
}

/// One thing that happened in the GUI's surface.
///
/// Batched into [`crate::DaemonMessage::Input`]: a key repeat or a wheel burst
/// is one message, not one per event, for the same reason the PTY path
/// coalesces a paste into one frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorInput {
    Key {
        key: WireKey,
        modifiers: u8,
    },
    /// Committed text: a paste, or what an IME composition resolved to.
    ///
    /// Clamped by the sender against `MAX_DOCUMENT_BYTES` before it travels;
    /// the host clamps again, because the sender is not the only thing that
    /// can reach this socket.
    Text(String),
    Mouse {
        kind: WireMouseKind,
        /// 0-based line.
        line: u32,
        /// 0-based, in UTF-16 code units — the same unit as [`WireCaret`].
        ///
        /// Every column on this wire is UTF-16, in both directions. A pointer
        /// that reported display cells instead would be a second convention
        /// living next to the first, which is how a caret ends up one cell
        /// left of where it was clicked next to a wide character.
        column: u32,
        modifiers: u8,
    },
    /// Whole lines, positive downward. The GUI scrolls its own container; this
    /// is only for a wheel the host must answer with a new window.
    Wheel {
        lines: i32,
    },
    Focus(bool),
}

/// Most input events one [`crate::DaemonMessage::Input`] may carry.
///
/// A held key repeats at the OS rate and a paste is one `Text`, so a batch
/// this long is a sender that stopped draining, not a person typing.
pub const MAX_INPUT_EVENTS: usize = 256;

impl WireSpan {
    /// A plain, unscoped run.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            scope: WireScope::Plain,
        }
    }
}

impl ViewRow {
    /// The row's text, reassembled from its spans.
    #[must_use]
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode, DaemonMessage, EditorMessage};

    /// The worst frame a producer honouring [`VIEW_ROW_BUDGET`] can build:
    /// every row at the span cap, every span one character.
    fn worst_case_frame() -> ViewFrame {
        let mut rows = Vec::new();
        let mut spent = 0;
        for line in 0..MAX_VIEW_ROWS {
            let spans: Vec<WireSpan> = (0..MAX_ROW_SPANS)
                .map(|i| WireSpan {
                    text: "x".to_string(),
                    scope: if i % 2 == 0 {
                        WireScope::Keyword
                    } else {
                        WireScope::String
                    },
                })
                .collect();
            let cost = spans.len() + spans.len() * SPAN_OVERHEAD_BYTES;
            if !rows.is_empty() && spent + cost > VIEW_ROW_BUDGET {
                break;
            }
            spent += cost;
            rows.push(ViewRow {
                line: line as u32,
                truncated: false,
                spans,
                mark: None,
                diagnostic: None,
                fold: None,
            });
        }
        ViewFrame {
            buffer_id: 7,
            doc_version: 42,
            first_line: 0,
            total_lines: 100_000,
            clipped: true,
            rows,
            folded: vec![WireFold {
                header: 3,
                hidden: 12,
            }],
            caret: WireCaret { line: 1, column: 4 },
            selection: vec![WireRange {
                from: WireCaret { line: 1, column: 0 },
                to: WireCaret { line: 2, column: 9 },
            }],
            extra_carets: vec![WireCaret { line: 5, column: 2 }],
            decorations: (0..MAX_DECORATIONS)
                .map(|i| WireDecoration {
                    kind: WireDecorationKind::Match,
                    range: WireRange {
                        from: WireCaret {
                            line: i as u32,
                            column: 0,
                        },
                        to: WireCaret {
                            line: i as u32,
                            column: 4,
                        },
                    },
                })
                .collect(),
        }
    }

    #[test]
    fn a_frame_built_to_the_budget_fits_the_cap() {
        let frame = worst_case_frame();
        let encoded = encode(&EditorMessage::ViewFrame { frame }).expect("encodes");
        assert!(
            encoded.len() <= MAX_VIEW_FRAME_BYTES,
            "{} bytes is over the {MAX_VIEW_FRAME_BYTES}-byte frame cap",
            encoded.len()
        );
    }

    #[test]
    fn the_view_messages_round_trip() {
        let frame = ViewFrame {
            buffer_id: 1,
            doc_version: 9,
            first_line: 30,
            total_lines: 4000,
            clipped: false,
            rows: vec![ViewRow {
                line: 30,
                truncated: true,
                mark: Some(crate::WireMarkKind::Modified),
                diagnostic: Some(WireSeverityLevel::Warning),
                fold: Some(3),
                spans: vec![
                    WireSpan {
                        text: "fn ".into(),
                        scope: WireScope::Keyword,
                    },
                    WireSpan::plain("main"),
                ],
            }],
            folded: Vec::new(),
            caret: WireCaret {
                line: 30,
                column: 3,
            },
            selection: Vec::new(),
            extra_carets: Vec::new(),
            decorations: Vec::new(),
        };
        assert_eq!(frame.rows[0].text(), "fn main");
        let message = EditorMessage::ViewFrame {
            frame: frame.clone(),
        };
        let bytes = encode(&message).expect("encodes");
        let decoded: EditorMessage = rmp_serde::from_slice(&bytes[4..]).expect("decodes");
        assert_eq!(decoded, message);

        for message in [
            DaemonMessage::Input {
                request_id: Some(4),
                events: vec![
                    EditorInput::Key {
                        key: WireKey::Char('k'),
                        modifiers: modifiers::CONTROL | modifiers::SHIFT,
                    },
                    EditorInput::Text("pasted".into()),
                    EditorInput::Mouse {
                        kind: WireMouseKind::Drag,
                        line: 12,
                        column: 4,
                        modifiers: modifiers::ALT,
                    },
                    EditorInput::Wheel { lines: -3 },
                    EditorInput::Focus(true),
                ],
            },
            DaemonMessage::SetView {
                request_id: 5,
                view: ViewRequest {
                    first_line: 120,
                    line_count: 64,
                },
            },
        ] {
            let bytes = encode(&message).expect("encodes");
            let decoded: DaemonMessage = rmp_serde::from_slice(&bytes[4..]).expect("decodes");
            assert_eq!(decoded, message);
        }
    }

    /// A frame the producer built without a budget is refused by the framing
    /// itself rather than reaching a peer that would allocate for it.
    #[test]
    fn a_frame_past_the_control_cap_never_encodes() {
        let frame = ViewFrame {
            rows: (0..MAX_VIEW_ROWS)
                .map(|line| ViewRow {
                    line: line as u32,
                    truncated: false,
                    spans: vec![WireSpan::plain("x".repeat(MAX_ROW_BYTES))],
                    mark: None,
                    diagnostic: None,
                    fold: None,
                })
                .collect(),
            ..ViewFrame::default()
        };
        assert!(matches!(
            encode(&EditorMessage::ViewFrame { frame }),
            Err(crate::ControlError::FrameTooLarge { .. })
        ));
    }
}
