import type { EditorPlace } from "../../../contracts/terminal";

export function scrollForCaret(
  caret: EditorPlace,
  previous: EditorPlace | undefined,
  scrollTop: number,
  height: number,
  lineHeight: number,
): number | null {
  if (previous?.line === caret.line && previous.column === caret.column) return null;
  return scrollToShow(caret.line, scrollTop, height, lineHeight);
}

/**
 * Lines kept mounted above and below what is visible.
 *
 * Fixed, not a fraction of the file — the point of a window is that a 10 000
 * line buffer costs what a 100 line one does. Must match
 * `editor_control::view::VIEW_OVERSCAN`, which is what the host budgets for.
 */
export const OVERSCAN = 24;

/** Most lines one request may mount; the host clamps to the same number. */
export const MAX_WINDOW = 512;

export type EditorWindow = {
  /** 0-based first line to mount, overscan included. */
  firstLine: number;
  /** How many lines to mount. */
  lineCount: number;
};

/**
 * The window for a scroll position, in lines.
 *
 * `scrollTop` and `height` are CSS pixels; `lineHeight` is what one row
 * measures. A zero or negative line height would divide by nothing, so it is
 * floored at 1 rather than guarded at every call site.
 */
export function windowFor(
  scrollTop: number,
  height: number,
  lineHeight: number,
  totalLines: number,
): EditorWindow {
  const line = Math.max(1, lineHeight);
  const visible = Math.max(1, Math.ceil(height / line));
  const top = Math.max(0, Math.floor(scrollTop / line));
  const firstLine = Math.max(0, top - OVERSCAN);
  const lineCount = Math.min(MAX_WINDOW, visible + 2 * OVERSCAN);
  if (totalLines <= 0) return { firstLine: 0, lineCount };
  // Never past the end: a window anchored beyond the last line would ask the
  // host for rows it would answer with none, and the surface would blank.
  return { firstLine: Math.min(firstLine, Math.max(0, totalLines - 1)), lineCount };
}

/** Whether two windows name the same request, so an idle scroll sends nothing. */
export function sameWindow(a: EditorWindow | null, b: EditorWindow): boolean {
  return a !== null && a.firstLine === b.firstLine && a.lineCount === b.lineCount;
}

/**
 * How far to scroll so `line` is visible, or `null` when it already is.
 *
 * Just enough, never centred: a caret that jumps the viewport to the middle on
 * every arrow key is the thing a person notices.
 */
export function scrollToShow(
  line: number,
  scrollTop: number,
  height: number,
  lineHeight: number,
): number | null {
  const unit = Math.max(1, lineHeight);
  const top = line * unit;
  const bottom = top + unit;
  if (top < scrollTop) return top;
  if (bottom > scrollTop + height) return bottom - height;
  return null;
}
