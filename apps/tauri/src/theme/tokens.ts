import { contrast, derivedTokens, hex, mix, on, parseHex, pct, readableColor } from "./mix";

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
      bg: "#282828",
      rail: "#282828",
      sidebar: "#32302f",
      surface: "#32302f",
      surfaceHi: "#3c3836",
      editor: "#282828",
      step: "#504945",
      text: "#ebdbb2",
      muted: "#bdae93",
      faint: "#928374",
      accent: "#458588",
      needsYou: "#fe8019",
      needsYouDeep: "#d65d0e",
      green: "#b8bb26",
      red: "#fb4934",
      amber: "#fabd2f",
      blue: "#83a598",
      termBg: "#282828",
      termFg: "#ebdbb2",
      gitAdded: "#98971a",
      gitModified: "#d79921",
      gitDeleted: "#cc241d",
      gitUntracked: "#504945",
      gitConflict: "#b16286",
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

export const THEME_LABELS: Record<ThemeBaseId, string> = {
  "gruvbox-hard": "Gruvbox",
  gruvbox: "Gruvbox Dark",
  neutral: "Neutral Dark",
  "gruvbox-light": "Gruvbox Light",
  "neutral-light": "Neutral Light",
  ocean: "Ocean",
  forest: "Forest",
  custom: "Custom",
};

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
  step: "#404040",
  text: "#c7c7c7",
  muted: "#969696",
  faint: "#7d7d7d",
  needsYouDeep: "#d65d0e",
  green: "#98971a",
  amber: "#d79921",
  termFg: "#c7c7c7",
  gitAdded: "#46bb26",
  gitModified: "#d79921",
  gitUntracked: "#689d6a",
  gitDeleted: "#fb4934",
  gitConflict: "#fb4934",
  ansi: ["#1f1f1f", ...palettes.gruvbox.ansi.slice(1, 15), "#c7c7c7"],
};

export function registerCustomPalette(value: Palette, light: boolean): void {
  // Keep the replaceable custom slot out of built-in pickers and fixture exports.
  Object.defineProperty(palettes, "custom", { value, writable: true, configurable: true });
  customIsLight = light;
}

export const metrics = {
  titleH: 36,
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
  /** Pixel-valued names permit new steps without renumbering existing tokens. */
  space: [1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32],
  radius2XS: 3,
  /** Values below 40 stack within panes; 40 and above order global overlays. */
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

// Raw palette colours remain available to terminal rendering and theme export.
const SEMANTIC_FORWARDS: Record<string, string> = {
  "--bg-base": "var(--forge-bg)",
  "--bg-subtle": "var(--forge-sidebar)",
  "--bg-raised": "var(--forge-surface)",
  "--bg-overlay": "var(--forge-surface-hi)",
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
  "--info-solid": "var(--forge-blue)",
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

function semanticColors(
  active: Palette,
  interaction: Record<string, string>,
): Record<string, string> {
  const bg = parseHex(active.bg);
  const text = parseHex(active.text);
  const roles = {
    accent: active.accent,
    danger: active.red,
    success: active.green,
    warning: active.amber,
    attention: active.needsYou,
    info: active.blue,
  };
  const soft = (fill: string) => mix(bg, parseHex(fill), pct(14));
  const grounds = [
    ...[
      active.bg,
      active.rail,
      active.sidebar,
      active.surface,
      active.surfaceHi,
      active.editor,
    ].map(parseHex),
    ...Object.entries(interaction)
      .filter(([name]) => !name.includes("border"))
      .map(([, value]) => parseHex(value)),
    ...Object.values(roles).map(soft),
  ];
  const foreground = (fill: number) => {
    const candidate = on(fill, bg, text);
    return hex(contrast(candidate, fill) >= 4.5 ? candidate : on(fill, 0, 0xffffff));
  };
  const colors: Record<string, string> = {
    "--fg-default": hex(readableColor(text, grounds, text)),
    "--fg-muted": hex(readableColor(parseHex(active.muted), grounds, text)),
    "--fg-subtle": hex(readableColor(parseHex(active.faint), grounds, text)),
    "--accent-soft": hex(soft(active.accent)),
    "--danger-soft": hex(soft(active.red)),
    "--forge-focus-ring": hex(readableColor(parseHex(active.accent), grounds, text, 3)),
    "--border-strong": hex(mix(bg, text, pct(25))),
  };
  for (const [role, color] of Object.entries(roles)) {
    const fill = parseHex(color);
    colors[`--${role}-fg`] = foreground(fill);
    colors[`--${role}-text`] = hex(readableColor(fill, grounds, text));
    if (role === "accent" || role === "danger") {
      const hover = mix(fill, text, pct(12));
      colors[`--${role}-solid-hover`] = hex(hover);
      colors[`--${role}-hover-fg`] = foreground(hover);
    }
  }
  for (const [role, color] of Object.entries({
    added: active.gitAdded,
    modified: active.gitModified,
    deleted: active.gitDeleted,
    untracked: active.gitUntracked,
    conflict: active.gitConflict,
    ignored: active.gitIgnored,
  })) {
    colors[`--git-${role}-text`] = hex(readableColor(parseHex(color), grounds, text));
  }
  return colors;
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
  const interaction = derivedTokens({
    bg: parseHex(active.bg),
    text: parseHex(active.text),
    sidebar: parseHex(active.sidebar),
    editor: parseHex(active.editor),
    amber: parseHex(active.amber),
    needsYou: parseHex(active.needsYou),
    gitAdded: parseHex(active.gitAdded),
    gitDeleted: parseHex(active.gitDeleted),
  });
  Object.assign(variables, interaction, semanticColors(active, interaction));
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
