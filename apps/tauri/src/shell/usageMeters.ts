export type UsageTone = "normal" | "warning" | "full" | "stale";

const STALE_AFTER_MS = 600_000;

export function usagePercent(value: number): number {
  return Math.max(0, Math.min(100, value));
}

export function usageTone(percent: number, stale: boolean): UsageTone {
  if (stale) return "stale";
  if (percent >= 90) return "full";
  if (percent >= 75) return "warning";
  return "normal";
}

export function usageIsStale(collectedAt: string, now = Date.now()): boolean {
  const collected = Date.parse(collectedAt);
  return !Number.isFinite(collected) || now - collected > STALE_AFTER_MS;
}

export function usageResetLabel(resetsAt: string | null, now = Date.now()): string | null {
  if (!resetsAt) return null;
  const remaining = Date.parse(resetsAt) - now;
  if (!Number.isFinite(remaining) || remaining <= 0) return null;

  const minutes = Math.floor(remaining / 60_000);
  return `resets in ${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

export function usageUpdatedLabel(collectedAt: string, now = Date.now()): string {
  const elapsed = now - Date.parse(collectedAt);
  if (!Number.isFinite(elapsed) || elapsed < 0) return "Updated just now";
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return "Updated just now";
  if (minutes < 60) return `Updated ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  if (hours < 24) return `Updated ${hours}h ${remainder}m ago`;
  const days = Math.floor(hours / 24);
  return `Updated ${days}d ago`;
}

export function usageShortResetLabel(resetsAt: string | null, now = Date.now()): string | null {
  const label = usageResetLabel(resetsAt, now);
  if (!label) return null;
  return label.replace("resets in ", "");
}
