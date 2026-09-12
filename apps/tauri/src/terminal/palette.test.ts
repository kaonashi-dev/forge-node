import { describe, expect, it } from "vitest";
import { ColorCache, isDefaultBackground, resolveColor, type TerminalPalette } from "./palette";
import { COLOR_BG, COLOR_FG, COLOR_RGB } from "./types";

const palette: TerminalPalette = {
  fg: "#c7c7c7",
  bg: "#1f1f1f",
  accent: "#6cacbd",
  caret: "#c7c7c7",
  caretText: "#1f1f1f",
  selection: "#404040",
  ansi: Array.from({ length: 16 }, (_, index) => `#0000${index.toString(16)}${index.toString(16)}`),
};

describe("terminal palette (terminal.rs port)", () => {
  it("keeps the two defaults apart so INVERSE can swap them", () => {
    expect(resolveColor(COLOR_FG, palette)).toBe(palette.fg);
    expect(resolveColor(COLOR_BG, palette)).toBe(palette.bg);
    expect(isDefaultBackground(COLOR_BG)).toBe(true);
    expect(isDefaultBackground(COLOR_FG)).toBe(false);
  });

  it("takes the 16 named colors from the theme", () => {
    expect(resolveColor(0, palette)).toBe(palette.ansi[0]);
    expect(resolveColor(15, palette)).toBe(palette.ansi[15]);
  });

  // The xterm cube: index 16 is black and index 231 is white, with each
  // channel stepping 0, 95, 135, 175, 215, 255.
  it("walks the 216-color cube by the xterm formula", () => {
    expect(resolveColor(16, palette)).toBe("#000000");
    expect(resolveColor(231, palette)).toBe("#ffffff");
    expect(resolveColor(196, palette)).toBe("#ff0000");
    expect(resolveColor(46, palette)).toBe("#00ff00");
    expect(resolveColor(21, palette)).toBe("#0000ff");
  });

  it("walks the 24-step grey ramp", () => {
    expect(resolveColor(232, palette)).toBe("#080808");
    expect(resolveColor(255, palette)).toBe("#eeeeee");
  });

  it("unpacks a tagged RGB triple", () => {
    expect(resolveColor(COLOR_RGB | 0x010203, palette)).toBe("#010203");
    expect(resolveColor(COLOR_RGB | 0xffffff, palette)).toBe("#ffffff");
  });
});

describe("ColorCache", () => {
  const themed: TerminalPalette = {
    fg: "#c7c7c7",
    bg: "#1f1f1f",
    accent: "#6cacbd",
    caret: "#c7c7c7",
    caretText: "#1f1f1f",
    selection: "#404040",
    ansi: Array.from(
      { length: 16 },
      (_, index) => `#0000${index.toString(16)}${index.toString(16)}`,
    ),
  };

  it("answers exactly what resolveColor answers", () => {
    // The cache is a speed change and nothing else; a divergence here would be
    // a recoloured terminal that no other test would notice.
    const cache = new ColorCache(themed);
    for (const value of [
      COLOR_FG,
      COLOR_BG,
      0,
      7,
      15,
      16,
      100,
      231,
      232,
      255,
      COLOR_RGB | 0x336699,
    ]) {
      expect(cache.resolve(value), String(value)).toBe(resolveColor(value, themed));
    }
  });

  it("returns the same string object on a second hit, which is the point", () => {
    const cache = new ColorCache(themed);
    expect(cache.resolve(200)).toBe(cache.resolve(200));
    const rgb = COLOR_RGB | 0x112233;
    expect(cache.resolve(rgb)).toBe(cache.resolve(rgb));
  });

  it("rebuilds everything when the palette moves", () => {
    const cache = new ColorCache(themed);
    expect(cache.resolve(1)).toBe(themed.ansi[1]);
    cache.setPalette({ ...themed, ansi: themed.ansi.map(() => "#ffffff") });
    expect(cache.resolve(1)).toBe("#ffffff");
  });

  it("survives more truecolour values than it can hold", () => {
    // The cap is cleared whole rather than evicted one at a time; what matters
    // is that the answer stays right on either side of the clear.
    const cache = new ColorCache(themed);
    for (let index = 0; index < 600; index += 1) cache.resolve(COLOR_RGB | index);
    expect(cache.resolve(COLOR_RGB | 0x000001)).toBe("#000001");
  });
});
