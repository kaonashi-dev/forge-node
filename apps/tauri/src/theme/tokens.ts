import { contrast, derivedTokens, hex, mix, on, parseHex, pct, readableColor } from "./mix";
import { cssName, syntaxVariables } from "./syntax";

export type ThemeBaseId =
  | "calm-dark"
  | "calm-light"
  | "forge-dark"
  | "forge-light"
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
  "calm-light",
  "forge-light",
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
  strip: string;
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
  finish?: Finish;
} & Partial<Record<PaletteOverride, string>>;

/**
 * The Ink finish: grounds a base pins for the active tab, the strip's rule and
 * the hover wash. A base that has one also takes Ink's selection (a thinner
 * tint plus a hairline) and depth; a base without one keeps the ring-edged
 * tab, the 14% selection and the shared card shadow.
 */
export type Finish = { tab: string; rule: string; hover?: string };

export const HUE_COUNT = 9;

/**
 * Roles a base may pin instead of deriving: rows below primary text, icons
 * above faint, the card edge, and type on an ember wash. Absent, each derives
 * as it always has, so a palette without them keeps its look.
 */
export const PALETTE_OVERRIDES = ["fgDefault", "fgSubtle", "border", "needsYouText"] as const;
export type PaletteOverride = (typeof PALETTE_OVERRIDES)[number];

/** The tab strip sits halfway between the window and the card it belongs to. */
export function deriveStrip(rail: string, bg: string): string {
  return hex(mix(parseHex(rail), parseHex(bg), pct(50)));
}

type PaletteInput = Omit<Palette, "ansi" | "strip"> & { strip?: string };

/**
 * Calm's project hues, the source every base derives its nine from: they are
 * kept where they already clear the floor on that base and pulled toward its
 * text where they do not.
 */
const CALM_HUES = {
  dark: [
    "#5cc8c0",
    "#7aa2f7",
    "#7ed69a",
    "#d98ae0",
    "#f28fad",
    "#b5d86a",
    "#6cc4f0",
    "#e0c36a",
    "#a78bfa",
  ],
  light: [
    "#0c6e68",
    "#2f5fc9",
    "#1f7343",
    "#94379f",
    "#b0355f",
    "#536d0f",
    "#146892",
    "#86640b",
    "#6d28d9",
  ],
} as const;

const palette = (values: PaletteInput, ansi: string[]): Palette => ({
  ...values,
  strip: values.strip ?? deriveStrip(values.rail, values.bg),
  ansi,
});

export const palettes = {
  "calm-dark": palette(
    {
      bg: "#111112",
      rail: "#0c0c0d",
      sidebar: "#0c0c0d",
      strip: "#0e0e0f",
      surface: "#1c1c1f",
      surfaceHi: "#2a2a2e",
      editor: "#111112",
      step: "#2a2a2f",
      text: "#ededef",
      muted: "#a0a0a8",
      faint: "#7e7e87",
      accent: "#e4e4e7",
      needsYou: "#f59e4b",
      needsYouDeep: "#c2620f",
      green: "#4ade80",
      red: "#f87171",
      amber: "#e3b341",
      blue: "#7aa2f7",
      termBg: "#111112",
      termFg: "#c9c9cf",
      gitAdded: "#4ade80",
      gitModified: "#e3b341",
      gitDeleted: "#f87171",
      gitUntracked: "#67d4e8",
      gitConflict: "#f59e4b",
      gitIgnored: "#5a5a62",
      fgDefault: "#c9c9cf",
      fgSubtle: "#8a8a93",
      border: "#222225",
      needsYouText: "#f7b276",
      finish: { tab: "#1c1c1f", rule: "#1f1f22", hover: "#19191c" },
    },
    [
      "#1c1c1f",
      "#f87171",
      "#4ade80",
      "#e3b341",
      "#7aa2f7",
      "#a78bfa",
      "#67d4e8",
      "#c9c9cf",
      "#5a5a62",
      "#fca5a5",
      "#86efac",
      "#f0c96a",
      "#a5c0fa",
      "#c4b5fd",
      "#8fe0ee",
      "#ededef",
    ],
  ),
  "calm-light": palette(
    {
      bg: "#ffffff",
      rail: "#f4f4f2",
      sidebar: "#f4f4f2",
      strip: "#f9f9f8",
      surface: "#f1f1f2",
      surfaceHi: "#e6e6e9",
      editor: "#ffffff",
      step: "#d4d4d8",
      text: "#1a1a1c",
      muted: "#5e5e66",
      faint: "#67676f",
      accent: "#27272a",
      needsYou: "#a04a08",
      needsYouDeep: "#7c3a06",
      green: "#15803d",
      red: "#b91c1c",
      amber: "#93610a",
      blue: "#1d4ed8",
      termBg: "#ffffff",
      termFg: "#1a1a1c",
      gitAdded: "#15803d",
      gitModified: "#93610a",
      gitDeleted: "#b91c1c",
      gitUntracked: "#0e7490",
      gitConflict: "#a04a08",
      gitIgnored: "#67676f",
      fgDefault: "#2e2e33",
      fgSubtle: "#6d6d75",
      border: "#e4e4e7",
      finish: { tab: "#ffffff", rule: "#e9e9eb" },
    },
    [
      "#ffffff",
      "#b91c1c",
      "#15803d",
      "#93610a",
      "#1d4ed8",
      "#6d28d9",
      "#0e7490",
      "#5e5e66",
      "#67676f",
      "#dc2626",
      "#166534",
      "#8a5a00",
      "#2563eb",
      "#7c3aed",
      "#0e7490",
      "#1a1a1c",
    ],
  ),
  "forge-dark": palette(
    {
      bg: "#1a1918",
      rail: "#121211",
      sidebar: "#1a1918",
      surface: "#211f1d",
      surfaceHi: "#2a2725",
      editor: "#1a1918",
      step: "#34302d",
      text: "#ede5d6",
      muted: "#aea69a",
      faint: "#8d857a",
      accent: "#4fa39b",
      needsYou: "#f08a3c",
      needsYouDeep: "#b55a1c",
      green: "#9cbf57",
      red: "#e2645a",
      amber: "#e0b341",
      blue: "#7fa9cc",
      termBg: "#161514",
      termFg: "#cfc6b8",
      gitAdded: "#9cbf57",
      gitModified: "#e0b341",
      gitDeleted: "#e2645a",
      gitUntracked: "#b98bc9",
      gitConflict: "#f08a3c",
      gitIgnored: "#6e6760",
    },
    [
      "#1a1918",
      "#e2645a",
      "#9cbf57",
      "#e0b341",
      "#7fa9cc",
      "#b98bc9",
      "#4fa39b",
      "#aea69a",
      "#5e574f",
      "#ee8177",
      "#b0ce73",
      "#edc65f",
      "#9cbfdc",
      "#c9a4d6",
      "#6fbbb3",
      "#ede5d6",
    ],
  ),
  "forge-light": palette(
    {
      bg: "#faf8f3",
      rail: "#f1ece3",
      sidebar: "#f1ece3",
      surface: "#ffffff",
      surfaceHi: "#e7e0d4",
      editor: "#ffffff",
      step: "#dfd8cb",
      text: "#26231f",
      muted: "#6b6459",
      faint: "#8c857a",
      accent: "#29766f",
      needsYou: "#9e5615",
      needsYouDeep: "#7a410f",
      green: "#5c7a21",
      red: "#b03a31",
      amber: "#9a7314",
      blue: "#2f6187",
      termBg: "#faf8f3",
      termFg: "#26231f",
      gitAdded: "#5c7a21",
      gitModified: "#9a7314",
      gitDeleted: "#b03a31",
      gitUntracked: "#7a4a8c",
      gitConflict: "#9e5615",
      gitIgnored: "#8c857a",
    },
    [
      "#26231f",
      "#b03a31",
      "#5c7a21",
      "#9a7314",
      "#2f6187",
      "#7a4a8c",
      "#29766f",
      "#6b6459",
      "#8c857a",
      "#c9524a",
      "#6b8246",
      "#98751f",
      "#4a7ca6",
      "#96689f",
      "#35867f",
      "#14120f",
    ],
  ),
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
  "calm-dark": "Calm Dark",
  "calm-light": "Calm Light",
  "forge-dark": "Forge Dark",
  "forge-light": "Forge Light",
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
  strip: deriveStrip("#0c141e", "#111b27"),
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
  strip: deriveStrip("#111913", "#17211c"),
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

/**
 * Accents offered beside the theme cards. A light base gets darker versions so
 * a fill and its derived text still clear the floor on a white card.
 */
export const ACCENT_SWATCHES: Record<"dark" | "light", { name: string; color: string }[]> = {
  dark: [
    { name: "Neutral", color: "#e4e4e7" },
    { name: "Violet", color: "#a78bfa" },
    { name: "Teal", color: "#5ccabb" },
    { name: "Blue", color: "#7aa2f7" },
    { name: "Rose", color: "#f28fad" },
    { name: "Lime", color: "#a3d977" },
  ],
  light: [
    { name: "Neutral", color: "#27272a" },
    { name: "Violet", color: "#6d28d9" },
    { name: "Teal", color: "#0f766e" },
    { name: "Blue", color: "#1d4ed8" },
    { name: "Rose", color: "#be185d" },
    { name: "Lime", color: "#4d7c0f" },
  ],
};

export function registerCustomPalette(value: Palette, light: boolean): void {
  // Keep the replaceable custom slot out of built-in pickers and fixture exports.
  Object.defineProperty(palettes, "custom", { value, writable: true, configurable: true });
  customIsLight = light;
}

export const metrics = {
  titleH: 36,
  tabH: 32,
  stripH: 46,
  openFilesH: 36,
  locationH: 32,
  sidebarW: 300,
  handleW: 4,
  // Density moves `rowH`, `treeRowH` and the control ladder; this is the compact default.
  rowH: 26,
  treeRowH: 28,
  controlXS: 26,
  controlSM: 28,
  controlMD: 30,
  controlLG: 34,
  // Badge, row, button/tab, card/panel, pill. List and segmented (10) and the palette (14) live in `scale`.
  radiusXS: 5,
  radiusSM: 7,
  radiusMD: 8,
  radiusLG: 12,
  radiusXL: 999,
  // Geist Mono first; the rest catch glyphs it lacks, per glyph (docs/theming.md).
  mono: '"Geist Mono", "JetBrainsMono Nerd Font Mono", "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, monospace',
  monoSize: 13,
  monoLineHeight: 1.35,
  sans: 'Geist, -apple-system, BlinkMacSystemFont, "Segoe UI", "Helvetica Neue", Arial, sans-serif',
  display:
    'Geist, -apple-system, BlinkMacSystemFont, "Segoe UI", "Helvetica Neue", Arial, sans-serif',
  // Caption, row label, body, section heading, page title. `sm` is the chrome's base.
  textXS: 12,
  textSM: 13,
  textMD: 14,
  textLG: 15,
  textXL: 24,
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
  space: [1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 20, 22, 24, 28, 32, 44],
  radius2XS: 3,
  radiusList: 10,
  radiusPalette: 14,
  /** Section title and the mono section label; outside the chrome-size multiplier. */
  text2XL: 24,
  textLabel: 10,
  /** Counts, keys and branch names set in Geist Mono beside 13px chrome. */
  textMono: 11,
  /** Values below 40 stack within panes; 40 and above order global overlays. */
  z: {
    raised: 1,
    over: 2,
    handle: 3,
    strip: 5,
    route: 15,
    notice: 30,
    scrim: 50,
    dialog: 51,
    popup: 55,
    tooltip: 60,
  },
  /** Two depths: the card resting on the window, and anything floating over it. */
  shadow: {
    sm: "0 8px 20px rgb(0 0 0 / 40%)",
    md: "0 12px 32px rgb(0 0 0 / 35%)",
    lg: "0 30px 80px rgb(0 0 0 / 60%)",
  },
  weight: { regular: 400, medium: 500, semibold: 600, bold: 700 },
  /** Named for the role in the type scale (docs/theming.md), not the number. */
  leading: {
    none: 1,
    title: 1.2,
    heading: 1.3,
    label: 1.4,
    card: 1.45,
    caption: 1.5,
    body: 1.6,
    prose: 1.7,
  },
  tracking: { tight: "-0.01em", label: "0.1em" },
  opacity: { disabled: 0.5, dim: 0.55, drag: 0.45 },
  // Hover/focus/selection, menus/tooltips/toasts, sidebar and panel resize. Output repaints never animate.
  motion: {
    fast: "90ms",
    menu: "120ms",
    slow: "160ms",
    /** A result landing: a commit, an opened pull request. */
    settle: "240ms",
    /** The needs-you pulse, and nothing else loops this slowly. */
    breathe: "2400ms",
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
  "--tree-row-h": "var(--forge-tree-row-h)",
  "--strip-h": "var(--forge-strip-h)",
  "--tab-h": "var(--forge-tab-h)",
  "--location-h": "var(--forge-location-h)",
  "--open-files-h": "var(--forge-open-files-h)",
  /* The type scale and the two faces, so a rule names one layer, not both. */
  "--text-xs": "var(--forge-text-xs)",
  "--text-sm": "var(--forge-text-sm)",
  "--text-md": "var(--forge-text-md)",
  "--text-lg": "var(--forge-text-lg)",
  "--text-xl": "var(--forge-text-xl)",
  "--font-mono": "var(--forge-mono)",
  "--font-sans": "var(--forge-sans)",
  "--font-display": "var(--forge-display)",
  /* The redesign's surface names; the artboards spell components in these. */
  "--bg-window": "var(--forge-rail)",
  "--bg-strip": "var(--forge-strip)",
  /* Overlays and raised controls sit a step above the card; fields and tracks sink to the strip. */
  "--bg-float": "color-mix(in srgb, var(--forge-surface) 40%, var(--forge-bg))",
  "--bg-sunken": "var(--forge-strip)",
  "--border-field": "var(--forge-surface-hi)",
  "--tab-edge": "var(--forge-surface-hi)",
  /* Separators and line numbers: glyphs a reader counts, not words they read, so below faint. */
  "--fg-rule": "color-mix(in srgb, var(--forge-text) 22%, var(--forge-bg))",
  /* The accent's own foreground: a neutral accent is a light fill, where a white thumb vanishes. */
  "--switch-thumb-on": "var(--accent-fg)",
  "--fg-strong": "var(--forge-text-strong)",
  "--bg-terminal": "var(--forge-term-bg)",
  "--bg-panel": "var(--forge-bg)",
  "--bg-sidebar": "var(--forge-sidebar)",
  "--surface": "var(--forge-surface)",
  "--surface-hi": "var(--forge-surface-hi)",
  "--line": "var(--forge-step)",
  "--fg": "var(--fg-default)",
  "--fg-caption": "var(--fg-subtle)",
  "--accent": "var(--forge-accent)",
  "--attention": "var(--forge-needs-you)",
  "--radius-badge": "var(--forge-radius-xs)",
  "--radius-row": "var(--forge-radius-sm)",
  "--radius-control": "var(--forge-radius-md)",
  "--radius-card": "var(--radius-list)",
  "--radius-panel": "var(--forge-radius-lg)",
  "--radius-pill": "var(--forge-radius-xl)",
  /*
   * Selection never draws a bar down an edge: a chosen row changes its own
   * ground and carries a hairline ring *inside* its box, so nothing shifts.
   * Tabs are the exception and keep their underline on the shared edge.
   */
  "--select-bg": "color-mix(in srgb, var(--accent) var(--finish-select-tint), transparent)",
  "--select-ring": "var(--finish-select-ring)",
  "--attention-bg": "color-mix(in srgb, var(--attention) 13%, transparent)",
  "--attention-ring": "none",
  /* Ember washes: the pinned needs-you card and the strip's waiting pill, at rest and under the pointer. */
  "--attention-wash": "color-mix(in srgb, var(--attention) 9%, transparent)",
  "--attention-wash-hover": "color-mix(in srgb, var(--attention) 14%, transparent)",
  "--attention-pill": "color-mix(in srgb, var(--attention) 12%, transparent)",
  "--attention-pill-hover": "color-mix(in srgb, var(--attention) 20%, transparent)",
  /* A segmented track laid on the window ground, where the strip ground would sink nothing. */
  "--track-on-window": "color-mix(in srgb, var(--fg-strong) 4%, transparent)",
  "--attention-halo": "0 0 0 3px color-mix(in srgb, var(--attention) 18%, transparent)",
  "--card-select-border": "color-mix(in srgb, var(--accent) 50%, transparent)",
  "--card-select-halo": "0 0 0 3px color-mix(in srgb, var(--accent) 11%, transparent)",
  "--card-shadow": "var(--finish-card-depth)",
  "--overlay-shadow": "var(--shadow-lg), 0 1px 0 rgb(255 255 255 / 4%) inset",
  /* A callout or soft status: a wash and a hairline in one hue. */
  "--accent-tint-bg": "color-mix(in srgb, var(--accent) 7%, transparent)",
  "--accent-tint-border": "color-mix(in srgb, var(--accent) 32%, transparent)",
  "--danger-tint-bg": "color-mix(in srgb, var(--danger-solid) 7%, transparent)",
  "--danger-tint-border": "color-mix(in srgb, var(--danger-solid) 40%, transparent)",
  "--attention-tint-bg": "color-mix(in srgb, var(--attention) 7%, transparent)",
  "--attention-tint-border": "color-mix(in srgb, var(--attention) 30%, transparent)",
  "--diff-meta-bg": "color-mix(in srgb, var(--fg-default) 4%, var(--forge-editor))",
  "--diff-hunk-bg": "color-mix(in srgb, var(--info-solid) 7%, var(--forge-editor))",
  /* The two sides of a three-way merge, and a find hit wherever text is searched. */
  "--merge-ours-bg": "color-mix(in srgb, var(--info-solid) 12%, transparent)",
  "--merge-theirs-bg": "color-mix(in srgb, var(--git-untracked-text) 12%, transparent)",
  "--find-match-bg": "color-mix(in srgb, var(--warning-solid) 28%, transparent)",
  "--scrim": "rgb(5 5 6 / 55%)",
  "--focus-ring-width": "2px",
  "--focus-ring-offset": "2px",
  /* For a focusable thing whose container clips an outside outline. */
  "--focus-ring-inset": "inset 0 0 0 var(--focus-ring-width) var(--ring)",
  "--focus-ring-shadow": "0 0 0 2px var(--bg-panel), 0 0 0 4px var(--ring)",
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
    // `--select-bg` and `--attention-bg` are 13% tints laid over whichever panel holds the row.
    ...[active.bg, active.sidebar, active.surface, active.surfaceHi].flatMap((ground) =>
      [active.accent, active.needsYou].map((tint) =>
        mix(parseHex(ground), parseHex(tint), pct(13)),
      ),
    ),
  ];
  /*
   * Secondary and tertiary text sit on the window, the sidebar, the card, the
   * strip and a raised surface, never on an accent tint (a chosen row reads in
   * default text). Holding them to the tints too pulled both up to the same
   * grey and erased the step between them.
   */
  const textGrounds = [
    active.rail,
    active.sidebar,
    active.bg,
    active.strip,
    active.surface,
    active.editor,
  ].map(parseHex);
  const foreground = (fill: number) => {
    const candidate = on(fill, bg, text);
    return hex(contrast(candidate, fill) >= 4.5 ? candidate : on(fill, 0, 0xffffff));
  };
  const colors: Record<string, string> = {
    "--forge-text-strong": hex(readableColor(text, grounds, text)),
    "--fg-default": hex(readableColor(parseHex(active.fgDefault ?? active.text), grounds, text)),
    "--fg-muted": hex(readableColor(parseHex(active.muted), textGrounds, text)),
    "--fg-subtle": hex(readableColor(parseHex(active.fgSubtle ?? active.faint), textGrounds, text)),
    "--accent-soft": hex(soft(active.accent)),
    "--danger-soft": hex(soft(active.red)),
    "--forge-focus-ring": hex(readableColor(parseHex(active.accent), grounds, text, 3)),
    "--border-strong": hex(mix(bg, text, pct(25))),
  };
  for (const [role, color] of Object.entries(roles)) {
    const fill = parseHex(color);
    colors[`--${role}-fg`] = foreground(fill);
    colors[`--${role}-text`] = hex(readableColor(fill, grounds, text));
    if (role === "attention" && active.needsYouText) {
      colors["--attention-text"] = hex(readableColor(parseHex(active.needsYouText), grounds, text));
    }
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
 * The tokens that never change with the base: the scale forwards above and the
 * spacing/z/shadow/type/motion constants. Emitted once, in a plain `:root`.
 */
export function staticTokens(): Record<string, string> {
  const out: Record<string, string> = { ...SEMANTIC_FORWARDS };
  for (const value of scale.space) {
    out[`--space-${value}`] = `${value}px`;
  }
  out["--radius-2xs"] = `${scale.radius2XS}px`;
  out["--radius-list"] = `${scale.radiusList}px`;
  out["--radius-palette"] = `${scale.radiusPalette}px`;
  out["--text-2xl"] = `${scale.text2XL}px`;
  out["--text-label"] = `${scale.textLabel}px`;
  out["--text-mono"] = `${scale.textMono}px`;
  for (const [name, value] of Object.entries(scale.z)) {
    out[`--z-${name}`] = String(value);
  }
  for (const [name, value] of Object.entries(scale.shadow)) {
    out[`--shadow-${name}`] = value;
  }
  for (const [name, value] of Object.entries(scale.weight)) out[`--weight-${name}`] = String(value);
  for (const [name, value] of Object.entries(scale.leading)) {
    out[`--leading-${name}`] = String(value);
  }
  for (const [name, value] of Object.entries(scale.tracking)) out[`--tracking-${name}`] = value;
  for (const [name, value] of Object.entries(scale.opacity)) {
    out[`--opacity-${name}`] = String(value);
  }
  out["--motion-0"] = "0ms";
  out["--motion-fast"] = scale.motion.fast;
  out["--motion-menu"] = scale.motion.menu;
  out["--motion-slow"] = scale.motion.slow;
  out["--motion-settle"] = scale.motion.settle;
  out["--motion-breathe"] = scale.motion.breathe;
  out["--depth-0"] = "none";
  out["--depth-1"] = "none";
  out["--depth-2"] = "var(--shadow-md)";
  out["--depth-3"] = "var(--shadow-lg)";
  out["--ease"] = scale.motion.ease;
  out["--motion-enter"] = scale.motion.enter;
  out["--motion-exit"] = scale.motion.exit;
  return out;
}

function isOverride(key: string): key is PaletteOverride {
  return (PALETTE_OVERRIDES as readonly string[]).includes(key);
}

export function toCssVariables(active: Palette): Record<string, string> {
  const variables: Record<string, string> = {};
  for (const [key, value] of Object.entries(active)) {
    if (key !== "ansi" && !isOverride(key) && typeof value === "string") {
      variables[`--forge-${cssName(key)}`] = value;
    }
  }
  active.ansi.forEach((color, index) => {
    variables[`--forge-ansi-${index}`] = color;
  });
  for (const [key, value] of Object.entries(metrics)) {
    const unitless = key === "monoLineHeight";
    variables[`--forge-${cssName(key)}`] =
      typeof value === "number" && !unitless ? `${value}px` : String(value);
  }
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
  if (active.border) interaction["--forge-border"] = active.border;
  if (active.finish?.hover) interaction["--forge-hover"] = active.finish.hover;
  Object.assign(
    variables,
    interaction,
    semanticColors(active, interaction),
    syntaxVariables(active),
    finishVariables(active),
    hueVariables(active),
  );
  return variables;
}

function isLightGround(active: Palette): boolean {
  return contrast(parseHex(active.bg), 0) > contrast(parseHex(active.bg), 0xffffff);
}

function finishVariables(active: Palette): Record<string, string> {
  const light = isLightGround(active);
  const finish = active.finish;
  if (!finish) {
    return {
      "--finish-tab-bg": "var(--border-subtle)",
      "--finish-tab-shadow": "inset 0 0 0 1px var(--tab-edge)",
      "--finish-strip-rule": "var(--surface)",
      "--finish-select-tint": "14%",
      "--finish-select-ring": "none",
      "--finish-top-light": "rgb(255 255 255 / 3%)",
      "--finish-card-depth": "var(--shadow-md)",
    };
  }
  return {
    "--finish-tab-bg": finish.tab,
    "--finish-tab-shadow": light
      ? "inset 0 0 0 1px var(--border-subtle), 0 1px 2px rgb(0 0 0 / 6%)"
      : "inset 0 1px 0 rgb(255 255 255 / 6%), 0 1px 2px rgb(0 0 0 / 50%)",
    "--finish-strip-rule": finish.rule,
    "--finish-select-tint": "8%",
    "--finish-select-ring": light
      ? "inset 0 0 0 1px rgb(0 0 0 / 6%)"
      : "inset 0 0 0 1px rgb(255 255 255 / 8%)",
    "--finish-top-light": light ? "transparent" : "rgb(255 255 255 / 4%)",
    "--finish-card-depth": light
      ? "0 1px 2px rgb(0 0 0 / 5%), 0 12px 32px rgb(0 0 0 / 8%)"
      : "0 18px 40px rgb(0 0 0 / 50%)",
  };
}

/**
 * The tint a hue sits on behind an avatar, over the ground the avatar sits
 * on. 15% on a dark ground, 12% on a light one, where the same share reads
 * heavier.
 */
export function hueTint(active: Palette): number {
  return isLightGround(active) ? 12 : 15;
}

/**
 * Every hue clears 4.5:1 as an avatar's initial on its own tint, over the
 * sidebar, the card and the strip, so a square or a line drawn in it clears
 * the 3:1 a shape needs with room to spare.
 */
export function projectHues(active: Palette): string[] {
  const source = CALM_HUES[isLightGround(active) ? "light" : "dark"];
  const amount = pct(hueTint(active));
  const plain = [active.sidebar, active.bg, active.strip, active.surface].map(parseHex);
  const text = parseHex(active.text);
  const tinted = (hue: number) => [...plain, ...plain.map((ground) => mix(ground, hue, amount))];
  return source.map((color) => {
    // The tint is made of the hue, so moving the hue moves its ground too.
    let hue = parseHex(color);
    for (let pass = 0; pass < 4; pass += 1) {
      const next = readableColor(hue, tinted(hue), text);
      if (next === hue) break;
      hue = next;
    }
    return hex(hue);
  });
}

function hueVariables(active: Palette): Record<string, string> {
  const out: Record<string, string> = { "--hue-tint": `${hueTint(active)}%` };
  projectHues(active).forEach((color, index) => {
    out[`--hue-${index}`] = color;
  });
  return out;
}

export function applyTheme(active: Palette): void {
  const root = document.documentElement;
  for (const [name, value] of Object.entries(toCssVariables(active))) {
    root.style.setProperty(name, value);
  }
}
