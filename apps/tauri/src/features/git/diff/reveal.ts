import { createSignal } from "solid-js";

/** A boxed path, so asking for the same file twice is still a change. */
export type DiffReveal = { path: string };

const [pending, setPending] = createSignal<DiffReveal | null>(null);

/** The patch scrolls to `path` once it is mounted, now or when the tab opens. */
export function revealDiffFile(path: string): void {
  setPending({ path });
}

export const pendingDiffReveal = pending;

export function clearDiffReveal(request: DiffReveal): void {
  if (pending() === request) setPending(null);
}

export function diffFileAnchor(path: string): string {
  return `diff-file-${encodeURIComponent(path)}`;
}
