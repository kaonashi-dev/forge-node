// Theme tokens for the Tauri shell.

import { derivedTokens, hex, mix, on, parseHex, pct } from "./mix";

export type ThemeBaseId =
  | "gruvbox-hard"
  | "gruvbox"
  | "neutral"
  | "gruvbox-light"
  | "neutral-light"
  | "ocean"
  | "forest"
  | "custom";

/** Which bases are light, for `prefers-color-scheme` and `color-scheme`. */
export const LIGHT_BASES: ReadonlySet<ThemeBaseId> = new Set<ThemeBaseId>([
  "gruvbox-light",
  "neutral-light",
]);

let customIsLight = false;

export function baseIsLight(base: ThemeBaseId): boolean {
  return base === "custom" ? customIsLight : LIGHT_BASES.has(base);
}

export type Palette = {
  bg: string;
  rail: string;
  sidebar: string;
  surface: string;
  surfaceHi: string;
  editor: string;
  step: string;
  text: string;
  muted: string;
  faint: string;
  accent: string;
  needsYou: string;
  needsYouDeep: string;
  green: string;
  red: string;
  amber: string;
  blue: string;
  termBg: string;
  termFg: string;
  gitAdded: string;
  gitModified: string;
  gitDeleted: string;
  gitUntracked: string;
  gitConflict: string;
  gitIgnored: string;
  ansi: string[];
};

const palette = (values: Omit<Palette, "ansi">, ansi: string[]): Palette => ({ ...values, ansi });

export const palettes = {
  "gruvbox-hard": palette(
    {
      bg: "#1f1f1f",
      rail: "#171717",
      sidebar: "#1a1a1a",
      surface: "#262626",
      surfaceHi: "#303030",
      editor: "#1f1f1f",
      step: "#404040",
      text: "#c7c7c7",
      muted: "#969696",
      faint: "#7d7d7d",
      accent: "#6cacbd",
      needsYou: "#fe8019",
      needsYouDeep: "#d65d0e",
      green: "#98971a",
      red: "#fb4934",
      amber: "#d79921",
      blue: "#83a598",
      termBg: "#1f1f1f",
      termFg: "#c7c7c7",
      gitAdded: "#46bb26",
      gitModified: "#d79921",
      gitDeleted: "#fb4934",
      gitUntracked: "#689d6a",
      gitConflict: "#fb4934",
      gitIgnored: "#928374",
    },
    [
      "#1f1f1f",
      "#cc241d",
      "#98971a",
      "#d79921",
      "#458588",
      "#b16286",
      "#689d6a",
      "#a89984",
      "#928374",
      "#fb4934",
      "#b8bb26",
      "#fabd2f",
      "#83a598",
      "#d3869b",
      "#8ec07c",
      "#c7c7c7",
    ],
  ),
  gruvbox: palette(
    {
      bg: "#282828",
      rail: "#1d2021",
      sidebar: "#232526",
      surface: "#3c3836",
      surfaceHi: "#504945",
      editor: "#282828",
      step: "#665c54",
      text: "#ebdbb2",
      muted: "#bdae93",
      faint: "#928374",
      accent: "#d3869b",
      needsYou: "#fe8019",
      needsYouDeep: "#af3a03",
      green: "#b8bb26",
      red: "#fb4934",
      amber: "#fabd2f",
      blue: "#83a598",
      termBg: "#282828",
      termFg: "#ebdbb2",
      gitAdded: "#b8bb26",
      gitModified: "#fabd2f",
      gitDeleted: "#fb4934",
      gitUntracked: "#8ec07c",
      gitConflict: "#fb4934",
      gitIgnored: "#928374",
    },
    [
      "#282828",
      "#cc241d",
      "#98971a",
      "#d79921",
      "#458588",
      "#b16286",
      "#689d6a",
      "#a89984",
      "#928374",
      "#fb4934",
      "#b8bb26",
      "#fabd2f",
      "#83a598",
      "#d3869b",
      "#8ec07c",
      "#ebdbb2",
    ],
  ),
  neutral: palette(
    {
      bg: "#0a0a0a",
      rail: "#131313",
      sidebar: "#171717",
      surface: "#1c1c1c",
      surfaceHi: "#262626",
      editor: "#0f0f0f",
      step: "#404040",
      text: "#fafafa",
      muted: "#a1a1a1",
      faint: "#8a8a8a",
      accent: "#a78bfa",
      needsYou: "#f97316",
      needsYouDeep: "#ea580c",
      green: "#4ade80",
      red: "#ff6568",
      amber: "#eab308",
      blue: "#3794ff",
      termBg: "#101010",
      termFg: "#e6e6e6",
      gitAdded: "#81b88b",
      gitModified: "#e2c08d",
      gitDeleted: "#c74e39",
      gitUntracked: "#73c991",
      gitConflict: "#e4676b",
      gitIgnored: "#8c8c8c",
    },
    [
      "#1a1b26",
      "#f7768e",
      "#9ece6a",
      "#e0af68",
      "#7aa2f7",
      "#bb9af7",
      "#7dcfff",
      "#a9b1d6",
      "#414868",
      "#ff7a93",
      "#b9f27c",
      "#ff9e64",
      "#7da6ff",
      "#bb9af7",
      "#7dcfff",
      "#c0caf5",
    ],
  ),
  /*
   * The two light bases (§5.4 T1).
   *
   * Built to the same rules as the dark three and checked by the same tests:
   * `ui.test.ts` holds text at 4.5 on every surface and the derived status
   * foregrounds at 4.0 against their fills, in *every* base, so a light palette
   * that reads badly fails rather than ships. What changes is the direction the
   * derived tokens travel — `mix(bg, text, …)` darkens here instead of
   * lightening — and that falls out of the formulas rather than being a second
   * set of them.
   *
   * The colours are gruvbox's own light variant and a neutral grey/violet pair
   * matching the dark `neutral`, so switching bases changes the ground without
   * changing which hue means "danger".
   */
  "gruvbox-light": palette(
    {
      bg: "#fbf1c7",
      rail: "#ebdbb2",
      sidebar: "#f2e5bc",
      surface: "#f9f5d7",
      surfaceHi: "#ebdbb2",
      editor: "#fbf1c7",
      step: "#d5c4a1",
      text: "#3c3836",
      muted: "#5f5750",
      faint: "#7c6f64",
      accent: "#076678",
      needsYou: "#af3a03",
      needsYouDeep: "#8f3000",
      green: "#79740e",
      red: "#9d0006",
      amber: "#8f5a0b",
      blue: "#076678",
      termBg: "#fbf1c7",
      termFg: "#3c3836",
      gitAdded: "#79740e",
      gitModified: "#8f5a0b",
      gitDeleted: "#9d0006",
      gitUntracked: "#427b58",
      gitConflict: "#9d0006",
      gitIgnored: "#7c6f64",
    },
    [
      "#fbf1c7",
      "#cc241d",
      "#98971a",
      "#d79921",
      "#458588",
      "#b16286",
      "#689d6a",
      "#7c6f64",
      "#928374",
      "#9d0006",
      "#79740e",
      "#8f5a0b",
      "#076678",
      "#8f3f71",
      "#427b58",
      "#3c3836",
    ],
  ),
  "neutral-light": palette(
    {
      bg: "#ffffff",
      rail: "#f1f1f1",
      sidebar: "#f7f7f7",
      surface: "#f4f4f4",
      surfaceHi: "#e8e8e8",
      editor: "#ffffff",
      step: "#d4d4d4",
      text: "#171717",
      muted: "#565656",
      faint: "#6e6e6e",
      accent: "#5b21b6",
      needsYou: "#9a3412",
      needsYouDeep: "#7c2d12",
      green: "#166534",
      red: "#b91c1c",
      amber: "#a16207",
      blue: "#1d4ed8",
      termBg: "#ffffff",
      termFg: "#171717",
      gitAdded: "#166534",
      gitModified: "#a16207",
      gitDeleted: "#b91c1c",
      gitUntracked: "#15803d",
      gitConflict: "#b91c1c",
      gitIgnored: "#6e6e6e",
    },
    [
      "#ffffff",
      "#b91c1c",
      "#166534",
      "#a16207",
      "#1d4ed8",
      "#7e22ce",
      "#0e7490",
      "#565656",
      "#6e6e6e",
      "#dc2626",
      "#15803d",
      "#946200",
      "#2563eb",
      "#9333ea",
      "#0e7490",
      "#171717",
    ],
  ),
} as Record<ThemeBaseId, Palette>;

palettes.ocean = {
  ...palettes.neutral,
  bg: "#111b27",
  editor: "#111b27",
  termBg: "#111b27",
  rail: "#0c141e",
  sidebar: "#152130",
  surface: "#1b2939",
  surfaceHi: "#25364a",
  accent: "#7dcfff",
};
palettes.forest = {
  ...palettes["gruvbox-hard"],
  bg: "#17211c",
  editor: "#17211c",
  termBg: "#17211c",
  rail: "#111913",
  sidebar: "#1a261f",
  surface: "#223027",
  surfaceHi: "#2b3a30",
  accent: "#8ec07c",
};

export function registerCustomPalette(value: Palette, light: boolean): void {
  // Keep the replaceable custom slot out of built-in pickers and fixture exports.
  Object.defineProperty(palettes, "custom", { value, writable: true, configurable: true });
  customIsLight = light;
}

export const metrics = {
  titleH: 36,
  statusH: 24,
  tabH: 34,
  sidebarW: 300,
  handleW: 4,
  /*
   * Denser than the ladder the GUI handed over (28 rows, 24–36 controls,
   * 6–26 radii). The reference is Linear and Zed: a 26px list row, controls that
   * start at 20 and top out at 32, and hairline radii — 4/6/8/10 — with one full
   * pill at the top of the scale. `ui.test.ts` only asks that each ladder stays
   * monotone, so the numbers move without a test pinning them to the old shell.
   */
  rowH: 26,
  controlXS: 20,
  controlSM: 24,
  controlMD: 28,
  controlLG: 32,
  radiusXS: 4,
  radiusSM: 6,
  radiusMD: 8,
  radiusLG: 10,
  radiusXL: 999,
  // Prefer a system Nerd Font install for prompt icons; bundled JetBrains Mono is the fallback (docs/theming.md).
  mono: "JetBrainsMono Nerd Font Mono, JetBrains Mono, ui-monospace, SFMono-Regular, Menlo, monospace",
  monoSize: 13,
  monoLineHeight: 1.35,
  /*
   * The chrome's own face.
   *
   * Nothing is bundled for it on purpose: `-apple-system` is SF Pro on macOS
   * and `Segoe UI`/`Inter` cover the other two, which is the face the rest of
   * each platform is already drawing. Mono stays where mono is the content —
   * the grid, the editor, a diff, a path, a branch, a chord.
   */
  sans: '-apple-system, BlinkMacSystemFont, "Segoe UI", Inter, "Noto Sans", "Helvetica Neue", Arial, sans-serif',
  /*
   * Five steps, and the reason there are only five: a scale with twelve sizes
   * in it is not a scale, and this file had twelve. `sm` is the default the
   * chrome is written in; `xs` is metadata; `md` is a body line; `lg`/`xl`
   * are the only two headings a panel is allowed.
   */
  textXS: 11,
  textSM: 12,
  textMD: 13,
  textLG: 15,
  textXL: 18,
} as const;

/**
 * The theme-independent half of the scale: spacing, stacking, elevation and
 * motion. Numbers here are the same in every base, so they are emitted once in
 * a plain `:root` and — unlike the colours — are *not* written inline by
 * `applyTheme`. That is the whole reason the split exists: `applyTheme` writes
 * every variable it is handed as an inline style, and an inline copy of a
 * constant is a second place to change it. These stay in the stylesheet.
 */
export const scale = {
  /**
   * The spacing ladder, named by the pixel value it carries.
   *
   * Ordinal names (`--space-5` for 12px) were the first cut, and they made the
   * ladder unextendable: the shell stylesheet used 10px thirty times and 14px four
   * times, and neither has an ordinal slot that does not renumber every rung
   * above it. Value names take an insertion without touching a single call
   * site, which is what let §5.1's migration finish instead of stalling on a
   * missing rung and putting a raw `px` back.
   *
   * 1 and 3 are here for hairlines and the gaps between chips, not as general
   * spacing. Everything off this ladder is snapped to it — 5→6, 7→8, 9→8,
   * 18→16 — because a scale nobody may round to is a list, not a scale.
   */
  space: [1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32],
  /**
   * The hairline radius, below the four in `metrics`.
   *
   * It lives here and not there because it is new: `metrics` is mirrored into
   * the `--forge-*` layer §5.1 is retiring, and a rung added to a layer being
   * deleted has to be moved again the day it goes.
   */
  radius2XS: 3,
  /**
   * The stacking ladder, documented for years and now a token.
   *
   * Two halves. Below 40 is *within* a pane — a handle over its neighbours, a
   * badge over the canvas, a sticky strip over what scrolls beneath it — and
   * those numbers only compete with their own siblings. From 40 up is the
   * overlay ladder, where a menu, a scrim, a dialog and a tooltip all have to
   * agree, and where a raw integer is how two of them end up equal.
   */
  z: {
    raised: 1,
    over: 2,
    handle: 3,
    strip: 5,
    route: 15,
    notice: 30,
    context: 40,
    scrim: 50,
    dialog: 51,
    popup: 55,
    tooltip: 60,
  },
  /** Elevation, moved out of the stylesheet so a surface reads it as a token. */
  shadow: {
    sm: "0 8px 24px rgb(0 0 0 / 35%)",
    md: "0 12px 32px rgb(0 0 0 / 40%)",
    lg: "0 24px 64px rgb(0 0 0 / 45%)",
  },
  /*
   * One duration pair and two easings. `enter` decelerates into place and
   * `exit` accelerates away, which is the asymmetry that keeps a dialog from
   * looking like it is being pulled off screen at the same speed it arrived.
   */
  motion: {
    fast: "110ms",
    slow: "180ms",
    ease: "cubic-bezier(0.2, 0, 0, 1)",
    enter: "cubic-bezier(0, 0, 0.2, 1)",
    exit: "cubic-bezier(0.4, 0, 1, 1)",
  },
} as const;

/**
 * The semantic layer, generated from the same palettes.
 *
 * Two kinds of token live here. Most are theme-independent *forwards* onto the
 * legacy `--forge-*` names — `--bg-raised: var(--forge-surface)` — so the whole
 * 3.3k-line shell stylesheet kept resolving unchanged while `src/ui/` was rewritten
 * against the semantic names above it. The forwards are what `staticTokens`
 * emits; the aliases are retired the day the last `--forge-*` reader is gone.
 *
 * The derived foregrounds and soft tints are the exception: `--accent-fg` is
 * `on(accent)`, which depends on the base, so it is computed per theme in
 * `toCssVariables` and written inline like every other colour.
 */
const SEMANTIC_FORWARDS: Record<string, string> = {
  "--bg-base": "var(--forge-bg)",
  "--bg-subtle": "var(--forge-sidebar)",
  "--bg-raised": "var(--forge-surface)",
  "--bg-overlay": "var(--forge-surface-hi)",
  "--fg-default": "var(--forge-text)",
  "--fg-muted": "var(--forge-muted)",
  "--fg-subtle": "var(--forge-faint)",
  "--border-subtle": "var(--forge-border)",
  "--border-default": "var(--forge-border-hi)",
  /*
   * The three interaction grounds. They were the gap that kept `ui.css` on the
   * legacy names: a ghost button's hover, a highlighted menu item and the press
   * wash had no semantic spelling, so every rule that needed one reached past
   * the layer for `--forge-hover`.
   */
  "--bg-hover": "var(--forge-hover)",
  "--bg-selected": "var(--forge-selected)",
  "--bg-pressed": "var(--forge-pressed)",
  "--accent-solid": "var(--forge-accent)",
  "--danger-solid": "var(--forge-red)",
  "--success-solid": "var(--forge-green)",
  "--warning-solid": "var(--forge-amber)",
  "--attention-solid": "var(--forge-needs-you)",
  "--attention-soft": "var(--forge-needs-you-tint)",
  "--ring": "var(--forge-focus-ring)",
  "--radius-xs": "var(--forge-radius-xs)",
  "--radius-sm": "var(--forge-radius-sm)",
  "--radius-md": "var(--forge-radius-md)",
  "--radius-lg": "var(--forge-radius-lg)",
  "--radius-full": "var(--forge-radius-xl)",
  "--control-xs": "var(--forge-control-xs)",
  "--control-sm": "var(--forge-control-sm)",
  "--control-md": "var(--forge-control-md)",
  "--control-lg": "var(--forge-control-lg)",
  "--row-h": "var(--forge-row-h)",
  /* The type scale and the two faces, so a rule names one layer, not both. */
  "--text-xs": "var(--forge-text-xs)",
  "--text-sm": "var(--forge-text-sm)",
  "--text-md": "var(--forge-text-md)",
  "--text-lg": "var(--forge-text-lg)",
  "--text-xl": "var(--forge-text-xl)",
  "--font-mono": "var(--forge-mono)",
  "--font-sans": "var(--forge-sans)",
};

/** Per-theme semantic colours that have to be *computed*, not forwarded. */
function semanticColors(active: Palette): Record<string, string> {
  const bg = parseHex(active.bg);
  const text = parseHex(active.text);
  const accent = parseHex(active.accent);
  const fg = (fill: string) => hex(on(parseHex(fill), bg, text));
  const soft = (fill: string) => hex(mix(bg, parseHex(fill), pct(14)));
  return {
    "--accent-fg": fg(active.accent),
    "--accent-soft": hex(mix(bg, accent, pct(14))),
    "--accent-solid-hover": hex(mix(accent, text, pct(12))),
    "--danger-fg": fg(active.red),
    "--danger-soft": soft(active.red),
    "--success-fg": fg(active.green),
    "--warning-fg": fg(active.amber),
    "--attention-fg": fg(active.needsYou),
    "--border-strong": hex(mix(bg, text, pct(25))),
  };
}

/**
 * The tokens that never change with the base: the scale forwards above, the
 * spacing/z/shadow/motion constants, plus `--forge-*` bridges for elevation and
 * motion so any remaining `var(--forge-shadow-md)` keeps resolving
 * now that the literals moved here. Emitted once, in a plain `:root`.
 */
export function staticTokens(): Record<string, string> {
  const out: Record<string, string> = { ...SEMANTIC_FORWARDS };
  for (const value of scale.space) {
    out[`--space-${value}`] = `${value}px`;
  }
  out["--radius-2xs"] = `${scale.radius2XS}px`;
  for (const [name, value] of Object.entries(scale.z)) {
    out[`--z-${name}`] = String(value);
  }
  for (const [name, value] of Object.entries(scale.shadow)) {
    out[`--shadow-${name}`] = value;
    out[`--forge-shadow-${name}`] = `var(--shadow-${name})`;
  }
  out["--motion-fast"] = scale.motion.fast;
  out["--motion-slow"] = scale.motion.slow;
  out["--ease"] = scale.motion.ease;
  out["--motion-enter"] = scale.motion.enter;
  out["--motion-exit"] = scale.motion.exit;
  out["--forge-motion-fast"] = "var(--motion-fast)";
  out["--forge-motion-slow"] = "var(--motion-slow)";
  out["--forge-ease"] = "var(--ease)";
  return out;
}

export function toCssVariables(active: Palette): Record<string, string> {
  const variables: Record<string, string> = {};
  for (const [key, value] of Object.entries(active)) {
    if (key !== "ansi" && typeof value === "string") {
      variables[`--forge-${cssName(key)}`] = value;
    }
  }
  active.ansi.forEach((color, index) => {
    variables[`--forge-ansi-${index}`] = color;
  });
  for (const [key, value] of Object.entries(metrics)) {
    variables[`--forge-${cssName(key)}`] = typeof value === "number" ? `${value}px` : value;
  }
  variables["--forge-mono-size"] = `${metrics.monoSize}px`;
  variables["--forge-mono-lh"] = String(metrics.monoLineHeight);
  variables["--forge-mono-line-height"] = String(metrics.monoLineHeight);
  Object.assign(
    variables,
    derivedTokens({
      bg: parseHex(active.bg),
      text: parseHex(active.text),
      sidebar: parseHex(active.sidebar),
      editor: parseHex(active.editor),
      accent: parseHex(active.accent),
      amber: parseHex(active.amber),
      needsYou: parseHex(active.needsYou),
      gitAdded: parseHex(active.gitAdded),
      gitDeleted: parseHex(active.gitDeleted),
    }),
    semanticColors(active),
  );
  return variables;
}

function cssName(key: string): string {
  return key.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();
}

export function applyTheme(active: Palette): void {
  const root = document.documentElement;
  for (const [name, value] of Object.entries(toCssVariables(active))) {
    root.style.setProperty(name, value);
  }
}
