import { rolePrefix, sessionTitle, type Session } from "../../contracts/runtime";
import { openSessions } from "../../navigation/tabOrder";

/**
 * Order sessions as a parent/child tree.
 *
 * Roots (`parent_session_id === null`) come first; each root's descendants
 * follow depth-first. Siblings take the tab strip's order — shells first, then
 * the checkout's dragged `order` — so the rail and the tabs never disagree.
 *
 * A flat list would hide that a child belongs to the session that started it.
 */
export type TreeRow = {
  session: Session;
  /** `0` is a root. */
  depth: number;
};

export function orderSessionsTree(sessions: Session[], order: string[] = []): TreeRow[] {
  if (sessions.length === 0) return [];

  const rank = new Map(openSessions(sessions, order).map((session, i) => [session.id, i]));
  const byTab = (left: Session, right: Session) =>
    (rank.get(left.id) ?? 0) - (rank.get(right.id) ?? 0);

  const byParent = new Map<string | null, Session[]>();
  for (const session of sessions) {
    const key = session.parent_session_id;
    const siblings = byParent.get(key);
    if (siblings) siblings.push(session);
    else byParent.set(key, [session]);
  }
  for (const siblings of byParent.values()) {
    siblings.sort(byTab);
  }

  const out: TreeRow[] = [];
  const walk = (session: Session, depth: number): void => {
    out.push({ session, depth });
    const children = byParent.get(session.id);
    if (!children) return;
    byParent.delete(session.id);
    for (const child of children) walk(child, depth + 1);
  };
  for (const root of byParent.get(null) ?? []) walk(root, 0);
  byParent.delete(null);

  // Orphans whose parent is not in this list — a child in another workspace,
  // or one whose parent has been closed. Flat at the end rather than dropped:
  // a running agent that no longer has a parent is still running.
  const placed = new Set(out.map((row) => row.session.id));
  const stragglers = sessions.filter((session) => !placed.has(session.id)).sort(byTab);
  for (const session of stragglers) out.push({ session, depth: 0 });

  return out;
}

/** Rail or tab title with its role prefix, when the role is not the generic one. */
export function sessionDisplayTitle(session: Session): string {
  const base = sessionTitle(session);
  const prefix = rolePrefix(session.role);
  return prefix ? `${prefix} · ${base}` : base;
}
