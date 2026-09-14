import type { FileTree } from "./types";

export function fileAffected(path: string, changed: string): boolean {
  return changed === "" || path === changed || path.startsWith(`${changed}/`);
}

/**
 * Whether a fresh listing says anything the one in hand did not.
 *
 * A watcher fires for every write, and the overwhelmingly common write — an
 * agent or an editor saving a file that already exists — produces a listing
 * identical to the one on screen. Storing it anyway replaces the tree, which
 * repaints every row and re-reads the folds, so the panel blinks once per
 * save with nothing to show for it. Order is part of the answer: the daemon
 * walks the checkout the same way twice, so a differing order is a real move.
 */
export function sameListing(current: FileTree | null, next: FileTree): boolean {
  if (!current) return false;
  if (
    current.workspace_id !== next.workspace_id ||
    current.truncated !== next.truncated ||
    current.entries.length !== next.entries.length
  ) {
    return false;
  }
  return current.entries.every((entry, at) => {
    const other = next.entries[at];
    return (
      entry.path === other.path && entry.kind === other.kind && entry.ignored === other.ignored
    );
  });
}
