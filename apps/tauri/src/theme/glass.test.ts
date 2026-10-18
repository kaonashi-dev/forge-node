import { describe, expect, it } from "vitest";
import { withGlass } from "./glass";
import { contrast, mix, parseHex, pct } from "./mix";
import {
  GLASS_GLOWS,
  GLASS_PANE_OPACITY,
  LIGHT_BASES,
  palettes,
  toCssVariables,
  type ThemeBaseId,
} from "./tokens";

const dark = (Object.keys(palettes) as ThemeBaseId[]).filter((base) => !LIGHT_BASES.has(base));

describe("glass finish", () => {
  it.each(dark)("keeps %s's text readable over the sunken window", (base) => {
    const palette = withGlass(palettes[base]);
    const v = toCssVariables(palette);
    const grounds = [palette.rail, palette.sidebar, palette.bg, palette.strip, palette.surface];
    for (const token of ["--fg-default", "--fg-muted", "--fg-subtle", "--accent-text"]) {
      const minimum = Math.min(
        ...grounds.map((ground) => contrast(parseHex(v[token]), parseHex(ground))),
      );
      expect(minimum, `${base}: ${token}`).toBeGreaterThanOrEqual(4.5);
    }
    expect(contrast(parseHex(v["--accent-fg"]), parseHex(palette.accent))).toBeGreaterThanOrEqual(
      4.5,
    );
  });

  it.each(dark)("keeps %s's sidebar text readable where the glows peak", (base) => {
    const palette = withGlass(palettes[base]);
    const v = toCssVariables(palette);
    const peaks = GLASS_GLOWS.map(({ ansi, share }) =>
      mix(
        mix(parseHex(palette.rail), parseHex(palette.ansi[ansi]), pct(share)),
        parseHex(palette.rail),
        pct(GLASS_PANE_OPACITY),
      ),
    );
    for (const token of ["--fg-default", "--fg-muted", "--fg-subtle"]) {
      const minimum = Math.min(...peaks.map((peak) => contrast(parseHex(v[token]), peak)));
      expect(minimum, `${base}: ${token}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("frosts each base in its own accent and hues", () => {
    const glass = (base: ThemeBaseId) => toCssVariables(withGlass(palettes[base]));
    for (const token of ["--glass-backdrop", "--glass-pane", "--glass-tab"]) {
      expect(glass("gruvbox")[token], token).not.toBe(glass("forge-dark")[token]);
    }
  });

  it("leaves the base's text, accent and terminal colours alone", () => {
    const { text, muted, faint, accent, ansi } = withGlass(palettes["gruvbox"]);
    const base = palettes["gruvbox"];
    expect({ text, muted, faint, accent, ansi }).toEqual({
      text: base.text,
      muted: base.muted,
      faint: base.faint,
      accent: base.accent,
      ansi: base.ansi,
    });
  });
});
