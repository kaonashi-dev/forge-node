import { rolePrefix, sessionTitle, type Session } from "../../contracts/runtime";

/**
 * Order sessions as a parent/child tree.
 *
 * Roots (`parent_session_id === null`) come first in creation order; each
 * root's descendants follow depth-first, sorted by `created_at` among siblings.
 *
 * A flat list would hide that a child belongs to the session that started it.
 */
export type TreeRow = {
  session: Session;
  /** `0` is a root. */
  depth: number;
};

export function orderSessionsTree(sessions: Session[]): TreeRow[] {
  if (sessions.length === 0) return [];

  const byParent = new Map<string | null, Session[]>();
  for (const session of sessions) {
    const key = session.parent_session_id;
    const siblings = byParent.get(key);
    if (siblings) siblings.push(session);
    else byParent.set(key, [session]);
  }
  for (const siblings of byParent.values()) {
    siblings.sort((left, right) => left.created_at.localeCompare(right.created_at));
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
  const stragglers = sessions
    .filter((session) => !placed.has(session.id))
    .sort((left, right) => left.created_at.localeCompare(right.created_at));
  for (const session of stragglers) out.push({ session, depth: 0 });

  return out;
}

/** Rail or tab title with its role prefix, when the role is not the generic one. */
export function sessionDisplayTitle(session: Session): string {
  const base = sessionTitle(session);
  const prefix = rolePrefix(session.role);
  return prefix ? `${prefix} · ${base}` : base;
}
