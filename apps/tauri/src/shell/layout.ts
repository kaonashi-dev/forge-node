// Layout preferences, persisted through the daemon (§15.2).
//
// The daemon owns them rather than `localStorage`: it is the same store the
// so a persisted rail width survives relaunch.
// the next, and a second window opens where the first left off.

import { setAppState } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";

/** Keys, in `ui.*` — the namespace `apps/tauri already uses. */
export const RAIL_OPEN_KEY = "ui.sidebar.open";
export const RAIL_WIDTH_KEY = "ui.sidebar.width";
export const PANEL_OPEN_KEY = "ui.panel.open";
export const PANEL_WIDTH_KEY = "ui.panel.width";
export const THEME_BASE_KEY = "ui.theme_base";
/** Which inspector tab is up, so it survives a restart (§4.2 U9). */
export const PANEL_TAB_KEY = "ui.panel.tab";
/** Unified or split, in the Diff tab (§2.3 D2). */
export const DIFF_SPLIT_KEY = "ui.diff.split";
/**
 * Whether a session opens with its changes split showing (§16.7).
 *
 * Only the *preference* is persisted. Which sessions have it open is runtime
 * state in `sessionChangesStore`: the daemon purges session rows on startup,
 * so a remembered split would point at a session that no longer exists.
 */
export const SESSION_SPLIT_OPEN_KEY = "ui.session_split.open";
export const SESSION_SPLIT_WIDTH_KEY = "ui.session_split.width";
/** Row height multiplier: `compact` / `default` / `comfortable` (§5.4 T3). */
export const DENSITY_KEY = "ui.density";
/** Terminal font zoom, as a multiplier on the theme's mono size (§4.1 U3). */
export const TERMINAL_ZOOM_KEY = "ui.terminal.zoom";
/** Write the open file on blur and after a pause. Off by default (§2.2 A8). */
export const AUTOSAVE_KEY = "ui.editor.autosave";
/** Which editing model the editor uses: `default`, `vim` or `helix` (A9/A10). */
export const EDITOR_KEYMAP_KEY = "ui.editor.keymap";

/** Bounds the drag handles clamp to, so a panel cannot be dragged to nothing. */
export const RAIL_RANGE = { min: 180, max: 480, fallback: 240 };
/* 320 rather than 288: six inspector tabs are 307px of one row, and at 288 the
   strip opened already scrolled — with `History` cut in half at the left edge,
   because the tab it scrolls to is the selected one and `Git` is last. */
export const PANEL_RANGE = { min: 220, max: 560, fallback: 320 };
/* Narrower floor than the inspector: this column holds paths and two counts,
   never a patch, so it can give the terminal back more room than a panel that
   has to fit six tabs in one row. */
export const SESSION_SPLIT_RANGE = { min: 240, max: 620, fallback: 340 };

export function readFlag(key: string, fallback: boolean): boolean {
  const value = forgeStore.app_state[key];
  if (value === undefined) return fallback;
  return value === "true";
}

/**
 * Read a stored width, clamped to the range the handle allows.
 *
 * Clamped on the way *in*, not only on the way out: a value written by an
 * older build, or by a shell with a different minimum, must not be able to
 * collapse the panel to zero on startup.
 */
export function readWidth(
  key: string,
  range: { min: number; max: number; fallback: number },
): number {
  const raw = Number.parseInt(forgeStore.app_state[key] ?? "", 10);
  if (!Number.isFinite(raw)) return range.fallback;
  return Math.min(Math.max(raw, range.min), range.max);
}

export function writeFlag(key: string, value: boolean): void {
  void setAppState(key, value ? "true" : "false").catch(() => undefined);
}

export function writeWidth(key: string, value: number): void {
  void setAppState(key, String(Math.round(value))).catch(() => undefined);
}

/** Read one of a fixed set of strings, falling back when the stored one is not. */
export function readChoice<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  const value = forgeStore.app_state[key];
  return allowed.includes(value as T) ? (value as T) : fallback;
}

export function writeChoice(key: string, value: string): void {
  void setAppState(key, value).catch(() => undefined);
}

/** Read a stored multiplier, clamped. A bad one must not break the grid. */
export function readScale(key: string, min: number, max: number, fallback: number): number {
  const raw = Number.parseFloat(forgeStore.app_state[key] ?? "");
  if (!Number.isFinite(raw)) return fallback;
  return Math.min(Math.max(raw, min), max);
}
