import { type JSX, createSignal, onCleanup, onMount } from "solid-js";
import { applyDensity, isDensity } from "./density";
import {
  applyTheme,
  baseIsLight,
  palettes,
  registerCustomPalette,
  type ThemeBaseId,
} from "./tokens";
import { applyUiFont } from "./uiFont";

import { parseCustomTheme } from "./customTheme";
import { withGlass } from "./glass";

const DEFAULT_BASE: ThemeBaseId = "calm-dark";

export const SYSTEM_BASE = "system" as const;
export type ThemePreference = ThemeBaseId | typeof SYSTEM_BASE | `{${string}`;

const SYSTEM_DARK: ThemeBaseId = "calm-dark";
const SYSTEM_LIGHT: ThemeBaseId = "calm-light";

const [base, setBase] = createSignal<ThemeBaseId>(DEFAULT_BASE, { equals: false });
const [preference, setPreference] = createSignal<ThemePreference>(DEFAULT_BASE);
const [glass, setGlass] = createSignal(false);

/** Whether the Glass finish is asked for; it only takes effect on a dark base. */
export const glassPreference = glass;

/** Turns the Glass finish on or off and repaints the current theme under it. */
export function applyGlass(on: boolean): void {
  if (glass() === on) return;
  setGlass(on);
  applyThemeBase(preference());
}

// JS-rendered surfaces must refresh even when one custom palette replaces another.
export const themeBase = base;

/** The chosen preference, including `system` or an embedded custom palette. */
export const themePreference = preference;

function prefersLight(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia("(prefers-color-scheme: light)").matches;
}

/** The concrete base a preference resolves to right now. */
export function resolveBase(next: ThemePreference): ThemeBaseId {
  if (next === SYSTEM_BASE) return prefersLight() ? SYSTEM_LIGHT : SYSTEM_DARK;
  if (next.startsWith("{")) {
    try {
      const custom = parseCustomTheme(next);
      registerCustomPalette(custom.palette, custom.mode === "light");
      return "custom";
    } catch {
      return DEFAULT_BASE;
    }
  }
  return Object.hasOwn(palettes, next) ? (next as ThemeBaseId) : DEFAULT_BASE;
}

export function applyThemeBase(next: ThemePreference = DEFAULT_BASE): void {
  const resolved = resolveBase(next);
  document.documentElement.dataset.theme = resolved;
  // Native controls and scrollbars need the same mode as the CSS tokens.
  document.documentElement.style.colorScheme = baseIsLight(resolved) ? "light" : "dark";
  const glassy = glass() && !baseIsLight(resolved);
  if (glassy) document.documentElement.dataset.glass = "true";
  else delete document.documentElement.dataset.glass;
  applyTheme(glassy ? withGlass(palettes[resolved]) : palettes[resolved]);
  // `applyTheme` rewrites every `--forge-*` metric, including the ones density
  // and UI font overlay. Put those back from `dataset` so a theme switch does
  // not snap the chrome to the defaults.
  restoreChromeOverlays();
  setPreference(resolved === DEFAULT_BASE && next !== SYSTEM_BASE ? DEFAULT_BASE : next);
  setBase(resolved);
}

function restoreChromeOverlays(): void {
  const { dataset } = document.documentElement;
  if (isDensity(dataset.density)) applyDensity(dataset.density);
  const font = Number.parseFloat(dataset.uiFont ?? "");
  if (Number.isFinite(font)) applyUiFont(font);
}

export function ThemeProvider(props: { children: JSX.Element; base?: ThemePreference }) {
  onMount(() => {
    applyThemeBase(props.base ?? preference());

    // Keep listening so choosing system later also follows OS changes.
    if (typeof window === "undefined" || !window.matchMedia) return;
    const query = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => {
      if (preference() === SYSTEM_BASE) applyThemeBase(SYSTEM_BASE);
    };
    query.addEventListener("change", onChange);
    onCleanup(() => query.removeEventListener("change", onChange));
  });
  return props.children;
}
