// Layout preferences, persisted through the daemon (§15.2).
//
// The daemon owns them rather than `localStorage`: it is the same store the
// so a persisted rail width survives relaunch.
// the next, and a second window opens where the first left off.

import { createEffect } from "solid-js";

import { setAppState } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";

/** Keys, in `ui.*` — the namespace `apps/tauri already uses. */
export const SIDEBAR_OPEN_KEY = "ui.sidebar.open";
export const SIDEBAR_WIDTH_KEY = "ui.sidebar.width";
export const SIDEBAR_VIEW_KEY = "ui.sidebar.view";
export const THEME_BASE_KEY = "ui.theme_base";
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
/**
 * The checkout the window was last pointed at, so a relaunch opens where the
 * person left off rather than on whichever worktree happens to be first.
 *
 * The checkout and not the session: the daemon purges session rows on startup
 * (`sessions.persist_history = false`), so a remembered session would name a
 * row that no longer exists, while the worktree it belonged to is still there.
 */
export const LAST_WORKSPACE_KEY = "ui.last_workspace";

/** Bounds the drag handles clamp to, so a panel cannot be dragged to nothing. */
/* One width for every view. 224 rather than 180: the strip is seven 24px
   glyphs, six 6px gaps and 16px of padding (220px) plus the hairline border,
   and below that the last view is clipped. Must stay in step with
   `metrics.sidebarW`, which paints the first frame before the stored width
   arrives. */
export const SIDEBAR_RANGE = { min: 224, max: 560, fallback: 300 };
/* Narrower floor than the sidebar: this column holds paths and two counts,
   never a patch, so it can give the terminal back more room than a panel that
   has to fit seven views in one row. */
export const SESSION_SPLIT_RANGE = { min: 240, max: 620, fallback: 340 };

/**
 * Run `seed` once the daemon's stored preferences have landed.
 *
 * The window paints before the snapshot arrives, so a preference read into a
 * signal at *creation* reads an empty `app_state`, keeps the fallback, and
 * keeps it forever: the write path still works and the value is in the
 * database, but every relaunch quietly ignores it. A read inside a reactive
 * context corrects itself when the snapshot lands, because `forgeStore` is a
 * store; a `createSignal(read…())` cannot, and that is the shape this exists
 * for.
 *
 * Seeded once rather than tracked, like `AppShell` does with the rail: a later
 * write to `app_state` must not yank a panel the person has since dragged.
 */
export function seedFromAppState(seed: () => void): void {
  let seeded = false;
  createEffect(() => {
    if (seeded || Object.keys(forgeStore.app_state).length === 0) return;
    seeded = true;
    seed();
  });
}

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
