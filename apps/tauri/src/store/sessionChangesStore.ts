// Session-keyed changes reads survive tab switches, independently of checkout
// focus. Answers stay runtime-only; the split preference lives in shell/layout.ts.

import { createStore } from "solid-js/store";
import type { SessionChanges } from "../workbench/types";

/**
 * The floor between two automatic reads of one session, in milliseconds.
 *
 * A `change_summary` is four subprocesses. A session that alternates between
 * one-second bursts and two-second pauses would otherwise turn the quiet
 * trigger into a loop; this is what keeps the panel a user-paced read rather
 * than something on `docs/performance.md`'s rungs. The refresh button ignores
 * it — an explicit gesture is not a loop.
 */
export const REFRESH_FLOOR_MS = 10_000;

/** How long a session has to go quiet before the split re-reads it. */
export const QUIET_MS = 2_000;

type Entry = {
  changes: SessionChanges | null;
  error: string | null;
  loading: boolean;
  /** When the answer on screen was read, for the "as of" line. */
  readAt: number | null;
};

const empty = (): Entry => ({ changes: null, error: null, loading: false, readAt: null });

export const [sessionChangesStore, setSessionChangesStore] = createStore({
  /** Sessions whose split is open. Absent means "follow the preference". */
  open: {} as Record<string, boolean>,
  bySession: {} as Record<string, Entry>,
});

export function splitEntry(session: string): Entry {
  return sessionChangesStore.bySession[session] ?? empty();
}

/** Whether the split is open for `session`, falling back to the preference. */
export function splitOpen(session: string | null, fallback: boolean): boolean {
  if (session === null) return false;
  return sessionChangesStore.open[session] ?? fallback;
}

export function setSplitOpen(session: string, open: boolean): void {
  setSessionChangesStore("open", session, open);
}

export function beginSessionChanges(session: string): void {
  setSessionChangesStore("bySession", session, (entry) => ({
    ...(entry ?? empty()),
    loading: true,
    error: null,
  }));
}

export function applySessionChanges(session: string, changes: SessionChanges): void {
  setSessionChangesStore("bySession", session, {
    changes,
    error: null,
    loading: false,
    readAt: Date.now(),
  });
}

export function failSessionChanges(session: string, error: string): void {
  setSessionChangesStore("bySession", session, (entry) => ({
    ...(entry ?? empty()),
    loading: false,
    error,
  }));
}

/** Drop a closed session's answer so it cannot come back with the id reused. */
export function forgetSession(session: string): void {
  setSessionChangesStore("bySession", session, undefined!);
  setSessionChangesStore("open", session, undefined!);
}

/**
 * Whether an automatic read is allowed right now.
 *
 * A read already in flight blocks the next one as firmly as the floor does:
 * the answers are not ordered, and a second read landing first would show an
 * older count as the newer one.
 */
export function mayAutoRefresh(session: string, now = Date.now()): boolean {
  const entry = splitEntry(session);
  if (entry.loading || entry.error !== null) return false;
  if (entry.readAt === null) return true;
  return now - entry.readAt >= REFRESH_FLOOR_MS;
}
