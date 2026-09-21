/**
 * The ring's inputs, read off the stores the window renders from.
 *
 * Here rather than inside `AppShell` so a test can assemble the same ring the
 * gesture walks. The defect this file exists to catch is a ring that disagrees
 * with the strip on screen — an empty Code tab the strip offers and the ring
 * cannot reach is a Ctrl+Tab that does nothing at all.
 */

import { connectionStore } from "../state/connection";
import { activeWorkspace } from "../state/workspace";
import { centerMode, codeOpen, currentViews } from "./viewsStore";
import { activeTargetKey, localTargets, type SwitchTarget } from "./tabTargets";

/** This checkout's panes; `sessionIds` is the strip, in strip order. */
export function switcherTargets(sessionIds: readonly string[]): SwitchTarget[] {
  return localTargets(activeWorkspace(), currentViews().open, sessionIds);
}

export function activeSwitcherKey(): string | null {
  const views = currentViews();
  return activeTargetKey(
    centerMode() === "code" && codeOpen(),
    activeWorkspace(),
    views.open,
    views.active,
    connectionStore.activeSession,
  );
}
