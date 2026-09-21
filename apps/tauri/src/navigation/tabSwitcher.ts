/**
 * Ctrl+Tab gesture: hold Control, tap Tab to walk the MRU ring, release to
 * commit. A second tap (or a short hold) reveals the Switch Tab list.
 *
 * The ring is every pane of this checkout — the terminals *and* the files,
 * diffs and pull requests parked in Code — plus a few live sessions from other
 * checkouts.
 *
 * Bracket chords (`next_session`) stay on strip order; only this module owns
 * the hold-to-confirm path, because `cmd-shift-]` is a discrete press.
 */

import { createSignal } from "solid-js";
import { initialIndex, stepIndex, switcherKeys, touchMru } from "./tabMru";
import { sessionKey, targetKey, type SwitchTarget } from "./tabTargets";

/** Show the list if Control is still held this long after the first Tab. */
const SHOW_DELAY_MS = 180;

export type TabSwitcherView = {
  targets: SwitchTarget[];
  index: number;
  /** First index that belongs to another checkout; `targets.length` when none. */
  foreignAt: number;
};

const [history, setHistory] = createSignal<string[]>([]);
const [view, setView] = createSignal<TabSwitcherView | null>(null);

let gesture: TabSwitcherView | null = null;
let visible = false;
let pinned = false;
let acceptTab = true;
let showTimer: ReturnType<typeof setTimeout> | null = null;
let listening = false;
let onCommit: ((target: SwitchTarget) => void) | null = null;

/** The list to render, or `null` while the gesture is hidden / idle. */
export const tabSwitcherView = view;

/** Remember the pane on screen so the next Ctrl+Tab can return here. */
export function recordTabFocus(key: string): void {
  if (gesture) return;
  setHistory((previous) => touchMru(previous, key));
}

/**
 * Install the commit callback once from the shell. Returns the selected pane on
 * Control-up; Escape and outside-click cancel without calling it.
 */
export function bindTabSwitcherCommit(commit: (target: SwitchTarget) => void): () => void {
  onCommit = commit;
  return () => {
    if (onCommit === commit) onCommit = null;
  };
}

/**
 * One Ctrl+Tab / Ctrl+Shift+Tab.
 *
 * `local` is this checkout's panes; `liveSessionIds` is every live terminal, so
 * the hold list can offer a few recent sessions from other checkouts underneath.
 */
export function stepTabSwitcher(
  delta: number,
  local: readonly SwitchTarget[],
  activeKey: string | null,
  liveSessionIds: readonly string[] = [],
): void {
  if (!acceptTab) return;

  if (gesture) {
    acceptTab = false;
    gesture = { ...gesture, index: stepIndex(gesture.index, gesture.targets.length, delta) };
    reveal();
    publish();
    return;
  }

  const byKey = new Map<string, SwitchTarget>();
  const localKeys = local.map((target) => {
    const key = targetKey(target);
    byKey.set(key, target);
    return key;
  });
  const liveKeys = liveSessionIds.map((id) => {
    const key = sessionKey(id);
    if (!byKey.has(key)) byKey.set(key, { kind: "session", id });
    return key;
  });

  const { keys, foreignAt } = switcherKeys(history(), localKeys, activeKey, liveKeys);
  // The repeat latch stays open: nothing was armed, so no Tab keyup is coming
  // to clear it, and the chord would be dead for the rest of the session.
  if (keys.length < 2) return;

  acceptTab = false;
  gesture = {
    targets: keys.map((key) => byKey.get(key)!),
    foreignAt,
    index: initialIndex(keys.length, delta),
  };
  visible = false;
  pinned = false;
  publish();
  armListeners();
  showTimer = setTimeout(() => {
    showTimer = null;
    if (!gesture) return;
    reveal();
    publish();
  }, SHOW_DELAY_MS);
}

/** Click a row: commit that pane and end the gesture. */
export function chooseTabSwitcher(index: number): void {
  if (!gesture || index < 0 || index >= gesture.targets.length) return;
  finish(gesture.targets[index]);
}

/** Point the cursor at a row without committing (hover / click-prep). */
export function hoverTabSwitcher(index: number): void {
  if (!gesture || index < 0 || index >= gesture.targets.length) return;
  gesture = { ...gesture, index };
  reveal();
  publish();
}

/**
 * The pointer took the list over, so Control-up stops committing.
 *
 * Without this the list is unclickable: reaching for the mouse means letting
 * go of Control, which is also what closes the list on the row being aimed at.
 */
export function pinTabSwitcher(): void {
  if (!gesture || pinned) return;
  pinned = true;
  reveal();
  publish();
}

/** Arrow keys while the list holds focus. */
export function nudgeTabSwitcher(delta: number): void {
  if (!gesture) return;
  gesture = { ...gesture, index: stepIndex(gesture.index, gesture.targets.length, delta) };
  reveal();
  publish();
}

/** Commit the highlighted row, as Enter does. */
export function commitTabSwitcher(): void {
  if (!gesture) return;
  chooseTabSwitcher(gesture.index);
}

/** Dismiss without changing the focused pane. */
export function cancelTabSwitcher(): void {
  finish(null);
}

/** Test seam: forget history and any in-flight gesture. */
export function resetTabSwitcher(): void {
  clearShowTimer();
  disarmListeners();
  gesture = null;
  visible = false;
  pinned = false;
  acceptTab = true;
  setHistory([]);
  setView(null);
}

function reveal(): void {
  visible = true;
}

function publish(): void {
  setView(gesture && visible ? { ...gesture } : null);
}

function finish(target: SwitchTarget | null): void {
  clearShowTimer();
  disarmListeners();
  gesture = null;
  visible = false;
  pinned = false;
  acceptTab = true;
  setView(null);
  if (target) {
    // After `gesture = null` above: `recordTabFocus` ignores writes made while
    // a gesture is in flight, which is every other call on this path.
    recordTabFocus(targetKey(target));
    onCommit?.(target);
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
  if (event.key === "Control" && !pinned) {
    finish(gesture ? gesture.targets[gesture.index] : null);
  }
}

function onKeyDown(event: KeyboardEvent): void {
  if (event.key !== "Escape" || !gesture) return;
  event.preventDefault();
  event.stopPropagation();
  finish(null);
}
