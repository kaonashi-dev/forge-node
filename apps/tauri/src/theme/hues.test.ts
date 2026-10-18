import { describe, expect, it } from "vitest";
import { contrast, mix, parseHex, pct } from "./mix";
import {
  DEFAULT_HUE,
  HUE_COUNT,
  HUE_NAMES,
  hueTint,
  palettes,
  projectHues,
  toCssVariables,
  type ThemeBaseId,
} from "./tokens";

describe("project hues", () => {
  it.each(Object.keys(palettes) as ThemeBaseId[])(
    "keeps %s's hues readable on their own tint",
    (base) => {
      const palette = palettes[base];
      const hues = projectHues(palette);
      const variables = toCssVariables(palette);
      expect(hues).toHaveLength(HUE_COUNT);
      expect(variables["--hue-default"]).toBe(hues[DEFAULT_HUE]);
      for (const [index, hue] of hues.entries()) {
        expect(variables[`--hue-${index}`]).toBe(hue);
        for (const ground of [palette.sidebar, palette.bg, palette.strip, palette.surface]) {
          const tint = mix(parseHex(ground), parseHex(hue), pct(hueTint(palette)));
          expect(contrast(parseHex(hue), tint), `${base} hue ${index}`).toBeGreaterThanOrEqual(4.5);
        }
      }
    },
  );

  it("names every slot and defaults to violet", () => {
    expect(HUE_NAMES).toHaveLength(HUE_COUNT);
    expect(HUE_NAMES[DEFAULT_HUE]).toBe("Violet");
    expect(projectHues(palettes["calm-dark"])[DEFAULT_HUE]).toBe("#a78bfa");
  });

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
    expect(projectHues(palettes["calm-light"])[DEFAULT_HUE]).toBe("#6d28d9");
  });
});
