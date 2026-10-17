// Terminal cell metrics, measured from the real font rather than assumed.
//
// A wrong advance shears every box-drawing TUI, so the width comes from the
// font's own `0` advance and the PTY is sized from the same numbers the canvas
// paints with (`CellMetrics`, below).

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

/** The paintable box inside a pane, in pane-relative coordinates. */
export type ContentBox = {
  /** Where the grid's first cell sits inside the pane. */
  x: number;
  y: number;
  width: number;
  height: number;
};

/**
 * The grid's box inside a pane: the rect minus the padding on each axis.
 *
 * The padding is a parameter rather than a `getComputedStyle` read because the
 * caller runs on every `ResizeObserver` callback — every frame of a split drag
 * — and reading style there forces a style recalc each frame. The token and
 * the stylesheet have to agree, which is what the metric exists for.
 */
export function contentBox(
  rect: { width: number; height: number },
  padX: number,
  padY: number,
): ContentBox {
  return {
    x: padX,
    y: padY,
    width: Math.max(0, rect.width - padX * 2),
    height: Math.max(0, rect.height - padY * 2),
  };
}

/**
 * Call `remeasure` once the named face is really resident, and again on any
 * later font load.
 *
 * `document.fonts.ready` resolves when nothing is *pending*, and a preloaded
 * face that has not been requested is not pending — so the first cell can be
 * measured from the fallback, and a wrong advance shears every box-drawing
 * TUI. `load()` asks for the face by name, and `loadingdone` catches a weight
 * that arrives after it. Returns the unsubscribe.
 */
export function whenFontSettles(font: string, remeasure: () => void): () => void {
  const fonts = document.fonts;
  if (!fonts) return () => undefined;
  void fonts.load(font).then(remeasure, () => undefined);
  const onDone = () => remeasure();
  fonts.addEventListener("loadingdone", onDone);
  return () => fonts.removeEventListener("loadingdone", onDone);
}
