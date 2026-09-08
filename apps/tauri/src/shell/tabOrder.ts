import type { Session } from "../runtime/types";

/** Snapshot order is a HashMap walk, so a first paint without a stored order would shuffle. */
function byCreatedAt(sessions: Session[]): Session[] {
  return [...sessions].sort(
    (a, b) => a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id),
  );
}

/** Apply a persisted tab order over the daemon's session list. */
export function openSessions(sessions: Session[], order: string[]): Session[] {
  const chronological = byCreatedAt(sessions);
  if (order.length === 0) return chronological;
  const byId = new Map(chronological.map((session) => [session.id, session]));
  const ordered: Session[] = [];
  for (const id of order) {
    const session = byId.get(id);
    if (session) {
      ordered.push(session);
      byId.delete(id);
    }
  }
  for (const session of chronological) {
    if (byId.has(session.id)) ordered.push(session);
  }
  return ordered;
}

/** Move a tab into a gap: 0 is before the first tab, length is after the last. */
export function moveTabToGap(order: string[], fromId: string, gap: number): string[] {
  const next = [...order];
  const fromIndex = next.indexOf(fromId);
  if (fromIndex < 0) return next;

  const boundedGap = Math.max(0, Math.min(gap, next.length));
  const insertAt = fromIndex < boundedGap ? boundedGap - 1 : boundedGap;
  if (insertAt === fromIndex) return next;

  next.splice(fromIndex, 1);
  next.splice(insertAt, 0, fromId);
  return next;
}

export function orderFromSessions(sessions: Session[], prior: string[]): string[] {
  return openSessions(sessions, prior).map((session) => session.id);
}

/** Which gap a pointer is in: 0 is before the first tab, length is after the last. */
export function gapAtX(tabs: readonly { left: number; width: number }[], x: number): number {
  for (let i = 0; i < tabs.length; i++) {
    const tab = tabs[i];
    if (x < tab.left + tab.width / 2) return i;
  }
  return tabs.length;
}

/** Same as `gapAtX`, for a vertical stack of checkout cards. */
export function gapAtY(items: readonly { top: number; height: number }[], y: number): number {
  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    if (y < item.top + item.height / 2) return i;
  }
  return items.length;
}

/** Where to paint the drop mark: `null` if the pointer is still on the dragged item. */
export function dropFromGap(
  ids: string[],
  gap: number,
  fromId: string,
): { id: string; after: boolean } | null {
  if (ids.length === 0) return null;
  const fromIndex = ids.indexOf(fromId);
  const bounded = Math.max(0, Math.min(gap, ids.length));
  const insertAt = fromIndex >= 0 && fromIndex < bounded ? bounded - 1 : bounded;
  if (insertAt === fromIndex) return null;
  if (bounded >= ids.length) return { id: ids[ids.length - 1], after: true };
  return { id: ids[bounded], after: false };
}
