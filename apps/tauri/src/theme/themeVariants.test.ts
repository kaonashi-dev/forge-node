import { describe, expect, it } from "vitest";
import { parseCustomTheme, type CustomTheme } from "./customTheme";
import { contrast, parseHex } from "./mix";
import { palettes } from "./tokens";
import {
  DEFAULT_CONTRAST,
  normalizeThemeColor,
  themeVariant,
  withThemeColor,
  withThemeContrast,
} from "./themeVariants";

const source: CustomTheme = { name: "Gruvbox", mode: "dark", palette: palettes["gruvbox-hard"] };

describe("theme variations", () => {
  it("accepts complete hex colors and rejects partial or executable values", () => {
    expect(normalizeThemeColor(" #AABBCC ")).toBe("#aabbcc");
    for (const value of ["#abc", "red", "#12345g", "url(test)", "#12345678"])
      expect(normalizeThemeColor(value)).toBeUndefined();
  });

  it("changes surfaces with the background without mutating the built-in or losing ANSI hues", () => {
    const original = structuredClone(source.palette);
    const edited = withThemeColor(source.palette, "bg", "#383838");
    expect(edited.bg).toBe("#383838");
    expect(edited.editor).toBe(edited.bg);
    expect(edited.termBg).toBe(edited.bg);
    expect(edited.surface).toBe("#42403f");
    expect(edited.ansi[0]).toBe(edited.bg);
    expect(edited.ansi.slice(1)).toEqual(original.ansi.slice(1));
    expect(source.palette).toEqual(original);
    for (const value of ["#ffffff", "#000000"])
      expect(withThemeColor(source.palette, "bg", value).surface).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("updates default terminal text and its matching ANSI slot with the foreground", () => {
    const edited = withThemeColor(source.palette, "text", "#D5C4A1");
    expect(edited.text).toBe("#d5c4a1");
    expect(edited.termFg).toBe(edited.text);
    expect(edited.ansi[15]).toBe(edited.text);
    expect(edited.ansi[7]).toBe(source.palette.ansi[7]);
  });

  it.each(["gruvbox-hard", "gruvbox-light"] as const)(
    "scales secondary contrast in %s without moving the chosen colors",
    (base) => {
      const palette = palettes[base];
      const variants = [0, DEFAULT_CONTRAST, 100].map((value) => withThemeContrast(palette, value));
      const ratios = variants.map((p) => contrast(parseHex(p.muted), parseHex(p.bg)));
      expect(ratios[0]).toBeLessThan(ratios[1]);
      expect(ratios[1]).toBeLessThan(ratios[2]);
      for (const variant of variants) {
        expect(variant.bg).toBe(palette.bg);
        expect(variant.text).toBe(palette.text);
        expect(variant.accent).toBe(palette.accent);
        expect(variant.ansi).toEqual(palette.ansi);
      }
    },
  );

  it("round trips adjustments and restores exact colors at 60 after reload", () => {
    const color = withThemeColor(source.palette, "accent", "#83a598");
    const saved = parseCustomTheme(JSON.stringify(themeVariant(source, color, 93)));
    expect(saved.adjustments?.contrast).toBe(93);
    const restored = themeVariant(saved, saved.adjustments!.palette, DEFAULT_CONTRAST);
    expect(restored.palette).toEqual(color);
    expect(restored.name).toBe("Gruvbox");
  });

  it("clamps contrast and derives native appearance from an edited background", () => {
    for (const input of [-1, 120, NaN, 60.3]) {
      expect(() =>
        parseCustomTheme(JSON.stringify(themeVariant(source, source.palette, input))),
      ).not.toThrow();
    }
    expect(themeVariant(source, withThemeColor(source.palette, "bg", "#ffffff"), 60).mode).toBe(
      "light",
    );
  });
});
