/**
 * Which rail rows are folded, as the one blob `app_state` stores (§15.2).
 *
 * Split out of `Sidebar.tsx` for the same reason `workspaceOrder.ts` is: the
 * component cannot be tested, and the part of folding that can go wrong is the
 * round trip through a string — an id dropped on the way out, or a payload
 * from an older build read back as "everything open".
 */

/** Read the stored set. Anything unparseable reads as nothing folded. */
export function parseFolds(raw: string | undefined): Set<string> {
  if (!raw) return new Set();
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? new Set(parsed.filter((id): id is string => typeof id === "string"))
      : new Set();
  } catch {
    return new Set();
  }
}

/**
 * The payload to store.
 *
 * Sorted, so folding the same two rows in either order writes the same string
 * — the daemon is written to on every fold, and an unstable payload would make
 * every one of them a change.
 */
export function foldsPayload(folds: Set<string>): string {
  return JSON.stringify([...folds].sort());
}

/** Fold an open row, or open a folded one. Returns a new set; never mutates. */
export function toggleFold(folds: Set<string>, id: string): Set<string> {
  const next = new Set(folds);
  if (!next.delete(id)) next.add(id);
  return next;
}
