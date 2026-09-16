import type { FileEntry, FileTree } from "./types";

/** Keep expanded rows visible until the fresh ignored-directory reads arrive. */
export function retainExpandedDirectories(current: FileTree | null, next: FileTree): FileTree {
  if (!current || current.workspace_id !== next.workspace_id) return next;
  const placeholders = new Set(
    next.entries
      .filter((entry) => entry.kind === "Directory" && entry.ignored)
      .map((entry) => entry.path),
  );
  if (placeholders.size === 0) return next;
  const retained = current.entries.filter((entry) => {
    let cut = entry.path.lastIndexOf("/");
    while (cut >= 0) {
      if (placeholders.has(entry.path.slice(0, cut))) return true;
      cut = entry.path.lastIndexOf("/", cut - 1);
    }
    return false;
  });
  if (retained.length === 0) return next;
  const byPath = new Map(retained.map((entry) => [entry.path, entry]));
  for (const entry of next.entries) byPath.set(entry.path, entry);
  return {
    ...next,
    entries: [...byPath.values()].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0)),
  };
}

/** Replace children, retaining expanded ignored descendants until their reads arrive. */
export function mergeDirectory(tree: FileTree, parent: string, listing: FileTree): FileTree {
  listing = retainExpandedDirectories(tree, listing);
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
    loadedDirectories: [
      ...(tree.loadedDirectories ?? []).filter(
        (path) => path !== parent && !path.startsWith(prefix),
      ),
      parent,
    ].sort(),
    truncated: tree.truncated || listing.truncated,
  };
}
