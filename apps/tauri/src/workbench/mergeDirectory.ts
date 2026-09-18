import type { FileTree } from "./types";
import { parentPath } from "./pathOperations";

/** Preserve children of directories still present while their own reads refresh. */
export function retainExpandedDirectories(current: FileTree | null, next: FileTree): FileTree {
  if (!current || current.workspace_id !== next.workspace_id) return next;
  const directories = new Set(
    next.entries.filter((entry) => entry.kind === "Directory").map((entry) => entry.path),
  );
  const retained = current.entries.filter((entry) => {
    let path = parentPath(entry.path);
    while (path) {
      if (directories.has(path)) return true;
      path = parentPath(path);
    }
    return false;
  });
  const byPath = new Map(retained.map((entry) => [entry.path, entry]));
  for (const entry of next.entries) byPath.set(entry.path, entry);
  return {
    ...next,
    entries: [...byPath.values()].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0)),
  };
}

/** Replace immediate children; a partial answer cannot establish that a path disappeared. */
export function mergeDirectory(tree: FileTree, parent: string, listing: FileTree): FileTree {
  if (tree.workspace_id !== listing.workspace_id) return tree;
  const children = new Map(listing.entries.map((entry) => [entry.path, entry]));
  const entries = tree.entries.filter((entry) => {
    if (entry.path === parent) return true;
    if (parentPath(entry.path) === parent) return listing.truncated && !children.has(entry.path);
    const relative = parent
      ? entry.path.startsWith(`${parent}/`)
        ? entry.path.slice(parent.length + 1)
        : null
      : entry.path;
    if (relative === null) return true;
    const child = parent ? `${parent}/${relative.split("/")[0]}` : relative.split("/")[0];
    return listing.truncated || children.get(child)?.kind === "Directory";
  });
  entries.push(...listing.entries);
  const paths = new Set(
    entries.filter((entry) => entry.kind === "Directory").map((entry) => entry.path),
  );
  return {
    ...tree,
    entries: entries.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0)),
    loadedDirectories: [
      ...new Set([
        ...(tree.loadedDirectories ?? []).filter((path) => !path || paths.has(path)),
        parent,
      ]),
    ].sort(),
    truncated: tree.truncated || listing.truncated,
  };
}
