import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BLINK_MS, CursorBlink } from "./cursorBlink";

describe("CursorBlink", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function blink() {
    const seen: boolean[] = [];
    const it = new CursorBlink((visible) => seen.push(visible));
    return { blink: it, seen };
  }

  it("alternates on the xterm half-period once running", () => {
    const { blink: caret, seen } = blink();
    caret.run(true);
    vi.advanceTimersByTime(BLINK_MS * 2);
    expect(seen).toEqual([false, true]);
    expect(caret.visible).toBe(true);
  });

  it("stays steady until it is told to run", () => {
    const { blink: caret, seen } = blink();
    vi.advanceTimersByTime(BLINK_MS * 4);
    expect(seen).toEqual([]);
    expect(caret.visible).toBe(true);
  });

  // Parking mid-blink is the case that matters: a pane that lost focus while
  // the caret happened to be in its hidden half would otherwise have no caret
  // at all until something else repainted the row.
  it("leaves the caret shown when the phase parks", () => {
    const { blink: caret } = blink();
    caret.run(true);
    vi.advanceTimersByTime(BLINK_MS);
    expect(caret.visible).toBe(false);
    caret.run(false);
    expect(caret.visible).toBe(true);
    vi.advanceTimersByTime(BLINK_MS * 4);
    expect(caret.visible).toBe(true);
  });

  it("restarts the phase shown on activity", () => {
    const { blink: caret } = blink();
    caret.run(true);
    // Most of the way to hidden, then a keystroke lands.
    vi.advanceTimersByTime(BLINK_MS - 30);
    caret.wake();
    vi.advanceTimersByTime(BLINK_MS - 30);
    expect(caret.visible).toBe(true);
    vi.advanceTimersByTime(30);
    expect(caret.visible).toBe(false);
  });

  it("reports only real flips", () => {
    const { blink: caret, seen } = blink();
    caret.run(true);
    caret.wake();
    caret.wake();
    expect(seen).toEqual([]);
  });

  it("drops its interval when disposed", () => {
    const { blink: caret } = blink();
    caret.run(true);
    caret.dispose();
    vi.advanceTimersByTime(BLINK_MS * 4);
    expect(caret.visible).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });
});
