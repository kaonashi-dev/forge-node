// The terminal wire, as `src-tauri/src/runtime/cells.rs` encodes it.
//
// A frame carries the *painted* form of the rows that changed — adjacent cells
// sharing a style merged into one run — not a copy of the grid. The grid itself
// stays on the runtime thread (ADR-011): this side never parses terminal bytes
// and never keeps a second emulator.

/** `[text, columns, fg, bg, flags]`. */
export type WireRun = [string, number, number, number, number];

export type WireRow = {
  /** Soft line-wrap into the next row. */
  w: boolean;
  /** Empty means a blank row: the backdrop the canvas already cleared to. */
  r: WireRun[];
};

export type CursorShape = "block" | "underline" | "beam" | "hidden";

export type WireCursor = {
  line: number;
  col: number;
  shape: CursorShape;
  visible: boolean;
};

export type MouseMode = "Off" | "Normal" | "ButtonEvent" | "AnyEvent";

export type TermModes = {
  alt_screen: boolean;
  bracketed_paste: boolean;
  app_cursor_keys: boolean;
  app_keypad: boolean;
  mouse_mode: MouseMode;
  mouse_sgr: boolean;
  focus_events: boolean;
};

export type CellsPayload = {
  terminal: string;
  seq: number;
  cols: number;
  rows: number;
  /** `patch` is the whole viewport and replaces what the canvas holds. */
  full: boolean;
  /** `[row index, row]`; a `null` row is history the cache has not fetched. */
  patch: [number, WireRow | null][];
  cursor: WireCursor;
  modes: TermModes;
  scroll_offset: number;
  scrollback_len: number;
  title: string | null;
  bell: boolean;
  /**
   * The newest keystroke id this frame is the echo of, or `0` for none.
   *
   * An id rather than a count because a key that encodes to nothing is never
   * written, and a count would leave the two sides permanently one apart with
   * no way to notice.
   */
  echo_id: number;
};

export const DEFAULT_MODES: TermModes = {
  alt_screen: false,
  bracketed_paste: false,
  app_cursor_keys: false,
  app_keypad: false,
  mouse_mode: "Off",
  mouse_sgr: false,
  focus_events: false,
};

/** Per-cell rendering flags; mirrors `domain::CellFlags`. */
export const FLAG_BOLD = 1 << 0;
export const FLAG_ITALIC = 1 << 1;
export const FLAG_UNDERLINE = 1 << 2;
export const FLAG_INVERSE = 1 << 3;
export const FLAG_DIM = 1 << 4;
export const FLAG_STRIKEOUT = 1 << 5;
export const FLAG_HIDDEN = 1 << 6;
export const FLAG_WIDE_CHAR = 1 << 7;
export const FLAG_WIDE_SPACER = 1 << 8;

/** The terminal's default foreground, as `cells.rs` encodes it. */
export const COLOR_FG = -1;
/** The terminal's default background. */
export const COLOR_BG = -2;
/** Set on a color that carries a literal RGB triple in its low 24 bits. */
export const COLOR_RGB = 1 << 24;

// The DOM editor surface's wire, as `domain::editor_frame` serializes it.
//
// A window of *lines*, never cells: the surface mounts one node per visible
// line and the browser composites the scroll, so nothing here has a grid in
// it. Columns are UTF-16 code units, which is what a JavaScript string indexes.

export type EditorScope =
  | "Plain"
  | "Comment"
  | "Keyword"
  | "ControlKeyword"
  | "String"
  | "Number"
  | "Type"
  | "Function"
  | "Property"
  | "Constant";

export type EditorSpan = {
  text: string;
  scope: EditorScope;
};

/** What the working tree did to a line. */
export type EditorMark = "Added" | "Modified" | "Deleted";

/** How much a row's diagnostic matters. */
export type EditorSeverity = "Error" | "Warning" | "Info";

export type EditorRow = {
  /** 0-based line in the document. */
  line: number;
  /** Longer than the host's per-row cap, and cut. */
  truncated: boolean;
  /** Concatenating the spans reproduces the row's text. */
  spans: EditorSpan[];
  mark: EditorMark | null;
  diagnostic: EditorSeverity | null;
  /**
   * A foldable block starts here and is hiding this many lines: `0` is open,
   * `null` is a line that starts no block at all.
   */
  fold: number | null;
};

export type EditorPlace = {
  /** 0-based line. */
  line: number;
  /** 0-based, in UTF-16 code units of the line's text. */
  column: number;
};

export type EditorRange = {
  from: EditorPlace;
  to: EditorPlace;
};

export type EditorDecorationKind = "Match" | "ActiveMatch" | "Bracket";

export type EditorDecoration = {
  kind: EditorDecorationKind;
  range: EditorRange;
};

export type EditorFold = {
  /** 0-based header line, which stays visible. */
  header: number;
  /** Lines below it the window does not carry. */
  hidden: number;
};

export type EditorFrame = {
  buffer_id: number;
  /** Monotonic; an older frame than the one on screen is dropped. */
  doc_version: number;
  first_line: number;
  total_lines: number;
  rows: EditorRow[];
  /** The window stopped short of what was asked for. Fewer rows, not none. */
  clipped: boolean;
  folded: EditorFold[];
  caret: EditorPlace;
  selection: EditorRange[];
  extra_carets: EditorPlace[];
  decorations: EditorDecoration[];
};

export type EditorFramePayload = {
  session_id: string;
  frame: EditorFrame;
};
