// Geometry of the painted caret. Pure so vitest can import it without a canvas.
//
// The painter lives in `renderer.ts`, which has no tests: Canvas2D is a DOM
// API and the suite runs in `environment: "node"`. Width, snap and shape are
// the part with a right and a wrong answer.

import type { CursorShape } from "./types";

/** A caret one CSS pixel wide, the browser's own, snapped to the device grid. */
export const CARET_CSS_PX = 1;

export type CaretCell = { width: number; height: number };

export type CaretRect = { x: number; y: number; w: number; h: number };

/** Physical-pixel thickness, returned in CSS pixels so the 2D transform hits the grid. */
function thicknessCss(ratio: number): number {
  const scale = ratio || 1;
  return Math.max(1, Math.round(CARET_CSS_PX * scale)) / scale;
}

/** Snap a CSS coordinate onto the device pixel grid. */
function snap(value: number, ratio: number): number {
  const scale = ratio || 1;
  return Math.round(value * scale) / scale;
}

/**
 * Device-pixel geometry for the caret, in CSS pixels.
 *
 * Beam and underline are one physical pixel (two on retina) so they match a
 * browser `<input>` and do not antialias. A block still covers the cell, at
 * the cell's own origin, so the inverted glyph lines up with the row.
 */
export function caretRect(
  col: number,
  row: number,
  wide: boolean,
  cell: CaretCell,
  shape: CursorShape,
  ratio: number,
): CaretRect {
  const y = row * cell.height;
  const cellW = wide ? cell.width * 2 : cell.width;

  if (shape === "beam") {
    const w = thicknessCss(ratio);
    return { x: snap(col * cell.width, ratio), y, w, h: cell.height };
  }

  if (shape === "underline") {
    const t = thicknessCss(ratio);
    return {
      x: snap(col * cell.width, ratio),
      y: y + cell.height - t,
      w: cellW,
      h: t,
    };
  }

  return { x: col * cell.width, y, w: cellW, h: cell.height };
}
