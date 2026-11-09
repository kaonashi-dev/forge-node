// The resolved colours the Rune client paints with, so `tokens.ts` stays the
// one source: Rust reads the result and never re-derives a mix or a contrast lift.
import { editorPalette } from "../src/theme/editorTheme";
import { LIGHT_BASES, palettes, THEME_LABELS, type ThemeBaseId } from "../src/theme/tokens";

export const RUNE_THEME_PATH = "../ui/src/theme/themes.json";

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
  return `${JSON.stringify({ bases, themes }, null, 2)}\n`;
}
