import { createSignal } from "solid-js";

/** A boxed path, so asking for the same file twice is still a change. */
export type DiffReveal = { path: string };

const [pending, setPending] = createSignal<DiffReveal | null>(null);

/**
 * Asks the working-tree patch to scroll to `path`, now or when its tab opens.
 * Only `DiffView` reads this; it settles the request when it lands, when the
 * file is not in the patch, or when the view goes away.
 */
export function revealDiffFile(path: string): void {
  setPending({ path });
}

export const pendingDiffReveal = pending;

export function clearDiffReveal(request: DiffReveal): void {
  if (pending() === request) setPending(null);
}
