// Terminal cell metrics, measured from the real font rather than assumed.
//
// A wrong advance shears every box-drawing TUI, so the width comes from the
// font's own `0` advance and the PTY is sized from the same numbers the canvas
// paints with (`theme tokens::CellMetrics`).

export type CellMetrics = {
  /** Advance width of one column, unrounded: rounding it shears long runs. */
  width: number;
  /** Height of one row. */
  height: number;
  fontSize: number;
  /** Where a glyph's baseline sits inside the row box. */
  baseline: number;
  family: string;
};

const PROBE = "0".repeat(64);

/**
 * Measure the monospace cell.
 *
 * Averaged over 64 glyphs because a single `measureText("0")` returns the
 * advance rounded to whatever precision the platform reports, and the error
 * multiplies by the column count.
 */
export function measureCell(fontSize: number, family: string, lineHeight: number): CellMetrics {
  const context = document.createElement("canvas").getContext("2d");
  const fallback = {
    width: fontSize * 0.6,
    height: Math.round(fontSize * lineHeight),
    fontSize,
    baseline: Math.round(fontSize * lineHeight * 0.75),
    family,
  };
  if (!context) return fallback;

  context.font = `${fontSize}px ${family}`;
  const measured = context.measureText(PROBE);
  const width = measured.width / PROBE.length;
  if (!Number.isFinite(width) || width <= 0) return fallback;

  const height = Math.round(fontSize * lineHeight);
  const single = context.measureText("Mg");
  const ascent = single.actualBoundingBoxAscent || fontSize * 0.75;
  const descent = single.actualBoundingBoxDescent || fontSize * 0.25;
  return {
    width,
    height,
    fontSize,
    baseline: Math.round((height + ascent - descent) / 2),
    family,
  };
}

/** The PTY geometry a pane of this size can paint. */
export function gridSize(
  paneWidth: number,
  paneHeight: number,
  metrics: CellMetrics,
): { cols: number; rows: number } {
  return {
    cols: Math.max(1, Math.floor(paneWidth / metrics.width)),
    rows: Math.max(1, Math.floor(paneHeight / metrics.height)),
  };
}
