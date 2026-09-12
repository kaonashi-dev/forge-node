// Wire color → CSS color.
//
// Port of `apps/tauri indexed_color}`. The 16
// named colors come from the active theme so a reskin carries the terminal with
// it; the 216-color cube and the 24-step grey ramp are the xterm formulas and
// belong to no theme.

import { COLOR_BG, COLOR_FG, COLOR_RGB } from "./types";

export type TerminalPalette = {
  /** Default foreground. */
  fg: string;
  /** Default background, and the backdrop the canvas clears to. */
  bg: string;
  /** Project accent; the hovered-path rule. */
  accent: string;
  /** Caret fill, the text colour. */
  caret: string;
  /** Glyph under an explicit block caret. */
  caretText: string;
  /** The wash behind a dragged selection. */
  selection: string;
  /** The 16 ANSI colors. */
  ansi: string[];
};

/**
 * Resolve one wire color.
 *
 * The two default sentinels are distinct because `INVERSE` swaps the slots and
 * their defaults with them: an inverted default cell paints the background on
 * the foreground, which a single "default" marker could not express.
 */
export function resolveColor(value: number, palette: TerminalPalette): string {
  if (value === COLOR_FG) return palette.fg;
  if (value === COLOR_BG) return palette.bg;
  if (value & COLOR_RGB) return rgbHex(value & 0xffffff);
  return indexed(value, palette);
}

/** Whether a slot is the terminal's own background — nothing to paint. */
export function isDefaultBackground(value: number): boolean {
  return value === COLOR_BG;
}

/**
 * P4: one `#rrggbb` string per colour, not one per run per frame.
 *
 * `resolveColor` builds a string for every indexed and truecolour run it is
 * handed. `docs/performance.md` prices this path per *cell* — ~10 000 a frame,
 * at up to 62fps — and a full-screen TUI in 256 colours is thousands of
 * `toString(16).padStart(6)` calls a frame for a few dozen distinct colours.
 * `font()` was already cached for exactly this reason; the colour was not.
 *
 * The 256 indexed slots are a dense array, filled on first use. Truecolour
 * cannot be — 16.7M possible values — so it gets a bounded map, cleared whole
 * rather than evicted one at a time: a run of output that touches more than
 * the cap is output whose colours are not being reused anyway, and clearing is
 * one operation where an LRU is bookkeeping on the hot path.
 */
const TRUECOLOR_CAP = 512;

export class ColorCache {
  private readonly slots: Array<string | undefined> = new Array(256);
  private readonly truecolor = new Map<number, string>();

  constructor(private palette: TerminalPalette) {}

  /** A theme change moves every colour; the tables are rebuilt, not patched. */
  setPalette(palette: TerminalPalette): void {
    this.palette = palette;
    this.slots.fill(undefined);
    this.truecolor.clear();
  }

  resolve(value: number): string {
    if (value === COLOR_FG) return this.palette.fg;
    if (value === COLOR_BG) return this.palette.bg;
    if (value & COLOR_RGB) {
      const rgb = value & 0xffffff;
      const hit = this.truecolor.get(rgb);
      if (hit !== undefined) return hit;
      if (this.truecolor.size >= TRUECOLOR_CAP) this.truecolor.clear();
      const built = rgbHex(rgb);
      this.truecolor.set(rgb, built);
      return built;
    }
    if (value >= 0 && value <= 255) {
      const hit = this.slots[value];
      if (hit !== undefined) return hit;
      const built = indexed(value, this.palette);
      this.slots[value] = built;
      return built;
    }
    return this.palette.fg;
  }
}

function indexed(index: number, palette: TerminalPalette): string {
  if (index >= 0 && index <= 15) {
    return palette.ansi[index] ?? palette.fg;
  }
  if (index >= 16 && index <= 231) {
    const value = index - 16;
    const channel = (component: number) => (component === 0 ? 0 : 55 + component * 40);
    return rgbHex(
      (channel(Math.floor(value / 36)) << 16) |
        (channel(Math.floor((value % 36) / 6)) << 8) |
        channel(value % 6),
    );
  }
  if (index >= 232 && index <= 255) {
    const level = 8 + (index - 232) * 10;
    return rgbHex((level << 16) | (level << 8) | level);
  }
  return palette.fg;
}

function rgbHex(value: number): string {
  return `#${value.toString(16).padStart(6, "0")}`;
}

/** Read the palette off the CSS variables the theme provider set. */
export function readPalette(root: HTMLElement = document.documentElement): TerminalPalette {
  const style = getComputedStyle(root);
  const read = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
  return {
    fg: read("--forge-term-fg", "#c7c7c7"),
    bg: read("--forge-term-bg", "#1f1f1f"),
    accent: read("--forge-accent", "#6cacbd"),
    caret: read("--forge-term-fg", "#c7c7c7"),
    caretText: read("--forge-term-bg", "#1f1f1f"),
    selection: read("--forge-step", "#404040"),
    ansi: Array.from({ length: 16 }, (_, index) =>
      read(`--forge-ansi-${index}`, index < 8 ? "#1f1f1f" : "#c7c7c7"),
    ),
  };
}
