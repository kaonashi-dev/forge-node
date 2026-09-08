/** Checkouts the rail can reorder: the main repo and each worktree. */
export type OrderableCheckout = {
  id: string;
  label: string;
  worktree: boolean;
};

export type WorkspaceOrderMap = Record<string, string[]>;

/** Main checkout first, then worktrees by name — the default before anyone drags. */
export function defaultWorkspaceSort<T extends OrderableCheckout>(workspaces: T[]): T[] {
  return [...workspaces].sort((left, right) =>
    left.worktree === right.worktree
      ? left.label.localeCompare(right.label)
      : Number(left.worktree) - Number(right.worktree),
  );
}

/** Apply a persisted checkout order, appending anything new in the default sort. */
export function orderWorkspaces<T extends OrderableCheckout>(
  workspaces: T[],
  order: string[],
): T[] {
  const fallback = defaultWorkspaceSort(workspaces);
  if (order.length === 0) return fallback;
  const byId = new Map(fallback.map((item) => [item.id, item]));
  const ordered: T[] = [];
  for (const id of order) {
    const item = byId.get(id);
    if (item) {
      ordered.push(item);
      byId.delete(id);
    }
  }
  for (const item of fallback) {
    if (byId.has(item.id)) ordered.push(item);
  }
  return ordered;
}

export function orderFromWorkspaces<T extends OrderableCheckout>(
  workspaces: T[],
  prior: string[],
): string[] {
  return orderWorkspaces(workspaces, prior).map((item) => item.id);
}

export function parseWorkspaceOrder(raw: string | undefined): WorkspaceOrderMap {
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: WorkspaceOrderMap = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (Array.isArray(value)) {
        out[key] = value.filter((id): id is string => typeof id === "string");
      }
    }
    return out;
  } catch {
    return {};
  }
}
