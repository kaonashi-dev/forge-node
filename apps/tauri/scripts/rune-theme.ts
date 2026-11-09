// The resolved colours the Rune client paints with, so `tokens.ts` stays the
// one source: Rust reads the result and never re-derives a mix or a contrast lift
// for a built-in base. `variants` pins Rust's port of the custom-theme editor.
import { editorPalette } from "../src/theme/editorTheme";
import { themeVariant, withThemeColor, type ThemeColor } from "../src/theme/themeVariants";
import {
  ACCENT_SWATCHES,
  HUE_COUNT,
  LIGHT_BASES,
  palettes,
  staticTokens,
  THEME_LABELS,
  toCssVariables,
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

// The chrome roles `toCssVariables` derives, by the name Rust reads them under.
const CHROME: Record<string, string> = {
  border: "--border-subtle",
  borderHi: "--border-default",
  borderStrong: "--border-strong",
  hover: "--bg-hover",
  selected: "--bg-selected",
  pressed: "--bg-pressed",
  sidebarHover: "--forge-sidebar-hi",
  textStrong: "--fg-strong",
  fgDefault: "--fg-default",
  fgMuted: "--fg-muted",
  fgSubtle: "--fg-subtle",
  focusRing: "--ring",
  accentText: "--accent-text",
  accentFg: "--accent-fg",
  accentSoft: "--accent-soft",
  accentHover: "--accent-solid-hover",
  dangerText: "--danger-text",
  dangerFg: "--danger-fg",
  dangerSoft: "--danger-soft",
  successText: "--success-text",
  successFg: "--success-fg",
  warningText: "--warning-text",
  warningFg: "--warning-fg",
  infoText: "--info-text",
  infoFg: "--info-fg",
  needsYouText: "--attention-text",
  needsYouFg: "--attention-fg",
  needsYouTint: "--forge-needs-you-tint",
  diffAddedBg: "--forge-diff-added-bg",
  diffRemovedBg: "--forge-diff-removed-bg",
  gitAdded: "--git-added-text",
  gitModified: "--git-modified-text",
  gitDeleted: "--git-deleted-text",
  gitUntracked: "--git-untracked-text",
  gitConflict: "--git-conflict-text",
  gitIgnored: "--git-ignored-text",
  tab: "--finish-tab-bg",
  stripRule: "--finish-strip-rule",
};

function chrome(id: ThemeBaseId) {
  const variables = { ...staticTokens(), ...toCssVariables(palettes[id]) };
  const read = (name: string): string => {
    const value = variables[name] ?? "";
    const forward = /^var\((--[\w-]+)\)$/.exec(value);
    return forward ? read(forward[1]) : value;
  };
  return {
    ...Object.fromEntries(Object.entries(CHROME).map(([role, name]) => [role, read(name)])),
    selectTint: Number.parseInt(read("--finish-select-tint"), 10),
    hueTint: Number.parseInt(read("--hue-tint"), 10),
    hues: Array.from({ length: HUE_COUNT }, (_, slot) => read(`--hue-${slot}`)),
  };
}

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
        chrome: chrome(id),
      },
    ]),
  );
  return `${JSON.stringify({ bases, themes, accents: ACCENT_SWATCHES, variants: variants() }, null, 2)}\n`;
}
