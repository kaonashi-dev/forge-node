// Elapsed time in the two spellings the chrome uses.
//
// Minutes are the finest unit on purpose: activity reaches the GUI coalesced
// to one broadcast every 30 s, so a seconds digit would be precision the
// replica does not have.

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** `4m`, `1h 12m`, `2d 3h`; under a minute is `<1m`. */
export function elapsedLabel(ms: number): string {
  const safe = Math.max(0, ms);
  if (safe < MINUTE) return "<1m";
  if (safe < HOUR) return `${Math.floor(safe / MINUTE)}m`;
  if (safe < DAY) {
    const minutes = Math.floor((safe % HOUR) / MINUTE);
    return minutes > 0 ? `${Math.floor(safe / HOUR)}h ${minutes}m` : `${Math.floor(safe / HOUR)}h`;
  }
  const hours = Math.floor((safe % DAY) / HOUR);
  return hours > 0 ? `${Math.floor(safe / DAY)}d ${hours}h` : `${Math.floor(safe / DAY)}d`;
}

/** Milliseconds since an ISO stamp, or `null` when it does not parse. */
export function since(stamp: string | undefined | null, now: number): number | null {
  if (!stamp) return null;
  const then = Date.parse(stamp);
  return Number.isNaN(then) ? null : Math.max(0, now - then);
}
