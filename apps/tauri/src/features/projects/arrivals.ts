/**
 * When each checkout arrived while the window was open.
 *
 * A workspace carries no creation time on the wire, so arrival is "an id the
 * store had not held since the first snapshot", stamped the moment the store
 * gains it. A card checks the stamp when it mounts, so one that first mounts
 * because its project was unfolded later does not play an entrance it missed.
 */
export const ARRIVAL_MS = 3000;

export function createArrivals() {
  const seen = new Map<string, number>();
  let primed = false;
  return {
    /** Records the ids in the store now; those present at the first snapshot never arrive. */
    note(ids: readonly string[], now: number): void {
      if (!primed) {
        if (ids.length === 0) return;
        for (const id of ids) seen.set(id, Number.NEGATIVE_INFINITY);
        primed = true;
        return;
      }
      for (const id of ids) if (!seen.has(id)) seen.set(id, now);
    },
    /** Whether `id` arrived within the last `ARRIVAL_MS`. */
    arrived(id: string, now: number): boolean {
      const at = seen.get(id);
      return at !== undefined && now - at < ARRIVAL_MS;
    },
  };
}

export const workspaceArrivals = createArrivals();
