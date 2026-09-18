import type { FileTree } from "./types";

export function fileAffected(path: string, changed: string): boolean {
  return changed === "" || path === changed || path.startsWith(`${changed}/`);
}

/** Stable answers retain their identity so content-only writes do not repaint rows. */
export function sameListing(current: FileTree | null, next: FileTree): boolean {
  if (!current) return false;
  const before = current.loadedDirectories ?? [];
  const after = next.loadedDirectories ?? [];
  if (
    before.length !== after.length ||
    before.some((path, index) => path !== after[index]) ||
    current.workspace_id !== next.workspace_id ||
    current.truncated !== next.truncated ||
    current.entries.length !== next.entries.length
  ) {
    return false;
  }
  return current.entries.every((entry, at) => {
    const other = next.entries[at];
    return (
      entry.path === other.path &&
      entry.kind === other.kind &&
      entry.ignored === other.ignored &&
      (entry.symlink ?? null) === (other.symlink ?? null)
    );
  });
}
