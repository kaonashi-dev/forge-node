import { palettes, type Palette } from "./tokens";

export const MAX_THEME_BYTES = 16 * 1024;
export type CustomTheme = { name: string; mode: "light" | "dark"; palette: Palette };

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseCustomTheme(source: string): CustomTheme {
  if (
    source.length > MAX_THEME_BYTES ||
    new TextEncoder().encode(source).length > MAX_THEME_BYTES
  ) {
    throw new Error("Theme JSON must be 16 KB or smaller.");
  }
  let value: unknown;
  try {
    value = JSON.parse(source);
  } catch {
    throw new Error("Invalid JSON. Check commas and quotation marks.");
  }
  if (
    !record(value) ||
    typeof value.name !== "string" ||
    !value.name.trim() ||
    value.name.length > 60
  ) {
    throw new Error("Theme name must contain 1–60 characters.");
  }
  if (value.mode !== "light" && value.mode !== "dark")
    throw new Error('Mode must be "light" or "dark".');
  if (!record(value.palette))
    throw new Error("A palette object is required. Download the template for all fields.");
  const result = { ...palettes["gruvbox-hard"] };
  for (const key of Object.keys(result) as (keyof Palette)[]) {
    const color = value.palette[key];
    if (key === "ansi") {
      if (!Array.isArray(color) || color.length !== 16 || !color.every(isColor))
        throw new Error("palette.ansi must contain exactly 16 #RRGGBB colors.");
      result.ansi = [...color];
    } else {
      if (!isColor(color)) throw new Error(`palette.${key} must be a #RRGGBB color.`);
      result[key] = color;
    }
  }
  return { name: value.name.trim(), mode: value.mode, palette: result };
}

function isColor(value: unknown): value is string {
  return typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value);
}
