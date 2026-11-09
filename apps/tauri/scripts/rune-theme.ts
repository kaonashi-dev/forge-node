// The resolved colours the Rune client paints with, so `tokens.ts` stays the
// one source: Rust reads the result and never re-derives a mix or a contrast lift
// for a built-in base. `variants` pins Rust's port of the custom-theme editor.
import { editorPalette } from "../src/theme/editorTheme";
import { themeVariant, withThemeColor, type ThemeColor } from "../src/theme/themeVariants";
import {
  ACCENT_SWATCHES,
  LIGHT_BASES,
  palettes,
  THEME_LABELS,
  type ThemeBaseId,
} from "../src/theme/tokens";

export const RUNE_THEME_PATH = "../ui/src/theme/themes.json";

const VARIANT_CASES: {
  base: ThemeBaseId;
  color?: { key: ThemeColor; value: string };
  contrast: number;
}[] = [
  { base: "gruvbox", color: { key: "accent", value: "#458588" }, contrast: 60 },
  { base: "calm-dark", color: { key: "bg", value: "#202830" }, contrast: 60 },
  { base: "calm-light", color: { key: "text", value: "#101820" }, contrast: 60 },
  { base: "calm-dark", color: { key: "text", value: "#ffffff" }, contrast: 80 },
  { base: "forge-dark", contrast: 25 },
  { base: "ocean", contrast: 90 },
];

function variants() {
  return VARIANT_CASES.map(({ base, color, contrast }) => {
    const palette = color ? withThemeColor(palettes[base], color.key, color.value) : palettes[base];
    const source = { name: "Fixture", mode: "dark" as const, palette };
    return {
      base,
      color: color ?? null,
      contrast,
      expected: themeVariant(source, palette, contrast),
    };
  });
}

export function runeThemes(): string {
  const bases = (Object.keys(palettes) as ThemeBaseId[]).filter((id) => id !== "custom");
  const themes = Object.fromEntries(
    bases.map((id) => [
      id,
      {
        label: THEME_LABELS[id],
        light: LIGHT_BASES.has(id),
        palette: palettes[id],
        editor: editorPalette(id),
      },
    ]),
  );
  return `${JSON.stringify({ bases, themes, accents: ACCENT_SWATCHES, variants: variants() }, null, 2)}\n`;
}
