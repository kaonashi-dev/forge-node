//! Shared terminal-grid wire types (§11.4).
//!
//! Placement note: the plan sketches these under `terminal-core`. We keep the
//! *data* types here in `domain` so both sides of the wire can name them
//! without `ui` ever depending on `terminal-core` (§17): the daemon's
//! `terminal-core` engine produces them, `protocol` transports them, and
//! `client`/`ui` render them. The `TerminalEngine` *trait* and the
//! `alacritty_terminal` implementation stay in `terminal-core`.

use crate::agent::PtySize;
use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Default number of scrollback lines included in a snapshot tail (§11.4).
pub const DEFAULT_SCROLLBACK_TAIL: usize = 200;

/// A rendered cell color. Named 0–15 colors are folded into `Indexed`.
/// `Default` means the terminal's default foreground or background, depending
/// on which slot of the [`Cell`] it occupies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Per-cell rendering flags packed into a `u16` bitset. Serialized as the raw
/// integer to keep deltas compact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellFlags(pub u16);

impl CellFlags {
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const INVERSE: Self = Self(1 << 3);
    pub const DIM: Self = Self(1 << 4);
    pub const STRIKEOUT: Self = Self(1 << 5);
    pub const HIDDEN: Self = Self(1 << 6);
    /// Leading cell of a wide (double-width) grapheme.
    pub const WIDE_CHAR: Self = Self(1 << 7);
    /// Continuation cell of a wide grapheme; carries no text (§11.4).
    pub const WIDE_SPACER: Self = Self(1 << 8);

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

impl std::ops::BitOr for CellFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// One terminal cell. `text` holds the full grapheme cluster; continuation
/// cells of wide characters carry the `WIDE_SPACER` flag and empty text (§11.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: CompactString,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Serialize for Cell {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            use serde::ser::SerializeStruct;
            let mut cell = serializer.serialize_struct("Cell", 4)?;
            cell.serialize_field("text", &self.text)?;
            cell.serialize_field("fg", &self.fg)?;
            cell.serialize_field("bg", &self.bg)?;
            cell.serialize_field("flags", &self.flags)?;
            cell.end()
        } else {
            // Field names would otherwise repeat for every cell of every frame.
            (&self.text, self.fg, self.bg, self.flags).serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            #[derive(Deserialize)]
            struct Fields {
                text: CompactString,
                fg: Color,
                bg: Color,
                flags: CellFlags,
            }
            let fields = Fields::deserialize(deserializer)?;
            Ok(Self {
                text: fields.text,
                fg: fields.fg,
                bg: fields.bg,
                flags: fields.flags,
            })
        } else {
            let (text, fg, bg, flags) =
                <(CompactString, Color, Color, CellFlags)>::deserialize(deserializer)?;
            Ok(Self {
                text,
                fg,
                bg,
                flags,
            })
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: CompactString::const_new(" "),
            fg: Color::Default,
            bg: Color::Default,
            flags: CellFlags::empty(),
        }
    }
}

/// A row of cells. `wrapped` marks a soft line-wrap into the next row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub cells: Arc<Vec<Cell>>,
    pub wrapped: bool,
}

impl Row {
    #[must_use]
    pub fn blank(cols: u16) -> Self {
        Self {
            cells: Arc::new(vec![Cell::default(); cols as usize]),
            wrapped: false,
        }
    }
}

/// Cursor rendering shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
    Hidden,
}

/// Cursor position within the visible grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// 0-based row within the visible grid.
    pub line: u16,
    /// 0-based column.
    pub col: u16,
    pub shape: CursorShape,
    pub visible: bool,
}

/// Mouse reporting mode (§11.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MouseMode {
    #[default]
    Off,
    /// Mode 1000: button press/release only.
    Normal,
    /// Mode 1002: button events plus motion while pressed.
    ButtonEvent,
    /// Mode 1003: any motion.
    AnyEvent,
}

/// Terminal modes needed by the renderer and input mapping (§11.4, §11.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermModes {
    pub alt_screen: bool,
    pub bracketed_paste: bool,
    pub app_cursor_keys: bool,
    pub app_keypad: bool,
    pub mouse_mode: MouseMode,
    /// SGR mouse encoding (mode 1006).
    pub mouse_sgr: bool,
    pub focus_events: bool,
}

/// Damage reported by the engine since the last poll (§11.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Damage {
    /// The whole visible grid changed (e.g. after resize or alt-screen switch).
    Full,
    /// Only these visible row indices changed.
    Partial(Vec<u16>),
    /// Inclusive column bounds for each damaged visible row.
    Columns(Vec<(u16, u16, u16)>),
}

/// A row fragment; `first` is a zero-based column and `wrapped` describes the row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellPatch {
    pub line: u16,
    pub first: u16,
    pub row: Row,
}

/// A complete snapshot of the grid at a given `seq` (§11.4). Sent on attach and
/// on resync.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub seq: u64,
    pub size: PtySize,
    /// `rows` visible lines.
    pub visible: Vec<Row>,
    /// Last N scrollback lines (N = [`DEFAULT_SCROLLBACK_TAIL`] by default).
    pub scrollback_tail: Vec<Row>,
    pub scrollback_len: u64,
    pub scrollback_generation: u64,
    pub cursor: Cursor,
    pub modes: TermModes,
    pub title: Option<String>,
}

/// The changed rows between two `seq` values (§11.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDelta {
    pub seq: u64,
    /// `(visible row index, contents)` for each damaged row.
    pub rows: Vec<(u16, Row)>,
    pub patches: Vec<CellPatch>,
    pub scrollback_len: u64,
    /// Changed generations invalidate cached history, including when its length is saturated.
    pub scrollback_generation: u64,
    /// How many lines entered scrollback since the previous delta.
    pub scrolled_lines: u32,
    pub cursor: Cursor,
    pub modes: TermModes,
}

impl TerminalDelta {
    /// Visible rows this delta actually carries, whether whole lines or column patches.
    pub fn changed_rows(&self) -> impl Iterator<Item = (u16, &Row)> + '_ {
        self.rows
            .iter()
            .map(|(index, row)| (*index, row))
            .chain(self.patches.iter().map(|patch| (patch.line, &patch.row)))
    }
}

/// A block of scrollback rows returned by `FetchScrollback` (§10.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollbackRows {
    pub generation: u64,
    /// Aligns a history read with the authoritative viewport when output raced the request.
    pub snapshot: Option<Box<TerminalSnapshot>>,
    /// Absolute scrollback line index of the first returned row (0 = oldest).
    pub from_line: i64,
    pub rows: Vec<Row>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every documented attribute must own a distinct bit, otherwise setting
    /// one would silently set another on the wire.
    #[test]
    fn each_attribute_owns_a_distinct_bit() {
        let all = [
            CellFlags::BOLD,
            CellFlags::ITALIC,
            CellFlags::UNDERLINE,
            CellFlags::INVERSE,
            CellFlags::DIM,
            CellFlags::STRIKEOUT,
            CellFlags::HIDDEN,
            CellFlags::WIDE_CHAR,
            CellFlags::WIDE_SPACER,
        ];
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.0.count_ones(), 1, "flag {i} is not a single bit");
            for b in &all[i + 1..] {
                assert_eq!(a.0 & b.0, 0, "flags {a:?} and {b:?} overlap");
            }
        }
    }

    #[test]
    fn contains_tests_every_bit_of_the_queried_set() {
        let styled = CellFlags::BOLD | CellFlags::UNDERLINE;
        assert!(styled.contains(CellFlags::BOLD));
        assert!(styled.contains(CellFlags::UNDERLINE));
        assert!(styled.contains(CellFlags::BOLD | CellFlags::UNDERLINE));
        assert!(!styled.contains(CellFlags::ITALIC));
        // A superset query fails even though one of its bits is present.
        assert!(!styled.contains(CellFlags::BOLD | CellFlags::ITALIC));
        // The empty set is contained in everything, including itself.
        assert!(styled.contains(CellFlags::empty()));
        assert!(CellFlags::empty().contains(CellFlags::empty()));
        assert!(!CellFlags::empty().contains(CellFlags::BOLD));
    }

    #[test]
    fn insert_and_remove_touch_only_the_named_bits() {
        let mut f = CellFlags::empty();
        assert_eq!(f, CellFlags::default());

        f.insert(CellFlags::BOLD);
        f.insert(CellFlags::ITALIC);
        assert_eq!(f, CellFlags::BOLD | CellFlags::ITALIC);

        // Inserting twice is idempotent.
        f.insert(CellFlags::BOLD);
        assert_eq!(f, CellFlags::BOLD | CellFlags::ITALIC);

        f.remove(CellFlags::BOLD);
        assert_eq!(f, CellFlags::ITALIC);

        // Removing a bit that was never set is a no-op.
        f.remove(CellFlags::HIDDEN);
        assert_eq!(f, CellFlags::ITALIC);

        f.remove(CellFlags::ITALIC);
        assert_eq!(f, CellFlags::empty());
    }

    /// The flags travel as a bare integer so a delta row stays compact; a
    /// struct-shaped encoding here would be a silent wire-format change.
    #[test]
    fn flags_serialize_as_the_raw_integer() {
        assert_eq!(serde_json::to_string(&CellFlags::BOLD).unwrap(), "1");
        assert_eq!(
            serde_json::to_string(&CellFlags::WIDE_SPACER).unwrap(),
            "256"
        );
        let combined = CellFlags::BOLD | CellFlags::WIDE_CHAR;
        let json = serde_json::to_string(&combined).unwrap();
        assert_eq!(json, "129");
        assert_eq!(serde_json::from_str::<CellFlags>(&json).unwrap(), combined);
    }

    #[test]
    fn a_default_cell_is_one_space_in_the_terminal_default_colors() {
        let cell = Cell::default();
        assert_eq!(cell.text, " ");
        assert_eq!(cell.fg, Color::Default);
        assert_eq!(cell.bg, Color::Default);
        assert_eq!(cell.flags, CellFlags::empty());
    }

    #[test]
    fn a_blank_row_is_cols_default_cells_and_never_wrapped() {
        let row = Row::blank(3);
        assert_eq!(row.cells.len(), 3);
        assert!(row.cells.iter().all(|c| *c == Cell::default()));
        assert!(!row.wrapped);

        // A zero-column grid is degenerate but must not panic.
        assert!(Row::blank(0).cells.is_empty());
    }

    #[test]
    fn the_wire_defaults_are_the_neutral_ones() {
        assert_eq!(Color::default(), Color::Default);
        assert_eq!(CursorShape::default(), CursorShape::Block);
        assert_eq!(MouseMode::default(), MouseMode::Off);
        assert_eq!(
            TermModes::default(),
            TermModes {
                alt_screen: false,
                bracketed_paste: false,
                app_cursor_keys: false,
                app_keypad: false,
                mouse_mode: MouseMode::Off,
                mouse_sgr: false,
                focus_events: false,
            }
        );
        // A default cursor is at the origin and, per the struct's derive, hidden
        // until the engine says otherwise.
        let cursor = Cursor::default();
        assert_eq!((cursor.line, cursor.col), (0, 0));
        assert!(!cursor.visible);
        assert_eq!(DEFAULT_SCROLLBACK_TAIL, 200);
    }

    #[test]
    fn every_color_shape_round_trips() {
        for color in [Color::Default, Color::Indexed(9), Color::Rgb(1, 2, 3)] {
            let json = serde_json::to_string(&color).unwrap();
            assert_eq!(serde_json::from_str::<Color>(&json).unwrap(), color);
        }
    }

    #[test]
    fn a_snapshot_round_trips_with_its_scrollback_and_modes() {
        let snapshot = TerminalSnapshot {
            scrollback_generation: 0,
            seq: 42,
            size: PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
            visible: vec![Row::blank(80), Row::blank(80)],
            scrollback_tail: vec![Row {
                cells: vec![Cell {
                    text: CompactString::const_new("x"),
                    fg: Color::Indexed(2),
                    bg: Color::Rgb(9, 9, 9),
                    flags: CellFlags::BOLD | CellFlags::UNDERLINE,
                }]
                .into(),
                wrapped: true,
            }],
            scrollback_len: 7,
            cursor: Cursor {
                line: 3,
                col: 4,
                shape: CursorShape::Beam,
                visible: true,
            },
            modes: TermModes {
                alt_screen: true,
                bracketed_paste: true,
                mouse_mode: MouseMode::ButtonEvent,
                mouse_sgr: true,
                ..TermModes::default()
            },
            title: Some("vim".to_owned()),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<TerminalSnapshot>(&json).unwrap(),
            snapshot
        );
    }

    #[test]
    fn a_delta_round_trips_with_its_damaged_rows() {
        let delta = TerminalDelta {
            patches: Vec::new(),
            scrollback_len: 0,
            scrollback_generation: 0,
            seq: 5,
            rows: vec![(0, Row::blank(2)), (7, Row::blank(2))],
            scrolled_lines: 3,
            cursor: Cursor::default(),
            modes: TermModes::default(),
        };
        let json = serde_json::to_string(&delta).unwrap();
        assert_eq!(serde_json::from_str::<TerminalDelta>(&json).unwrap(), delta);
    }

    #[test]
    fn damage_and_scrollback_rows_round_trip() {
        for damage in [
            Damage::Full,
            Damage::Partial(vec![0, 2, 5]),
            Damage::Columns(vec![(1, 0, 3)]),
        ] {
            let json = serde_json::to_string(&damage).unwrap();
            assert_eq!(serde_json::from_str::<Damage>(&json).unwrap(), damage);
        }
        // `from_line` is signed on the wire: callers address scrollback from the
        // oldest line, and a negative origin must survive the trip.
        let block = ScrollbackRows {
            snapshot: None,
            generation: 0,
            from_line: -12,
            rows: vec![Row::blank(1)],
        };
        let json = serde_json::to_string(&block).unwrap();
        assert_eq!(
            serde_json::from_str::<ScrollbackRows>(&json).unwrap(),
            block
        );
    }
}
