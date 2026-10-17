import type { Palette } from "./tokens";

/**
 * The Glass finish over a dark base: cooler greys, a deeper blue-black card
 * and strip, and a blue accent. It is a palette transform rather than a theme
 * of its own, so the chosen base still owns its terminal colours and status
 * hues, and switching it off is applying the base again. The translucent
 * sidebar and the pill tabs are structure, and live in `shell.css` under
 * `[data-glass]`.
 */
export function withGlass(source: Palette): Palette {
  return {
    ...source,
    ansi: [...source.ansi],
    bg: "#121318",
    editor: "#121318",
    termBg: "#121318",
    rail: "#0b0d14",
    sidebar: "#0b0d14",
    strip: "#101116",
    surface: "#1b1d26",
    surfaceHi: "#262935",
    accent: "#8ab4ff",
    muted: "#b4b6c2",
    faint: "#868896",
    fgSubtle: "#9a9caa",
    border: "#23252e",
    finish: { tab: "#1d2233", rule: "#1d1f27", hover: "#1a1c24" },
  };
}
