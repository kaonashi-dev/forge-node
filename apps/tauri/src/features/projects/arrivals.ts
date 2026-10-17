/**
 * Which checkouts arrived while the window was open.
 *
 * A workspace carries no creation time on the wire, so arrival is "an id the
 * rail had not seen since the first snapshot". Folding a card remounts it; the
 * set keeps that from replaying the entrance.
 */
export function createArrivals() {
  const known = new Set<string>();
  let primed = false;
  return {
    /** Records the ids already there when the first snapshot lands. */
    prime(ids: readonly string[]): void {
      if (primed || ids.length === 0) return;
      for (const id of ids) known.add(id);
      primed = true;
    },
    /** True once per id, and only for one that arrived after priming. */
    arrived(id: string): boolean {
      if (!primed || known.has(id)) return false;
      known.add(id);
      return true;
    },
  };
}

export const workspaceArrivals = createArrivals();
