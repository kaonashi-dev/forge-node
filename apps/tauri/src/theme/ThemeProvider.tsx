import { type JSX, createSignal, onCleanup, onMount } from "solid-js";
import {
  applyTheme,
  baseIsLight,
  palettes,
  registerCustomPalette,
  type ThemeBaseId,
} from "./tokens";

import { parseCustomTheme } from "./customTheme";

const DEFAULT_BASE: ThemeBaseId = "gruvbox-hard";

export const SYSTEM_BASE = "system" as const;
export type ThemePreference = ThemeBaseId | typeof SYSTEM_BASE | `{${string}`;

const SYSTEM_DARK: ThemeBaseId = "gruvbox-hard";
const SYSTEM_LIGHT: ThemeBaseId = "gruvbox-light";

const [base, setBase] = createSignal<ThemeBaseId>(DEFAULT_BASE, { equals: false });
const [preference, setPreference] = createSignal<ThemePreference>(DEFAULT_BASE);

// JS-rendered surfaces must refresh even when one custom palette replaces another.
export const themeBase = base;

/** What was *chosen*, which may be `system`. The Settings radio reads this. */
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
  applyTheme(palettes[resolved]);
  setPreference(resolved === DEFAULT_BASE && next !== SYSTEM_BASE ? DEFAULT_BASE : next);
  setBase(resolved);
}

export function ThemeProvider(props: { children: JSX.Element; base?: ThemePreference }) {
  onMount(() => {
    applyThemeBase(props.base ?? DEFAULT_BASE);

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
