import { describe, expect, it } from "vitest";
import { MAX_WINDOW, OVERSCAN, sameWindow, scrollToShow, windowFor } from "./editorWindow";

describe("windowFor", () => {
  it("mounts what is visible plus an overscan on each side", () => {
    const view = windowFor(0, 400, 20, 10_000);
    expect(view.firstLine).toBe(0);
    expect(view.lineCount).toBe(20 + 2 * OVERSCAN);
  });

  it("anchors the window an overscan above the first visible line", () => {
    const view = windowFor(2000, 400, 20, 10_000);
    expect(view.firstLine).toBe(100 - OVERSCAN);
  });

  /* The cost claim the whole surface rests on: the window for line 9 000 of a
     10 000 line file is the same size as the window for line 0. */
  it("costs the same at the end of a long file as at its start", () => {
    expect(windowFor(0, 400, 20, 10_000).lineCount).toBe(
      windowFor(180_000, 400, 20, 10_000).lineCount,
    );
  });

  it("never anchors past the last line", () => {
    expect(windowFor(999_999, 400, 20, 50).firstLine).toBe(49);
  });

  it("caps the window the host would clamp anyway", () => {
    expect(windowFor(0, 100_000, 1, 1_000_000).lineCount).toBe(MAX_WINDOW);
  });

  /* A line height of zero arrives from a measurement taken before layout; it
     must not divide. */
  it("survives an unmeasured line height", () => {
    expect(() => windowFor(10, 400, 0, 100)).not.toThrow();
    expect(windowFor(10, 400, 0, 100).lineCount).toBeGreaterThan(0);
  });
});

describe("sameWindow", () => {
  it("is false against nothing and true against an identical request", () => {
    const view = { firstLine: 4, lineCount: 60 };
    expect(sameWindow(null, view)).toBe(false);
    expect(sameWindow({ firstLine: 4, lineCount: 60 }, view)).toBe(true);
    expect(sameWindow({ firstLine: 5, lineCount: 60 }, view)).toBe(false);
  });
});

describe("scrollToShow", () => {
  it("says nothing when the line is already on screen", () => {
    expect(scrollToShow(10, 0, 400, 20)).toBeNull();
  });

  it("scrolls just enough, never to the middle", () => {
    expect(scrollToShow(0, 200, 400, 20)).toBe(0);
    expect(scrollToShow(30, 0, 400, 20)).toBe(30 * 20 + 20 - 400);
  });
});
