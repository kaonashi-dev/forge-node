// The line under the pointer, as text with a way back to the grid.
//
// Pure so a node test can hold it to the two things that make this awkward: a
// path long enough to soft-wrap is one reference across two rows, and a wide
// glyph is two columns holding one character.

import type { LinkSpan } from "./renderer";
import type { WireRow } from "./types";
import { rowColumns } from "./viewport";

/** One logical line, and where each of its characters was painted. */
export type LineText = {
  text: string;
  /** `rows[i]` and `cols[i]` are the cell `text[i]` came from. */
  rows: number[];
  cols: number[];
};

/**
 * How far a wrapped line is followed either way.
 *
 * One row is enough for the case this exists for — a path straddling the right
 * margin — and the bound is what keeps a screen of wrapped output from being
 * joined into one string on every crossed cell.
 */
const WRAP_REACH = 1;

/** The text around a row, joined across a soft wrap it takes part in. */
export function lineTextAt(
  rows: readonly (WireRow | null)[],
  row: number,
  width: number,
): LineText {
  let first = row;
  if (first - 1 >= 0 && rows[first - 1]?.w === true) first -= WRAP_REACH;
  let last = row;
  if (rows[last]?.w === true && last + 1 < rows.length) last += WRAP_REACH;

  const text: string[] = [];
  const rowOf: number[] = [];
  const colOf: number[] = [];
  for (let index = first; index <= last; index += 1) {
    const columns = rowColumns(rows[index] ?? null, width);
    for (let col = 0; col < columns.length; col += 1) {
      const glyph = columns[col] ?? "";
      for (const character of glyph) {
        text.push(character);
        rowOf.push(index);
        colOf.push(col);
      }
    }
  }
  return { text: text.join(""), rows: rowOf, cols: colOf };
}

/** Where a cell landed in the text, or `-1` for a column that holds nothing. */
export function indexOfCell(line: LineText, row: number, col: number): number {
  for (let index = 0; index < line.rows.length; index += 1) {
    if (line.rows[index] === row && line.cols[index] === col) return index;
  }
  return -1;
}

/**
 * The columns a text range covers, one inclusive range per row it crosses.
 *
 * `to` is exclusive, the way a reference reports itself.
 */
export function spansOfRange(line: LineText, from: number, to: number): LinkSpan[] {
  const spans: LinkSpan[] = [];
  for (let index = from; index < to && index < line.rows.length; index += 1) {
    const row = line.rows[index];
    const col = line.cols[index];
    if (row === undefined || col === undefined) continue;
    const open = spans[spans.length - 1];
    if (open && open.row === row) open.to = Math.max(open.to, col);
    else spans.push({ row, from: col, to: col });
  }
  return spans;
}
