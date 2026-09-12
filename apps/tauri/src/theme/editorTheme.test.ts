import { describe, expect, it } from "vitest";
import { SCOPE_CONTRAST_FLOOR, editorPalette, liftToFloor } from "./editorTheme";
import { contrast, parseHex } from "./mix";
import { palettes, type ThemeBaseId } from "./tokens";

const BASES = Object.keys(palettes) as ThemeBaseId[];

describe("editorPalette", () => {
  it.each(BASES)("keeps every scope legible on %s's editor ground", (base) => {
    const ground = parseHex(palettes[base].editor);
    const dim = Object.entries(editorPalette(base).scopes)
      .map(([scope, color]) => ({
        scope,
        ratio: Number(contrast(parseHex(color), ground).toFixed(2)),
      }))
      .filter((entry) => entry.ratio < SCOPE_CONTRAST_FLOOR);
    expect(dim).toEqual([]);
  });

  it.each(BASES)(
    "paints %s's editor on the base's own editor ground, not a CM6 default",
    (base) => {
      expect(editorPalette(base).background).toBe(palettes[base].editor);
      expect(editorPalette(base).foreground).toBe(palettes[base].text);
      expect(editorPalette(base).caret).toBe(palettes[base].text);
    },
  );

  it("keeps distinct scopes distinct after lifting", () => {
    // The floor moves colours; a floor that moved them all onto the same grey
    // would pass the contrast test and make the grammar pointless.
    const scopes = editorPalette("neutral").scopes;
    const hues = new Set([scopes.keyword, scopes.string, scopes.number, scopes.function]);
    expect(hues.size).toBe(4);
  });
});

describe("liftToFloor", () => {
  it("leaves a colour that already clears the floor exactly where it was", () => {
    const white = 0xffffff;
    expect(liftToFloor(white, 0x000000, white)).toBe(white);
  });

  it("lifts one that does not, and stops as soon as it clears", () => {
    const ground = 0x1f1f1f;
    const nearlyInvisible = 0x2a2a2a;
    const lifted = liftToFloor(nearlyInvisible, ground, 0xc7c7c7);
    expect(contrast(lifted, ground)).toBeGreaterThanOrEqual(SCOPE_CONTRAST_FLOOR);
    // Not simply snapped to the foreground: it should stop at the first step
    // that clears, so the hue is preserved as far as the floor allows.
    expect(lifted).not.toBe(0xc7c7c7);
  });
});
