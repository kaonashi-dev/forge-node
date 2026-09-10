/**
 * Ctrl+Tab gesture: hold Control, tap Tab to walk the MRU ring, release to
 * commit. A second tap (or a short hold) reveals the Switch Tab list.
 *
 * Bracket chords (`next_session`) stay on strip order; only this module owns
 * the hold-to-confirm path, because `cmd-shift-]` is a discrete press.
 */

import { createSignal } from "solid-js";
import { initialIndex, stepIndex, switcherIds, touchMru } from "./tabMru";

/** Show the list if Control is still held this long after the first Tab. */
const SHOW_DELAY_MS = 180;

export type TabSwitcherView = {
  ids: string[];
  index: number;
  /** First index that belongs to another checkout; `ids.length` when none. */
  foreignAt: number;
};

type Gesture = {
  ids: string[];
  index: number;
  foreignAt: number;
  /** Session that was current when the gesture began — Escape restores nothing. */
  originId: string;
};

const [history, setHistory] = createSignal<string[]>([]);
const [view, setView] = createSignal<TabSwitcherView | null>(null);

let gesture: Gesture | null = null;
let visible = false;
let acceptTab = true;
let showTimer: ReturnType<typeof setTimeout> | null = null;
let listening = false;
let onCommit: ((sessionId: string) => void) | null = null;

/** The list to render, or `null` while the gesture is hidden / idle. */
export const tabSwitcherView = view;

/** Remember a user-driven focus so the next Ctrl+Tab can return here. */
export function recordTabFocus(sessionId: string): void {
  if (gesture) return;
  setHistory(touchMru(history(), sessionId));
}

/**
 * Install the commit callback once from the shell. Returns the selected id on
 * Control-up; Escape and outside-click cancel without calling it.
 */
export function bindTabSwitcherCommit(commit: (sessionId: string) => void): () => void {
  onCommit = commit;
  return () => {
    if (onCommit === commit) onCommit = null;
  };
}

/**
 * One Ctrl+Tab / Ctrl+Shift+Tab.
 *
 * `localIds` is this checkout's strip; `liveIds` is every live terminal so the
 * hold list can offer a few recent sessions from other checkouts underneath.
 */
export function stepTabSwitcher(
  delta: number,
  localIds: readonly string[],
  activeId: string | null,
  liveIds: readonly string[] = localIds,
): void {
  if (!acceptTab) return;
  acceptTab = false;

  if (gesture) {
    gesture = {
      ...gesture,
      index: stepIndex(gesture.index, gesture.ids.length, delta),
    };
    reveal();
    publish();
    return;
  }

  const { ids, foreignAt } = switcherIds(history(), localIds, activeId, liveIds);
  if (ids.length < 2) return;

  gesture = {
    ids,
    foreignAt,
    index: initialIndex(ids.length, delta),
    originId: activeId ?? ids[0],
  };
  visible = false;
  publish();
  armListeners();
  showTimer = setTimeout(() => {
    showTimer = null;
    if (!gesture) return;
    reveal();
    publish();
  }, SHOW_DELAY_MS);
}

/** Click a row: commit that session and end the gesture. */
export function chooseTabSwitcher(sessionId: string): void {
  if (!gesture || !gesture.ids.includes(sessionId)) return;
  finish(sessionId);
}

/** Point the cursor at a row without committing (hover / click-prep). */
export function hoverTabSwitcher(index: number): void {
  if (!gesture || index < 0 || index >= gesture.ids.length) return;
  gesture = { ...gesture, index };
  reveal();
  publish();
}

/** Arrow keys while the list holds focus. */
export function nudgeTabSwitcher(delta: number): void {
  if (!gesture) return;
  gesture = {
    ...gesture,
    index: stepIndex(gesture.index, gesture.ids.length, delta),
  };
  reveal();
  publish();
}

/** Dismiss without changing the focused session. */
export function cancelTabSwitcher(): void {
  finish(null);
}

/** Test seam: forget history and any in-flight gesture. */
export function resetTabSwitcher(): void {
  clearShowTimer();
  disarmListeners();
  gesture = null;
  visible = false;
  acceptTab = true;
  setHistory([]);
  setView(null);
}

function reveal(): void {
  visible = true;
}

function publish(): void {
  if (!gesture || !visible) {
    setView(null);
    return;
  }
  setView({ ids: gesture.ids, index: gesture.index, foreignAt: gesture.foreignAt });
}

function finish(sessionId: string | null): void {
  clearShowTimer();
  disarmListeners();
  const chosen = sessionId;
  gesture = null;
  visible = false;
  acceptTab = true;
  setView(null);
  if (chosen) {
    setHistory(touchMru(history(), chosen));
    onCommit?.(chosen);
  }
}

function clearShowTimer(): void {
  if (showTimer !== null) {
    clearTimeout(showTimer);
    showTimer = null;
  }
}

function armListeners(): void {
  if (listening) return;
  listening = true;
  window.addEventListener("keyup", onKeyUp, true);
  window.addEventListener("keydown", onKeyDown, true);
}

function disarmListeners(): void {
  if (!listening) return;
  listening = false;
  window.removeEventListener("keyup", onKeyUp, true);
  window.removeEventListener("keydown", onKeyDown, true);
}

function onKeyUp(event: KeyboardEvent): void {
  if (event.key === "Tab") {
    acceptTab = true;
    return;
  }
  if (event.key === "Control") {
    const id = gesture?.ids[gesture.index] ?? null;
    finish(id);
  }
}

function onKeyDown(event: KeyboardEvent): void {
  if (event.key !== "Escape" || !gesture) return;
  event.preventDefault();
  event.stopPropagation();
  finish(null);
}
