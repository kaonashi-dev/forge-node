// The rows the canvas currently paints.
//
// This is a buffer of *encoded runs*, not a second `CellGrid`: the sequence
// rules of §10.5 live in `client::CellGrid` on the runtime thread, which is the
// one place they are implemented and tested. What arrives here has already been
// applied there, so the only job left is to keep the last frame around so a
// patch can repaint three rows instead of thirty-two.

import type { CellsPayload, TermModes, WireCursor, WireRow, WireRun } from "./types";
import { DEFAULT_MODES } from "./types";

export type ViewportCell = {
  text: string;
  fg: number;
  bg: number;
  flags: number;
};

const HIDDEN_CURSOR: WireCursor = {
  line: 0,
  col: 0,
  shape: "hidden",
  visible: false,
};

export class Viewport {
  terminal: string | null = null;
  seq = 0;
  cols = 0;
  rows: (WireRow | null)[] = [];
  cursor: WireCursor = HIDDEN_CURSOR;
  modes: TermModes = DEFAULT_MODES;
  scrollOffset = 0;
  scrollbackLen = 0;
  title: string | null = null;

  /**
   * Take one frame and report the row indices that have to repaint.
   *
   * The cursor's own row and the row it left are dirty even when neither
   * changed content, because the cursor is painted over the text rather than
   * encoded into it.
   */
  apply(payload: CellsPayload): number[] {
    const dirty = new Set<number>();
    const previousCursor = this.cursor;
    const resized =
      payload.cols !== this.cols ||
      payload.rows !== this.rows.length ||
      payload.terminal !== this.terminal;

    if (resized) {
      this.rows = new Array<WireRow | null>(payload.rows).fill(null);
    }
    if (payload.full || resized) {
      for (let index = 0; index < payload.rows; index += 1) dirty.add(index);
    }

    this.terminal = payload.terminal;
    this.seq = payload.seq;
    this.cols = payload.cols;
    this.scrollOffset = payload.scroll_offset;
    this.scrollbackLen = payload.scrollback_len;
    this.modes = payload.modes;
    this.cursor = payload.cursor;
    this.title = payload.title;

    for (const [index, row] of payload.patch) {
      if (index < this.rows.length) {
        this.rows[index] = row;
        dirty.add(index);
      }
    }

    if (previousCursor.line !== payload.cursor.line) dirty.add(previousCursor.line);
    dirty.add(payload.cursor.line);
    return [...dirty].filter((index) => index >= 0 && index < this.rows.length);
  }

  /** Forget everything: a fresh attach repaints from nothing. */
  reset(): void {
    this.terminal = null;
    this.rows = [];
    this.cols = 0;
    this.cursor = HIDDEN_CURSOR;
    this.modes = DEFAULT_MODES;
    this.scrollOffset = 0;
    this.scrollbackLen = 0;
    this.title = null;
  }

  /** The cell at a viewport position, or `null` past the end of the row. */
  cellAt(row: number, col: number): ViewportCell | null {
    const line = this.rows[row];
    if (!line) return null;
    let start = 0;
    for (const [text, cols, fg, bg, flags] of line.r) {
      if (col < start + cols) {
        return { text: glyphAt(text, cols, col - start), fg, bg, flags };
      }
      start += cols;
    }
    return null;
  }

  /** One row as a per-column array, which is what a word boundary needs. */
  columns(row: number): string[] {
    const out = new Array<string>(this.cols).fill(" ");
    const line = this.rows[row];
    if (!line) return out;
    let start = 0;
    for (const [text, cols] of line.r) {
      for (let offset = 0; offset < cols && start + offset < out.length; offset += 1) {
        out[start + offset] = glyphAt(text, cols, offset);
      }
      start += cols;
    }
    return out;
  }
}

/**
 * The glyph a run paints in one of its columns.
 *
 * A run of ordinary cells holds one grapheme per column. A run that does not —
 * a wide grapheme, which spans two — paints in its first column and leaves the
 * continuation blank, the way `WIDE_SPACER` does on the wire.
 */
function glyphAt(text: string, cols: number, offset: number): string {
  const graphemes = [...text];
  if (graphemes.length === cols) return graphemes[offset] ?? " ";
  return offset === 0 ? text : "";
}

/** Total columns a row's runs cover, for tests and for the trailing clear. */
export function runWidth(runs: WireRun[]): number {
  return runs.reduce((total, run) => total + run[1], 0);
}
