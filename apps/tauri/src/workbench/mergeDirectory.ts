import type { FileEntry, FileTree } from "./types";

/**
 * Replace an opaque ignored directory with its peeled children.
 *
 * Drops the placeholder row and anything already listed under it, then appends
 * the one-level answer. An empty listing keeps the directory row so the folder
 * stays visible; the explorer's opened-ignored set is what makes that row
 * non-opaque after a successful peel.
 */
export function mergeDirectory(tree: FileTree, parent: string, listing: FileTree): FileTree {
  const prefix = `${parent}/`;
  const kept = tree.entries.filter(
    (entry) => entry.path !== parent && !entry.path.startsWith(prefix),
  );
  const children: FileEntry[] =
    listing.entries.length > 0
      ? [...listing.entries]
      : [{ path: parent, kind: "Directory", ignored: true }];
  const byPath = new Map<string, FileEntry>();
  for (const entry of kept) byPath.set(entry.path, entry);
  for (const entry of children) byPath.set(entry.path, entry);
  const entries = [...byPath.values()].sort((a, b) =>
    a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
  );
  return {
    ...tree,
    entries,
    truncated: tree.truncated || listing.truncated,
  };
}
