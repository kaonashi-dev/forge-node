import type { CellPoint } from "../../shared/cell-grid/selection";
import type { Viewport } from "../../shared/cell-grid/viewport";

type ClickEvent = Pick<
  MouseEvent,
  "button" | "detail" | "clientX" | "clientY" | "altKey" | "ctrlKey" | "metaKey" | "shiftKey"
>;

export type CursorTarget = { terminal_id: string; seq: number; row: number; col: number };

function plainClick(event: ClickEvent): boolean {
  return (
    event.button === 0 &&
    event.detail === 1 &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.shiftKey
  );
}

export class CursorClick {
  private pending: { target: CursorTarget; x: number; y: number } | null = null;

  cancel(): void {
    this.pending = null;
  }

  begin(event: ClickEvent, point: CellPoint, viewport: Viewport): void {
    this.pending = null;
    if (
      !plainClick(event) ||
      !viewport.terminal ||
      viewport.scrollOffset !== 0 ||
      viewport.modes.alt_screen ||
      viewport.modes.mouse_mode !== "Off" ||
      !viewport.cursor.visible ||
      point.line < 0 ||
      point.line >= viewport.rows.length
    )
      return;
    this.pending = {
      target: {
        terminal_id: viewport.terminal,
        seq: viewport.seq,
        row: point.line,
        col: point.col,
      },
      x: event.clientX,
      y: event.clientY,
    };
  }

  move(event: Pick<MouseEvent, "clientX" | "clientY">): void {
    if (
      this.pending &&
      Math.hypot(event.clientX - this.pending.x, event.clientY - this.pending.y) > 4
    ) {
      this.pending = null;
    }
  }

  finish(event: ClickEvent, point: CellPoint, viewport: Viewport): CursorTarget | null {
    this.move(event);
    const target = this.pending?.target;
    this.pending = null;
    return target &&
      plainClick(event) &&
      viewport.terminal === target.terminal_id &&
      viewport.seq === target.seq &&
      viewport.scrollOffset === 0 &&
      point.line === target.row &&
      point.col === target.col
      ? target
      : null;
  }
}
