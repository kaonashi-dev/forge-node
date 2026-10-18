import { describe, expect, it, vi } from "vitest";
import { contentBox, gridSize, whenFontSettles, type CellMetrics } from "./metrics";

const cell: CellMetrics = {
  width: 8,
  height: 18,
  fontSize: 13,
  baseline: 14,
  family: "monospace",
};

describe("gridSize", () => {
  it("floors to whole cells and never drops below one", () => {
    expect(gridSize(80, 180, cell)).toEqual({ cols: 10, rows: 10 });
    expect(gridSize(0, 0, cell)).toEqual({ cols: 1, rows: 1 });
  });
});

describe("contentBox", () => {
  // The padding comes from the theme tokens, and the pane's own `measurePane`
  // subtracts it per axis; a test that only covered square padding would miss
  // the 24/16 terminal the clipping regression was about.
  it("subtracts the padding on each axis and moves the origin by it", () => {
    expect(contentBox({ width: 800, height: 600 }, 24, 16)).toEqual({
      x: 24,
      y: 16,
      width: 752,
      height: 568,
    });
  });

  it("clamps a pane smaller than its own padding to zero, never negative", () => {
    expect(contentBox({ width: 40, height: 20 }, 24, 16)).toEqual({
      x: 24,
      y: 16,
      width: 0,
      height: 0,
    });
  });
});

describe("whenFontSettles", () => {
  /* The font set is a browser API vitest's node environment does not have; only
     the three members the helper touches are stubbed. */
  function stubFonts(): { load: ReturnType<typeof vi.fn>; fire: () => void } {
    const listeners = new Set<() => void>();
    const load = vi.fn(() => Promise.resolve([]));
    vi.stubGlobal("document", {
      fonts: {
        load,
        addEventListener: (_name: string, listener: () => void) => listeners.add(listener),
        removeEventListener: (_name: string, listener: () => void) => listeners.delete(listener),
      },
    });
    return { load, fire: () => listeners.forEach((listener) => listener()) };
  }

  it("asks for the face by name, re-measures, and follows a later load", async () => {
    const fonts = stubFonts();
    try {
      const remeasure = vi.fn();
      const stop = whenFontSettles('13px "Geist Mono"', remeasure);
      expect(fonts.load).toHaveBeenCalledWith('13px "Geist Mono"');
      await Promise.resolve();
      expect(remeasure).toHaveBeenCalledTimes(1);
      fonts.fire();
      expect(remeasure).toHaveBeenCalledTimes(2);
      // Unsubscribed: a later load re-measures nothing.
      stop();
      fonts.fire();
      expect(remeasure).toHaveBeenCalledTimes(2);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("does nothing on a document with no font set", () => {
    vi.stubGlobal("document", {});
    try {
      const stop = whenFontSettles("13px monospace", () => undefined);
      expect(() => stop()).not.toThrow();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
