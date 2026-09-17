import type { CustomTheme } from "./customTheme";
import { hex, luminance, mix, parseHex, pct } from "./mix";
import type { Palette } from "./tokens";

export const DEFAULT_CONTRAST = 60;
export type ThemeColor = "accent" | "bg" | "text";

export function normalizeThemeColor(value: string): string | undefined {
  const trimmed = value.trim();
  return /^#[\da-f]{6}$/i.test(trimmed) ? trimmed.toLowerCase() : undefined;
}

export function withThemeColor(source: Palette, key: ThemeColor, color: string): Palette {
  const value = normalizeThemeColor(color);
  if (!value) throw new Error("Use a six-digit hex color, such as #458588.");
  const result = { ...source, ansi: [...source.ansi], [key]: value };
  if (key === "bg") {
    const original = parseHex(source.bg);
    const next = parseHex(value);
    const shift = (color: string) => {
      const from = parseHex(color);
      let changed = 0;
      for (const bits of [16, 8, 0]) {
        const channel =
          ((from >> bits) & 255) + ((next >> bits) & 255) - ((original >> bits) & 255);
        changed |= Math.max(0, Math.min(255, channel)) << bits;
      }
      return hex(changed);
    };
    for (const surface of [
      "rail",
      "sidebar",
      "surface",
      "surfaceHi",
      "editor",
      "termBg",
      "step",
    ] as const) {
      result[surface] = shift(source[surface]);
    }
    result.ansi[0] = result.termBg;
  } else if (key === "text") {
    result.termFg = value;
    for (const index of [7, 15]) {
      if (source.ansi[index].toLowerCase() === source.text.toLowerCase())
        result.ansi[index] = value;
    }
  }
  return result;
}

function contrastLevel(value: number): number {
  return Math.max(0, Math.min(100, Math.round(Number.isFinite(value) ? value : DEFAULT_CONTRAST)));
}

export function withThemeContrast(source: Palette, contrast: number): Palette {
  const level = contrastLevel(contrast);
  const result = { ...source, ansi: [...source.ansi] };
  if (level === DEFAULT_CONTRAST) return result;
  // Recompute from the unadjusted palette so dragging back to 60 restores exact colors.
  const amount =
    level < DEFAULT_CONTRAST
      ? ((DEFAULT_CONTRAST - level) / DEFAULT_CONTRAST) * 35
      : ((level - DEFAULT_CONTRAST) / (100 - DEFAULT_CONTRAST)) * 20;
  const toward = parseHex(level < DEFAULT_CONTRAST ? source.bg : source.text);
  for (const role of [
    "rail",
    "sidebar",
    "surface",
    "surfaceHi",
    "step",
    "muted",
    "faint",
  ] as const) {
    result[role] = hex(mix(parseHex(source[role]), toward, pct(amount)));
  }
  return result;
}

export function themeVariant(source: CustomTheme, palette: Palette, contrast: number): CustomTheme {
  return {
    name: source.name,
    mode: luminance(parseHex(palette.bg)) > 0.5 ? "light" : "dark",
    palette: withThemeContrast(palette, contrast),
    adjustments: { palette, contrast: contrastLevel(contrast) },
  };
}
