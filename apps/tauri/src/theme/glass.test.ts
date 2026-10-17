import { describe, expect, it } from "vitest";
import { withGlass } from "./glass";
import { contrast, parseHex } from "./mix";
import { LIGHT_BASES, palettes, toCssVariables, type ThemeBaseId } from "./tokens";

const dark = (Object.keys(palettes) as ThemeBaseId[]).filter((base) => !LIGHT_BASES.has(base));

describe("glass finish", () => {
  it.each(dark)("keeps text readable and its greys distinct over %s", (base) => {
    const palette = withGlass(palettes[base]);
    const v = toCssVariables(palette);
    const grounds = [palette.rail, palette.sidebar, palette.bg, palette.strip, palette.surface];
    for (const token of ["--fg-default", "--fg-muted", "--fg-subtle", "--accent-text"]) {
      const minimum = Math.min(
        ...grounds.map((ground) => contrast(parseHex(v[token]), parseHex(ground))),
      );
      expect(minimum, `${base}: ${token}`).toBeGreaterThanOrEqual(4.5);
    }
    expect(contrast(parseHex(v["--fg-muted"]), parseHex(v["--fg-subtle"]))).toBeGreaterThanOrEqual(
      1.2,
    );
    expect(contrast(parseHex(v["--accent-fg"]), parseHex(palette.accent))).toBeGreaterThanOrEqual(
      4.5,
    );
  });

  it("leaves the base's terminal colours alone", () => {
    expect(withGlass(palettes["gruvbox"]).ansi).toEqual(palettes["gruvbox"].ansi);
  });
});
