import { createSignal } from "solid-js";
import { PANEL_TAB_KEY, readChoice, writeChoice } from "../shell/layout";

export const INSPECTOR_TABS = ["History", "PR", "Features", "Lieutenant", "Files", "Git"] as const;

export type InspectorTab = (typeof INSPECTOR_TABS)[number];

const [tab, setTab] = createSignal<InspectorTab>("Git");

/**
 * Which inspector tab is up.
 *
 * Window state rather than `RightPanel`'s own: the shortcuts that name a tab
 * (`MOD-shift-F` for files, `MOD-shift-R` for pull requests) have to work with
 * the inspector collapsed, and a signal that only exists while the panel is
 * mounted cannot answer them. Held here, `AppShell` can open the panel *and*
 * point it at the tab in one action.
 */
export const inspectorTab = tab;

/**
 * Choose the inspector's tab, and remember it (§4.2 U9).
 *
 * Through the daemon's `app_state`, like the rail width and the theme: a
 * person who works in the Files tab should not be put back on Git by every
 * restart, and `app_state` is the store every other `ui.*` preference is in.
 *
 * The read is deferred to `restoreInspectorTab` rather than done at module
 * scope: this module is imported before the snapshot arrives, and the stored
 * value does not exist yet.
 */
export function setInspectorTab(next: InspectorTab): void {
  if (tab() === next) return;
  setTab(next);
  writeChoice(PANEL_TAB_KEY, next);
}

/** Point the inspector at the stored tab. Called once the snapshot lands. */
export function restoreInspectorTab(): void {
  setTab(readChoice(PANEL_TAB_KEY, INSPECTOR_TABS, "Git"));
}
