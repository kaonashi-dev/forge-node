import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Viewport } from "../../shared/cell-grid/viewport";
import { DEFAULT_MODES } from "../../contracts/terminal";
import { SelectionDrag } from "./selectionDrag";

const pointer = (x: number, y: number) => ({ clientX: x + 8, clientY: y + 8 });

function fixture(scroll = vi.fn<(lines: number) => Promise<void>>().mockResolvedValue(undefined)) {
  const viewport = new Viewport();
  viewport.terminal = "A";
  viewport.cols = 20;
  viewport.rows = new Array(10).fill(null);
  viewport.scrollbackLen = 100;
  viewport.modes = { ...DEFAULT_MODES };
  const changed = vi.fn();
  const drag = new SelectionDrag({
    viewport,
    geometry: () => ({ left: 8, top: 8, cellWidth: 10, cellHeight: 20 }),
    changed,
    scroll,
  });
  drag.syncTerminal("A");
  return { viewport, drag, scroll, changed };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("terminal selection lifecycle", () => {
  it("clears the highlight and the pending drag when another tab's frame arrives", () => {
    const { drag, scroll, changed } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    const previous = drag.range;
    drag.syncTerminal("B");
    expect(drag.range).toBeNull();
    expect(drag.dragging).toBe(false);
    expect(changed).toHaveBeenLastCalledWith(previous, null);
    vi.advanceTimersByTime(1000);
    expect(scroll).not.toHaveBeenCalled();
    drag.syncTerminal("A");
    expect(drag.range).toBeNull();
  });

  it("keeps the selection across ordinary frames from the same terminal", () => {
    const { drag } = fixture();
    const range = { anchor: { line: 2, col: 3 }, head: { line: 8, col: 5 } };
    drag.set(range);
    drag.syncTerminal("A");
    expect(drag.range).toBe(range);
  });

  it("clears selection and timers when the pane hides or its connection changes", () => {
    const { drag, scroll } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    drag.reset();
    drag.move(pointer(40, -100));
    vi.advanceTimersByTime(1000);
    expect(drag.range).toBeNull();
    expect(drag.dragging).toBe(false);
    expect(scroll).not.toHaveBeenCalled();
  });

  it("stops on release or blur while retaining the range for copying", () => {
    const { drag, scroll, viewport } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    const range = drag.range;
    drag.stop();
    viewport.scrollOffset = 5;
    drag.refresh();
    vi.advanceTimersByTime(1000);
    expect(drag.range).toBe(range);
    expect(scroll).not.toHaveBeenCalled();
  });
});

describe("selection edge scrolling", () => {
  it("extends into history with a stationary pointer while keeping the anchor", async () => {
    const { drag, scroll, viewport } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    await vi.advanceTimersByTimeAsync(50);
    expect(scroll).toHaveBeenLastCalledWith(1);
    viewport.scrollOffset = 1;
    drag.refresh();
    expect(drag.range).toEqual({ anchor: { line: 9, col: 4 }, head: { line: -1, col: 4 } });
    await vi.advanceTimersByTimeAsync(50);
    expect(scroll).toHaveBeenCalledTimes(2);
    viewport.scrollOffset = 2;
    drag.refresh();
    expect(drag.range?.head).toEqual({ line: -2, col: 4 });
  });

  it("waits for the actual scroll frame instead of filling the command queue", async () => {
    const { drag, scroll } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    await vi.advanceTimersByTimeAsync(1000);
    for (let i = 0; i < 10; i++) {
      drag.refresh();
      drag.move(pointer(40, -i));
    }
    await vi.advanceTimersByTimeAsync(1000);
    expect(scroll).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("clamps offscreen pointers and scrolls in bounded steps", async () => {
    const { drag, scroll, viewport } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, -10000));
    expect(drag.range?.head).toEqual({ line: 0, col: 0 });
    await vi.advanceTimersByTimeAsync(50);
    expect(scroll).toHaveBeenLastCalledWith(4);
    viewport.scrollOffset = 4;
    drag.refresh();
    expect(drag.range?.head).toEqual({ line: -4, col: 0 });
    drag.move(pointer(40, 10000));
    await vi.advanceTimersByTimeAsync(50);
    expect(scroll).toHaveBeenLastCalledWith(-4);
    expect(drag.range?.head).toEqual({ line: 5, col: 19 });
  });

  it("cancels the next step when the pointer returns to the center", () => {
    const { drag, scroll } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    drag.move(pointer(40, 100));
    vi.advanceTimersByTime(1000);
    expect(scroll).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each(["top", "bottom"])("stops scheduling at the %s boundary", async (edge) => {
    const { drag, scroll, viewport } = fixture();
    viewport.scrollOffset = edge === "top" ? 99 : 1;
    drag.begin(pointer(40, 100));
    drag.move(pointer(40, edge === "top" ? -100 : 300));
    await vi.advanceTimersByTimeAsync(50);
    expect(scroll).toHaveBeenLastCalledWith(edge === "top" ? 1 : -1);
    viewport.scrollOffset = edge === "top" ? 100 : 0;
    drag.refresh();
    await vi.advanceTimersByTimeAsync(1000);
    expect(scroll).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not scroll for a click held at the edge without a drag", () => {
    const { drag, scroll } = fixture();
    drag.begin(pointer(40, 1));
    drag.refresh();
    vi.advanceTimersByTime(1000);
    expect(scroll).not.toHaveBeenCalled();
  });

  it.each(["alternate", "mouse", "empty"])("does not autoscroll a %s viewport", (mode) => {
    const { drag, scroll, viewport } = fixture();
    if (mode === "alternate") viewport.modes.alt_screen = true;
    if (mode === "mouse") viewport.modes.mouse_mode = "Normal";
    if (mode === "empty") viewport.scrollbackLen = 0;
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    vi.advanceTimersByTime(1000);
    expect(scroll).not.toHaveBeenCalled();
  });

  it("updates the endpoint when the wheel moves the viewport during a drag", () => {
    const { drag, viewport } = fixture();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 80));
    viewport.scrollOffset = 20;
    drag.refresh();
    expect(drag.range).toEqual({ anchor: { line: 9, col: 4 }, head: { line: -16, col: 4 } });
  });

  it("a late scroll rejection cannot cancel a newer drag", async () => {
    let reject!: (reason: Error) => void;
    const scroll = vi
      .fn<(lines: number) => Promise<void>>()
      .mockImplementationOnce(
        () =>
          new Promise((_resolve, fail) => {
            reject = fail;
          }),
      )
      .mockResolvedValue(undefined);
    const { drag } = fixture(scroll);
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    await vi.advanceTimersByTimeAsync(50);
    drag.reset();
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    await vi.advanceTimersByTimeAsync(50);
    reject(new Error("disconnected"));
    await Promise.resolve();
    expect(drag.dragging).toBe(true);
    expect(scroll).toHaveBeenCalledTimes(2);
  });

  it("stops if the current scroll request fails", async () => {
    const scroll = vi.fn<(lines: number) => Promise<void>>().mockRejectedValue(new Error("full"));
    const { drag } = fixture(scroll);
    drag.begin(pointer(40, 180));
    drag.move(pointer(40, 1));
    await vi.advanceTimersByTimeAsync(1000);
    expect(drag.dragging).toBe(false);
    expect(scroll).toHaveBeenCalledTimes(1);
  });
});
