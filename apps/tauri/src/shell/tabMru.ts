/**
 * Most-recently-used order for the Ctrl+Tab session switcher.
 *
 * Strip order (`tabOrder`) is drag order. This ring is focus order: the tab
 * you just left sits at index 1, so a single Ctrl+Tab returns there the way a
 * browser does.
 */

/** Other-checkout rows appended under the local strip in the hold list. */
export const FOREIGN_RECENT = 3;

/** Move `id` to the front; drop duplicates. */
export function touchMru(history: readonly string[], id: string): string[] {
  return [id, ...history.filter((item) => item !== id)];
}

/**
 * Open sessions in MRU order: the active tab first, then the focus ring, then
 * any leftover strip order so a brand-new tab is still reachable.
 */
export function mruIds(
  history: readonly string[],
  openIds: readonly string[],
  activeId: string | null,
): string[] {
  const remaining = new Set(openIds);
  const out: string[] = [];
  if (activeId && remaining.delete(activeId)) out.push(activeId);
  for (const id of history) {
    if (remaining.delete(id)) out.push(id);
  }
  for (const id of openIds) {
    if (remaining.delete(id)) out.push(id);
  }
  return out;
}

/**
 * Local strip (MRU) plus up to `foreignLimit` live sessions from other
 * checkouts, newest-focus first so the hold list can jump projects without
 * leaving the current strip behind.
 */
export function switcherIds(
  history: readonly string[],
  localOpenIds: readonly string[],
  activeId: string | null,
  /** Every live terminal id; order is the fallback when history is thin. */
  liveIds: readonly string[],
  foreignLimit = FOREIGN_RECENT,
): { ids: string[]; foreignAt: number } {
  const local = mruIds(history, localOpenIds, activeId);
  const localOpen = new Set(localOpenIds);
  const live = new Set(liveIds);
  const foreign: string[] = [];

  for (const id of history) {
    if (foreign.length >= foreignLimit) break;
    if (localOpen.has(id) || !live.has(id)) continue;
    foreign.push(id);
  }
  if (foreign.length < foreignLimit) {
    for (const id of liveIds) {
      if (foreign.length >= foreignLimit) break;
      if (localOpen.has(id) || foreign.includes(id)) continue;
      foreign.push(id);
    }
  }

  return { ids: [...local, ...foreign], foreignAt: local.length };
}

/** Live terminals, newest activity first — fallback order for foreign rows. */
export function liveIdsByActivity(
  sessions: readonly {
    id: string;
    terminal_id: string | null;
    last_activity_at?: string;
    created_at: string;
  }[],
): string[] {
  return sessions
    .filter((session) => session.terminal_id != null)
    .slice()
    .sort((a, b) => {
      const aAt = a.last_activity_at ?? a.created_at;
      const bAt = b.last_activity_at ?? b.created_at;
      return bAt.localeCompare(aAt) || a.id.localeCompare(b.id);
    })
    .map((session) => session.id);
}

/** First step lands on the previous tab (or the last, when walking backward). */
export function initialIndex(length: number, delta: number): number {
  if (length < 2) return 0;
  return delta >= 0 ? 1 : length - 1;
}

export function stepIndex(index: number, length: number, delta: number): number {
  if (length <= 0) return 0;
  return (((index + delta) % length) + length) % length;
}
