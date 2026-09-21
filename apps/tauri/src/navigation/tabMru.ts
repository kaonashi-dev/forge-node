/**
 * Most-recently-used order for the Ctrl+Tab switcher.
 *
 * Strip order (`tabOrder`) is drag order. This ring is focus order over pane
 * keys (`tabTargets`): the pane you just left sits at index 1, so a single
 * Ctrl+Tab returns there the way a browser does — whether it was a terminal
 * or a file open in Code.
 */

/** Other-checkout rows appended under the local panes in the hold list. */
export const FOREIGN_RECENT = 3;

/**
 * How deep the ring remembers.
 *
 * Keys are panes, not sessions, so every file ever opened in every checkout
 * would otherwise leave a permanent entry — and `touchMru` copies the list on
 * each focus change. The list only ever feeds one checkout's strip order plus
 * `FOREIGN_RECENT` rows, so anything past this depth can never be read.
 */
const MRU_DEPTH = 64;

/** Move `key` to the front; drop duplicates, and anything past `MRU_DEPTH`. */
export function touchMru(history: readonly string[], key: string): string[] {
  const next = [key];
  for (const item of history) {
    if (next.length >= MRU_DEPTH) break;
    if (item !== key) next.push(item);
  }
  return next;
}

/**
 * Open panes in MRU order: the active one first, then the focus ring, then any
 * leftover strip order so a brand-new tab is still reachable.
 */
export function mruKeys(
  history: readonly string[],
  openKeys: readonly string[],
  activeKey: string | null,
): string[] {
  const remaining = new Set(openKeys);
  const out: string[] = [];
  if (activeKey && remaining.delete(activeKey)) out.push(activeKey);
  for (const key of history) {
    if (remaining.delete(key)) out.push(key);
  }
  for (const key of openKeys) {
    if (remaining.delete(key)) out.push(key);
  }
  return out;
}

/**
 * This checkout's panes (MRU) plus up to `foreignLimit` live sessions from
 * other checkouts, newest-focus first so the hold list can jump projects
 * without leaving the current strip behind.
 *
 * A foreign row has to be live, which is also what drops the parked views of
 * every other checkout out of the history: only sessions are ever live keys.
 */
export function switcherKeys(
  history: readonly string[],
  localKeys: readonly string[],
  activeKey: string | null,
  /** Every live terminal's key; order is the fallback when history is thin. */
  liveKeys: readonly string[],
  foreignLimit = FOREIGN_RECENT,
): { keys: string[]; foreignAt: number } {
  const local = mruKeys(history, localKeys, activeKey);
  const localOpen = new Set(localKeys);
  const live = new Set(liveKeys);
  const foreign: string[] = [];

  for (const key of history) {
    if (foreign.length >= foreignLimit) break;
    if (localOpen.has(key) || !live.has(key)) continue;
    foreign.push(key);
  }
  if (foreign.length < foreignLimit) {
    for (const key of liveKeys) {
      if (foreign.length >= foreignLimit) break;
      if (localOpen.has(key) || foreign.includes(key)) continue;
      foreign.push(key);
    }
  }

  return { keys: [...local, ...foreign], foreignAt: local.length };
}

/**
 * Live terminals, newest activity first — fallback order for foreign rows.
 *
 * Editors are excluded: an open file belongs to the Code strip, where it is
 * already a row of its own, not to the recent sessions of a checkout.
 */
export function liveIdsByActivity(
  sessions: readonly {
    id: string;
    kind?: string;
    terminal_id: string | null;
    last_activity_at?: string;
    created_at: string;
  }[],
): string[] {
  return sessions
    .filter((session) => session.terminal_id != null && session.kind !== "Editor")
    .slice()
    .sort((a, b) => {
      const aAt = a.last_activity_at ?? a.created_at;
      const bAt = b.last_activity_at ?? b.created_at;
      return bAt.localeCompare(aAt) || a.id.localeCompare(b.id);
    })
    .map((session) => session.id);
}

/** First step lands on the previous pane (or the last, when walking backward). */
export function initialIndex(length: number, delta: number): number {
  if (length < 2) return 0;
  return delta >= 0 ? 1 : length - 1;
}

export function stepIndex(index: number, length: number, delta: number): number {
  if (length <= 0) return 0;
  return (((index + delta) % length) + length) % length;
}
