import { describe, expect, it } from "vitest";
import { contrast, mix, parseHex, pct } from "./mix";
import { projectHue, projectHueIndex } from "./projectHue";
import {
  HUE_COUNT,
  hueTint,
  palettes,
  projectHues,
  toCssVariables,
  type ThemeBaseId,
} from "./tokens";

describe("project hue", () => {
  /**
   * Pinned answers, not a property: a change to the hash would recolour every
   * project a person already knows by its colour, so it has to fail loudly.
   */
  it("gives a project the same hue on every launch", () => {
    expect(projectHueIndex("00000000-0000-7000-8000-000000000102")).toBe(
      projectHueIndex("00000000-0000-7000-8000-000000000102"),
    );
    expect(projectHueIndex("")).toBe(0x811c9dc5 % HUE_COUNT);
    expect(projectHueIndex("a")).toBe(0xe40c292c % HUE_COUNT);
    expect(projectHue("a")).toBe(`var(--hue-${0xe40c292c % HUE_COUNT})`);
  });

  it("spreads ids across all nine hues", () => {
    const seen = new Set<number>();
    for (let n = 0; n < 200; n += 1) {
      seen.add(projectHueIndex(`00000000-0000-7000-8000-${String(n).padStart(12, "0")}`));
    }
    expect(seen.size).toBe(HUE_COUNT);
  });

  it.each(Object.keys(palettes) as ThemeBaseId[])(
    "keeps %s's hues readable on their own tint",
    (base) => {
      const palette = palettes[base];
      const hues = projectHues(palette);
      const variables = toCssVariables(palette);
      expect(hues).toHaveLength(HUE_COUNT);
      for (const [index, hue] of hues.entries()) {
        expect(variables[`--hue-${index}`]).toBe(hue);
        for (const ground of [palette.sidebar, palette.bg, palette.strip, palette.surface]) {
          const tint = mix(parseHex(ground), parseHex(hue), pct(hueTint(palette)));
          expect(contrast(parseHex(hue), tint), `${base} hue ${index}`).toBeGreaterThanOrEqual(4.5);
        }
      }
    },
  );

  it("keeps Calm's nine exactly as specified", () => {
    expect(projectHues(palettes["calm-dark"])).toEqual([
      "#5cc8c0",
      "#7aa2f7",
      "#7ed69a",
      "#d98ae0",
      "#f28fad",
      "#b5d86a",
      "#6cc4f0",
      "#e0c36a",
      "#a78bfa",
    ]);
    expect(projectHues(palettes["calm-light"])[8]).toBe("#6d28d9");
  });
});
