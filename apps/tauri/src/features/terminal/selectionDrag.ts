import { cellAtPoint, type CellPoint, type Selection } from "../../shared/cell-grid/selection";
import type { Viewport } from "../../shared/cell-grid/viewport";

type Pointer = Pick<MouseEvent, "clientX" | "clientY">;
type Geometry = { left: number; top: number; cellWidth: number; cellHeight: number };
type Options = {
  viewport: Viewport;
  geometry: () => Geometry;
  changed: (previous: Selection | null, next: Selection | null) => void;
  scroll: (lines: number) => Promise<void>;
};

const SCROLL_INTERVAL_MS = 50;
const MAX_SCROLL_LINES = 4;

export class SelectionDrag {
  private current: Selection | null = null;
  private terminal: string | null = null;
  private pointer: Pointer | null = null;
  private moved = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private pending: { offset: number } | null = null;

  constructor(private readonly options: Options) {}

  get range(): Selection | null {
    return this.current;
  }

  get dragging(): boolean {
    return this.pointer !== null;
  }

  syncTerminal(terminal: string | null): void {
    if (terminal === this.terminal) return;
    this.terminal = terminal;
    this.reset();
  }

  set(range: Selection | null): void {
    this.stop();
    this.update(range);
  }

  begin(event: Pointer): void {
    const point = this.point(event);
    this.set({ anchor: point, head: point });
    this.pointer = { clientX: event.clientX, clientY: event.clientY };
  }

  move(event: Pointer): void {
    if (!this.pointer) return;
    if (event.clientX !== this.pointer.clientX || event.clientY !== this.pointer.clientY) {
      this.moved = true;
    }
    this.pointer = { clientX: event.clientX, clientY: event.clientY };
    this.refresh();
  }

  refresh(): void {
    if (!this.pointer || !this.current) return;
    const viewport = this.options.viewport;
    this.update({ anchor: this.current.anchor, head: this.point(this.pointer) });
    if (this.pending && viewport.scrollOffset !== this.pending.offset) this.pending = null;
    if (this.scrollStep() === 0) {
      this.cancelTimer();
      return;
    }
    // Queue acceptance is not a new viewport; wait for the scrolled frame before sending again.
    if (this.timer !== undefined || this.pending) return;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      const lines = this.scrollStep();
      if (lines === 0) return;
      const pending = { offset: viewport.scrollOffset };
      this.pending = pending;
      void this.options.scroll(lines).catch(() => {
        if (this.pending === pending) this.stop();
      });
    }, SCROLL_INTERVAL_MS);
  }

  stop(): void {
    this.pointer = null;
    this.moved = false;
    this.pending = null;
    this.cancelTimer();
  }

  reset(): void {
    this.stop();
    this.update(null);
  }

  private cancelTimer(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
  }

  private update(next: Selection | null): void {
    const previous = this.current;
    if (
      previous === next ||
      (previous &&
        next &&
        previous.anchor.line === next.anchor.line &&
        previous.anchor.col === next.anchor.col &&
        previous.head.line === next.head.line &&
        previous.head.col === next.head.col)
    )
      return;
    this.current = next;
    this.options.changed(previous, next);
  }

  private point(event: Pointer): CellPoint {
    const { left, top, cellWidth, cellHeight } = this.options.geometry();
    const viewport = this.options.viewport;
    const height = viewport.rows.length * cellHeight;
    const y = event.clientY - top;
    const x = y < 0 ? 0 : y >= height ? viewport.cols * cellWidth : event.clientX - left;
    return cellAtPoint(
      x,
      Math.max(0, Math.min(height - 1, y)),
      cellWidth,
      cellHeight,
      viewport.scrollOffset,
      viewport.cols,
    );
  }

  private scrollStep(): number {
    const viewport = this.options.viewport;
    if (
      !this.pointer ||
      !this.moved ||
      viewport.modes.alt_screen ||
      viewport.modes.mouse_mode !== "Off" ||
      viewport.rows.length === 0
    )
      return 0;
    const { top, cellHeight } = this.options.geometry();
    const y = this.pointer.clientY - top;
    const height = viewport.rows.length * cellHeight;
    const edge = Math.min(cellHeight, height / 2);
    const distance = y < edge ? edge - y : y >= height - edge ? height - edge - y : 0;
    if (distance === 0) return 0;
    const lines = Math.min(
      MAX_SCROLL_LINES,
      Math.max(1, Math.ceil(Math.abs(distance) / cellHeight)),
    );
    return distance > 0
      ? Math.min(lines, Math.max(0, viewport.scrollbackLen - viewport.scrollOffset))
      : -Math.min(lines, viewport.scrollOffset);
  }
}
