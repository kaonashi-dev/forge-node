// Canvas2D painter for the viewport.
//
// Repaints the rows a frame actually damaged, which is the whole point of the
// run encoding upstream: a keystroke echo touches one row, and a full-screen
// clear is the only thing that touches thirty-two. Adjacent cells that share a
// style arrived merged, so the shaper kerns inside a run and the element count
// of a full-screen TUI drops by about an order of magnitude.
//
// The cursor and the selection are painted here rather than encoded upstream: a
// drag fires dozens of events per crossed cell, and none of them should reach
// the runtime thread.

import { caretRect } from "./caret";
import type { CellMetrics } from "./metrics";
import { ColorCache, isDefaultBackground, type TerminalPalette } from "./palette";
import { columnsOn, type Selection } from "./selection";
import {
  FLAG_BOLD,
  FLAG_DIM,
  FLAG_ITALIC,
  FLAG_STRIKEOUT,
  FLAG_UNDERLINE,
  FLAG_WIDE_CHAR,
} from "./types";
import type { Viewport } from "./viewport";

/** How much of its color a `DIM` run keeps (same 0.62 factor as the dim wash). */
const DIM_ALPHA = 0.62;
/** Thickness of an underline, a strikeout, and a link rule. */
const RULE = 2;

/**
 * The columns of one row a hovered link covers, `to` inclusive.
 *
 * A reference that soft-wraps is one link across two rows, so it is a list of
 * spans rather than a single range.
 */
export type LinkSpan = {
  row: number;
  from: number;
  to: number;
};

export class TerminalRenderer {
  private context: CanvasRenderingContext2D | null;
  private fonts = new Map<number, string>();
  private width = 0;
  private height = 0;
  private ratio = 1;

  metrics: CellMetrics;
  selection: Selection | null = null;
  focused = false;
  /** The path under the pointer, underlined while a modifier is held. */
  link: LinkSpan[] = [];
  /** Held steady while unfocused or under reduced motion; see `cursorBlink`. */
  cursorVisible = true;

  /** P4: one `#rrggbb` per colour rather than one per run per frame. */
  private colors: ColorCache;
  private current: TerminalPalette;

  constructor(
    private canvas: HTMLCanvasElement,
    metrics: CellMetrics,
    palette: TerminalPalette,
  ) {
    this.context = canvas.getContext("2d", { alpha: false });
    this.metrics = metrics;
    this.current = palette;
    this.colors = new ColorCache(palette);
  }

  get palette(): TerminalPalette {
    return this.current;
  }

  /** A theme change moves every colour, so the cache goes with it. */
  set palette(next: TerminalPalette) {
    this.current = next;
    this.colors.setPalette(next);
  }

  /**
   * Size the backing store for the device pixel ratio.
   *
   * Returns whether anything changed, so a caller can skip a repaint the
   * browser did not need.
   */
  resize(cssWidth: number, cssHeight: number, ratio = window.devicePixelRatio || 1): boolean {
    if (this.width === cssWidth && this.height === cssHeight && this.ratio === ratio) {
      return false;
    }
    this.width = cssWidth;
    this.height = cssHeight;
    this.ratio = ratio;
    this.canvas.width = Math.max(1, Math.round(cssWidth * ratio));
    this.canvas.height = Math.max(1, Math.round(cssHeight * ratio));
    this.canvas.style.width = `${cssWidth}px`;
    this.canvas.style.height = `${cssHeight}px`;
    return true;
  }

  /** Repaint the whole canvas, backdrop included. */
  paintAll(viewport: Viewport): void {
    const context = this.prepare();
    if (!context) return;
    context.fillStyle = this.palette.bg;
    context.fillRect(0, 0, this.width, this.height);
    for (let row = 0; row < viewport.rows.length; row += 1) {
      this.paintRow(context, viewport, row);
    }
  }

  /** Repaint only the rows a frame damaged. */
  paintRows(viewport: Viewport, rows: Iterable<number>): void {
    const context = this.prepare();
    if (!context) return;
    for (const row of rows) {
      if (row >= 0 && row < viewport.rows.length) {
        this.paintRow(context, viewport, row);
      }
    }
  }

  private prepare(): CanvasRenderingContext2D | null {
    const context = this.context;
    if (!context) return null;
    context.setTransform(this.ratio, 0, 0, this.ratio, 0, 0);
    context.textBaseline = "alphabetic";
    return context;
  }

  private paintRow(context: CanvasRenderingContext2D, viewport: Viewport, row: number): void {
    const { width: cellWidth, height: cellHeight } = this.metrics;
    const top = row * cellHeight;

    context.fillStyle = this.palette.bg;
    context.fillRect(0, top, this.width, cellHeight);

    const line = viewport.rows[row];
    if (line) {
      let x = 0;
      for (const [, cols, , bg] of line.r) {
        if (!isDefaultBackground(bg)) {
          context.fillStyle = this.colors.resolve(bg);
          context.fillRect(x, top, cols * cellWidth, cellHeight);
        }
        x += cols * cellWidth;
      }
    }

    // The selection is a wash under the text, not a recolor of it: a selection
    // has to stay readable over syntax-coloured output, and flattening the text
    // to one colour would throw away the thing being selected.
    const selected = this.selection
      ? columnsOn(this.selection, row - viewport.scrollOffset, viewport.cols)
      : null;
    if (selected) {
      context.fillStyle = this.palette.selection;
      context.fillRect(
        selected[0] * cellWidth,
        top,
        (selected[1] - selected[0] + 1) * cellWidth,
        cellHeight,
      );
    }

    if (line) {
      let x = 0;
      for (const [text, cols, fg, , flags] of line.r) {
        this.paintRun(context, text, x, top, cols * cellWidth, fg, flags);
        x += cols * cellWidth;
      }
    }

    this.paintLink(context, row);
    this.paintCursor(context, viewport, row);
  }

  /** The rule under a hovered path, in the project's accent so it reads as live. */
  private paintLink(context: CanvasRenderingContext2D, row: number): void {
    const { width: cellWidth, height: cellHeight } = this.metrics;
    for (const span of this.link) {
      if (span.row !== row) continue;
      context.fillStyle = this.palette.accent;
      context.fillRect(
        span.from * cellWidth,
        row * cellHeight + cellHeight - RULE,
        (span.to - span.from + 1) * cellWidth,
        1,
      );
    }
  }

  private paintRun(
    context: CanvasRenderingContext2D,
    text: string,
    x: number,
    top: number,
    width: number,
    fg: number,
    flags: number,
  ): void {
    const decorated = (flags & (FLAG_UNDERLINE | FLAG_STRIKEOUT)) !== 0;
    // Whitespace with no rule under it paints nothing: the background is
    // already down, and a full-screen TUI is mostly this run.
    if (!decorated && text.trim() === "") return;

    // P5. `save`/`restore` push and pop the whole 2D state — transform, clip,
    // every style — and this runs per run per frame. `globalAlpha` is the only
    // thing that needs restoring, and only a `DIM` run sets it, so the pair is
    // scoped to that case; `fillStyle` and `font` are assigned unconditionally
    // on the next run anyway.
    const dim = (flags & FLAG_DIM) !== 0;
    if (dim) context.globalAlpha = DIM_ALPHA;
    context.fillStyle = this.colors.resolve(fg);
    context.font = this.font(flags);
    context.fillText(text, x, top + this.metrics.baseline);
    if (flags & FLAG_UNDERLINE) {
      context.fillRect(x, top + this.metrics.height - RULE, width, 1);
    }
    if (flags & FLAG_STRIKEOUT) {
      context.fillRect(x, top + Math.round(this.metrics.height / 2), width, 1);
    }
    if (dim) context.globalAlpha = 1;
  }

  /**
   * Paint the cursor over the row it sits on.
   *
   * Nothing is drawn while the viewport is scrolled: the cursor belongs to a
   * line that is no longer on screen, and one painted at its old coordinates
   * would sit on unrelated text.
   */
  private paintCursor(context: CanvasRenderingContext2D, viewport: Viewport, row: number): void {
    const cursor = viewport.cursor;
    if (!this.focused || viewport.scrollOffset !== 0 || !this.cursorVisible) return;
    if (!cursor.visible || cursor.shape === "hidden" || cursor.line !== row) return;

    const cell = viewport.cellAt(row, cursor.col);
    const wide = cell !== null && (cell.flags & FLAG_WIDE_CHAR) !== 0;
    const rect = caretRect(cursor.col, row, wide, this.metrics, cursor.shape, this.ratio);

    context.save();
    context.fillStyle = this.palette.caret;
    context.fillRect(rect.x, rect.y, rect.w, rect.h);
    if (cursor.shape === "block") {
      const text = cell?.text ?? "";
      if (text.trim() !== "") {
        context.fillStyle = this.palette.caretText;
        context.font = this.font(cell?.flags ?? 0);
        context.fillText(text, rect.x, rect.y + this.metrics.baseline);
      }
    }
    context.restore();
  }

  /** The CSS font string for a run's decorations, memoised per flag set. */
  private font(flags: number): string {
    const key = flags & (FLAG_BOLD | FLAG_ITALIC);
    const cached = this.fonts.get(key);
    if (cached) return cached;
    const style = key & FLAG_ITALIC ? "italic " : "";
    const weight = key & FLAG_BOLD ? "700 " : "400 ";
    const font = `${style}${weight}${this.metrics.fontSize}px ${this.metrics.family}`;
    this.fonts.set(key, font);
    return font;
  }

  /** Drop the memoised fonts after a metric or theme change. */
  invalidateFonts(): void {
    this.fonts.clear();
  }
}
