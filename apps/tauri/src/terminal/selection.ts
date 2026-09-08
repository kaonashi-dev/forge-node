// Dragged ranges, in the coordinate space the viewport uses.
//
// The line counts down from the top of the *live* grid, so `0` is the first
// visible row and negatives are scrollback: scroll-stable on purpose, because a
// viewport row index would slide a live selection onto whatever text scrolled
// into those rows.

export type CellPoint = { line: number; col: number };

export type Selection = {
  /** Where the drag started. */
  anchor: CellPoint;
  /** Where the pointer is now. */
  head: CellPoint;
};

/** Compare in reading order. */
function before(a: CellPoint, b: CellPoint): boolean {
  return a.line === b.line ? a.col <= b.col : a.line < b.line;
}

/** The two ends in reading order. */
export function ends(selection: Selection): [CellPoint, CellPoint] {
  return before(selection.anchor, selection.head)
    ? [selection.anchor, selection.head]
    : [selection.head, selection.anchor];
}

/**
 * True when the gesture never left the cell it started on — a plain click,
 * which must not leave a one-cell selection behind.
 */
export function isEmpty(selection: Selection): boolean {
  return (
    selection.anchor.line === selection.head.line && selection.anchor.col === selection.head.col
  );
}

/**
 * The columns selected on `line`, or `null` when the line is outside the
 * selection. Both ends are inclusive: dragging across one cell selects it.
 */
export function columnsOn(
  selection: Selection,
  line: number,
  width: number,
): [number, number] | null {
  const [start, end] = ends(selection);
  if (line < start.line || line > end.line) return null;
  const last = Math.max(width - 1, 0);
  const from = line === start.line ? start.col : 0;
  const to = line === end.line ? end.col : last;
  return from <= to ? [from, Math.min(to, last)] : null;
}

/**
 * Characters that end a word for a double-click.
 *
 * Deliberately short. In a terminal a "word" is usually a path, a flag or a
 * URL, so `/`, `.`, `-`, `_`, `:` and `~` are *inside* one — double-clicking
 * `~/dev/forge-node` has to give back the whole thing, which is the only reason
 * anyone double-clicks in a terminal.
 */
const WORD_SEPARATORS = new Set([
  " ",
  "\t",
  '"',
  "'",
  "`",
  "(",
  ")",
  "[",
  "]",
  "{",
  "}",
  "<",
  ">",
  ",",
  ";",
  "|",
  "&",
  "=",
]);

/**
 * The run of word characters around `col`, as an inclusive column range.
 *
 * Returns the cell itself when it *is* a separator: double-clicking a space
 * selects that space rather than silently doing nothing, which is what makes
 * the gesture feel like it landed.
 */
export function wordAt(columns: string[], col: number): [number, number] {
  const isWord = (index: number) => {
    const text = columns[index];
    return text !== undefined && text !== "" && ![...text].every((c) => WORD_SEPARATORS.has(c));
  };
  if (!isWord(col)) return [col, col];
  let from = col;
  while (from > 0 && isWord(from - 1)) from -= 1;
  let to = col;
  while (to + 1 < columns.length && isWord(to + 1)) to += 1;
  return [from, to];
}

/**
 * The cell under a point measured from the top-left of the grid.
 *
 * Clamped rather than optional: a drag that leaves the pane sideways or
 * downwards means "to the end of that line" and "keep going down", which is
 * what every terminal does and what makes selecting a whole screen one gesture
 * instead of a careful one.
 */
export function cellAtPoint(
  x: number,
  y: number,
  cellWidth: number,
  cellHeight: number,
  scrollOffset: number,
  columns: number,
): CellPoint {
  const col = Math.floor(x / cellWidth);
  const row = Math.floor(y / cellHeight);
  return {
    line: row - scrollOffset,
    col: Math.min(Math.max(col, 0), Math.max(columns - 1, 0)),
  };
}
