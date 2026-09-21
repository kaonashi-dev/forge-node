/**
 * The ring's inputs, read off the stores the window renders from.
 *
 * Here rather than inside `AppShell` so a test can assemble the same ring the
 * gesture walks. The defect this file exists to catch is a ring that disagrees
 * with the strip on screen.
 */

import { createEffect, createMemo, on } from "solid-js";
import {
  connectionStore,
  pendingSessionSelection,
  sessionSelectionPending,
} from "../state/connection";
import { activeWorkspace } from "../state/workspace";
import { centerMode, codeOpen, currentViews } from "./viewsStore";
import { activeTargetKey, localTargets, type SwitchTarget } from "./tabTargets";
import { recordTabFocus } from "./tabSwitcher";

/**
 * Feed the Ctrl+Tab ring from whatever the centre column ends up showing.
 *
 * Installs a memo and an effect into the caller's reactive owner, so it must be
 * called from a component or a root, once. An effect rather than a call inside
 * each way in: a file opened from the tree, a Code tab clicked, a session
 * raised from the palette and a checkout switch are all "where I was", and the
 * pane on screen is the only thing they have in common.
 */
export function trackTabFocus(): void {
  createEffect(
    on(createMemo(activeSwitcherKey), (key) => {
      if (key) recordTabFocus(key);
    }),
  );
}

/** This checkout's panes; `sessionIds` is the strip, in strip order. */
export function switcherTargets(sessionIds: readonly string[]): SwitchTarget[] {
  return localTargets(activeWorkspace(), currentViews().open, sessionIds);
}

export function activeSwitcherKey(): string | null {
  const views = currentViews();
  // The command promise only acknowledges enqueueing, so the centre column has
  // already left the pane the user is on while `activeSession` still names it.
  const catchingUp =
    sessionSelectionPending() && pendingSessionSelection() !== connectionStore.activeSession;
  const session = catchingUp ? null : connectionStore.activeSession;
  return activeTargetKey(
    centerMode() === "code" && codeOpen(),
    activeWorkspace(),
    views.open,
    views.active,
    session,
  );
}
