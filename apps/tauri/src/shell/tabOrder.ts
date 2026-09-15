import type { Session } from "../runtime/types";

/** Snapshot order is a HashMap walk, so a first paint without a stored order would shuffle. */
function byCreatedAt(sessions: Session[]): Session[] {
  return [...sessions].sort(
    (a, b) => a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id),
  );
}

/** Same discriminator the tab labels use: a shell has no agent provider, and an editor is never one. */
function isShell(session: Session): boolean {
  return session.agent_provider_id == null && session.kind !== "Editor";
}

/**
 * Shells, then agents, preserving relative order inside each group.
 *
 * A new terminal then lands after the existing shells rather than after the
 * last agent, and a new agent still appends. Drag order inside a group
 * survives; a drop across groups does not.
 */
function groupShellsThenAgents(sessions: Session[]): Session[] {
  const shells: Session[] = [];
  const agents: Session[] = [];
  for (const session of sessions) {
    if (isShell(session)) shells.push(session);
    else agents.push(session);
  }
  return [...shells, ...agents];
}

/** Apply a persisted tab order over the daemon's session list. */
export function openSessions(sessions: Session[], order: string[]): Session[] {
  const chronological = byCreatedAt(sessions);
  if (order.length === 0) return groupShellsThenAgents(chronological);
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
  return groupShellsThenAgents(ordered);
}

/** One entry in the window strip, including the Code tab when it exists. */
export type StripItem = { kind: "code" } | { kind: "session"; id: string };

/**
 * What Option+N and the next/previous chords walk.
 *
 * Code leads so Option+1 is the files when they are open; sessions follow in
 * strip order (shells, then agents).
 */
export function stripItems(sessions: readonly Session[], codeOpen: boolean): StripItem[] {
  const items: StripItem[] = [];
  if (codeOpen) items.push({ kind: "code" });
  for (const session of sessions) items.push({ kind: "session", id: session.id });
  return items;
}

/** Index of the tab the window is showing, or `-1` when none of them is. */
export function stripIndex(
  items: readonly StripItem[],
  codeActive: boolean,
  activeSession: string | null,
): number {
  if (codeActive) return items.findIndex((item) => item.kind === "code");
  if (!activeSession) return -1;
  return items.findIndex((item) => item.kind === "session" && item.id === activeSession);
}

/**
 * Keep a drag inside its kind: dropping a shell among agents (or the reverse)
 * would only snap back on the next paint.
 */
export function clampGapToKind(sessions: readonly Session[], fromId: string, gap: number): number {
  const from = sessions.find((session) => session.id === fromId);
  if (!from) return gap;
  const draggingShell = isShell(from);
  let start = 0;
  let end = sessions.length;
  for (let i = 0; i < sessions.length; i++) {
    if (draggingShell && !isShell(sessions[i])) {
      end = i;
      break;
    }
    if (!draggingShell && isShell(sessions[i])) start = i + 1;
  }
  return Math.max(start, Math.min(gap, end));
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
