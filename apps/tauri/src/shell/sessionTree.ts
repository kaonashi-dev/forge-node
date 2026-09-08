import { rolePrefix, sessionTitle, type Session } from "../runtime/types";

/**
 * Order harness-linked sessions as a parent/child tree.
 *
 * Port of `apps/tauri Roots (`parent_session_id === null`)
 * come first in creation order; each root's descendants follow depth-first,
 * sorted by `created_at` among siblings.
 *
 * A flat list would be wrong rather than merely plainer: the harness starts an
 * orchestrator and hangs its steps off it, and side by side those rows read as
 * five unrelated agents instead of one piece of work.
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

/** One harness-linked session row, shared by the feature tab and the panel. */
export type HarnessAgentRow = {
  session: Session;
  label: string;
  depth: number;
};

/**
 * Orchestrator plus its direct children, in rail order.
 *
 * One level deep on purpose: this answers "who is on this feature", and a
 * subagent's own subagents are a question the feature tab does not ask.
 */
export function harnessFeatureAgents(
  sessions: Session[],
  orchestratorSessionId: string | null,
): HarnessAgentRow[] {
  if (orchestratorSessionId === null) return [];
  const orchestrator = sessions.find((session) => session.id === orchestratorSessionId);
  if (!orchestrator) return [];

  const children = sessions
    .filter((session) => session.parent_session_id === orchestratorSessionId)
    .sort((left, right) => left.created_at.localeCompare(right.created_at));

  return [
    { session: orchestrator, label: sessionDisplayTitle(orchestrator), depth: 0 },
    ...children.map((session) => ({
      session,
      label: sessionDisplayTitle(session),
      depth: 1,
    })),
  ];
}

/**
 * Where the executor child is running, when there is one.
 *
 * The harness can implement in a worktree while the orchestrator sits in the
 * main checkout, and the feature tab says so — otherwise a diff that does not
 * move looks like a step that did nothing.
 */
export function harnessIsolationNote(
  sessions: Session[],
  workspaces: { id: string; kind: string; branch: string | null; display_name: string | null }[],
  orchestratorSessionId: string | null,
): string | null {
  if (orchestratorSessionId === null) return null;
  const executor = sessions.find(
    (session) => session.parent_session_id === orchestratorSessionId && session.role === "Executor",
  );
  if (!executor) return null;
  const workspace = workspaces.find((item) => item.id === executor.workspace_id);
  if (!workspace) return null;
  if (workspace.kind === "GitWorktree") {
    const label = workspace.display_name ?? workspace.branch ?? "worktree";
    return `Implementing in worktree · ${label}`;
  }
  return `Implementing in ${workspace.branch ?? "checkout"}`;
}
