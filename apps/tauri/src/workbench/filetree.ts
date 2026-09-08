// Turning a flat `FileTree` into the rows a panel paints.
//
// The daemon answers with files only. Directories are synthesized here so the
// wire contract stays flat and the GUI can use the same fold semantics as the

import type { FileTree } from "./types";

type Node = {
  dirs: Map<string, Node>;
  files: string[];
};

export type TreeRow = {
  /** How many path segments deep, for the indent. */
  depth: number;
  /** The text painted in the row. */
  label: string;
  /** The full path used for identity, folding, and opening. */
  path: string;
  isFile: boolean;
  /** Resolved while building so rendering does not read the fold set. */
  folded: boolean;
};

function emptyNode(): Node {
  return { dirs: new Map(), files: [] };
}

/**
 * Flatten the daemon's files-only listing into visible tree rows.
 *
 * Directories are open unless their full path is in `collapsed`. A directory
 * chain with no files and one child is represented by one row, matching the
 * build the tree for the Files panel.
 */
export function treeRows(tree: FileTree | null, collapsed: Set<string>): TreeRow[] {
  if (!tree) return [];

  const root = emptyNode();
  for (const entry of tree.entries) {
    const parts = entry.path.split("/").filter(Boolean);
    const name = parts.pop();
    if (!name) continue;

    let node = root;
    for (const part of parts) {
      let child = node.dirs.get(part);
      if (!child) {
        child = emptyNode();
        node.dirs.set(part, child);
      }
      node = child;
    }

    if (entry.kind === "Directory") {
      if (!node.dirs.has(name)) node.dirs.set(name, emptyNode());
    } else {
      node.files.push(name);
    }
  }

  const rows: TreeRow[] = [];
  flatten(root, "", 0, collapsed, rows);
  return rows;
}

function compareNames(a: string, b: string): number {
  const lowerA = a.toLowerCase();
  const lowerB = b.toLowerCase();
  if (lowerA < lowerB) return -1;
  if (lowerA > lowerB) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

function flatten(
  node: Node,
  prefix: string,
  depth: number,
  collapsed: Set<string>,
  rows: TreeRow[],
): void {
  const dirs = [...node.dirs.entries()].sort(([a], [b]) => compareNames(a, b));
  for (const [name, initialChild] of dirs) {
    let label = name;
    let path = join(prefix, name);
    let child = initialChild;
    while (child.files.length === 0 && child.dirs.size === 1) {
      const [only, next] = child.dirs.entries().next().value as [string, Node];
      label = `${label}/${only}`;
      path = join(path, only);
      child = next;
    }

    const folded = collapsed.has(path);
    rows.push({ depth, label, path, isFile: false, folded });
    if (!folded) flatten(child, path, depth + 1, collapsed, rows);
  }

  for (const name of [...node.files].sort(compareNames)) {
    rows.push({
      depth,
      label: name,
      path: join(prefix, name),
      isFile: true,
      folded: false,
    });
  }
}

function join(prefix: string, name: string): string {
  return prefix ? `${prefix}/${name}` : name;
}

/**
 * Every directory path in the listing, chain ends and intermediates alike.
 *
 * The daemon answers with files only, so a directory exists here as a prefix
 * of something else — which is why this walks the segments of every entry
 * rather than filtering for `Directory` rows. Both spellings are returned on
 * purpose: `treeRows` folds a single-child chain into one row keyed by its
 * *end* (`.cursor/skills`), and the fold set is consulted by row path.
 */
export function directoryPaths(tree: FileTree | null): string[] {
  if (!tree) return [];
  const dirs = new Set<string>();
  for (const entry of tree.entries) {
    const parts = entry.path.split("/").filter(Boolean);
    // A file contributes its parents; a directory contributes itself as well.
    const upto = entry.kind === "Directory" ? parts.length : parts.length - 1;
    let prefix = "";
    for (let i = 0; i < upto; i += 1) {
      prefix = prefix ? `${prefix}/${parts[i]}` : parts[i];
      dirs.add(prefix);
    }
  }
  return [...dirs];
}

/**
 * Fold the directories this panel has not seen before.
 *
 * A tree opens closed: `ListFiles` answers with the whole checkout, and a
 * repository with a `.claude/skills/*` tree under it filled the panel with
 * three hundred rows before a person had asked for one of them.
 *
 * Written against a `seen` set rather than "collapse everything on reload"
 * because the tree is re-read after a refresh and after a write: re-seeding
 * from scratch would shut every folder the person had just opened. A folder
 * that appears later is new, so it arrives folded like the rest.
 */
export function foldUnseen(
  collapsed: Set<string>,
  seen: Set<string>,
  dirs: string[],
): { collapsed: Set<string>; seen: Set<string> } {
  const fresh = dirs.filter((dir) => !seen.has(dir));
  // The *same* sets back when there is nothing new, so a caller can compare by
  // identity and skip the write. Returning fresh copies unconditionally is
  // what turned this into an infinite loop the first time: a signal set to a
  // new `Set` with equal contents is still a change, so the effect that wrote
  // it woke itself, forever.
  if (fresh.length === 0) return { collapsed, seen };

  const nextCollapsed = new Set(collapsed);
  const nextSeen = new Set(seen);
  for (const dir of fresh) {
    nextSeen.add(dir);
    nextCollapsed.add(dir);
  }
  return { collapsed: nextCollapsed, seen: nextSeen };
}

function parentOf(path: string): string | null {
  const cut = path.lastIndexOf("/");
  return cut < 0 ? null : path.slice(0, cut);
}

/** Fold an open directory, or select the parent when already folded/a file. */
export function collapseTarget(
  row: TreeRow | undefined,
  collapsed: Set<string>,
): { fold: string } | { select: string } | null {
  if (!row) return null;
  if (!row.isFile && !collapsed.has(row.path)) return { fold: row.path };
  const parent = parentOf(row.path);
  return parent === null ? null : { select: parent };
}

/** Unfold a folded directory, or select its first visible child. */
export function expandTarget(
  row: TreeRow | undefined,
  collapsed: Set<string>,
  rows: TreeRow[],
  index: number,
): { unfold: string } | { select: string } | null {
  if (!row || row.isFile) return null;
  if (collapsed.has(row.path)) return { unfold: row.path };
  const next = rows[index + 1];
  return next && next.path.startsWith(`${row.path}/`) ? { select: next.path } : null;
}

/**
 * Narrow a listing to the paths that match `query` (§4.2 U8).
 *
 * Case-insensitive substring over the whole path, not a fuzzy match: the
 * palette is where fuzzy belongs, and a *filter* that reorders and re-scores
 * the tree under the person's fingers is a different tool. Typing `store`
 * should leave `src/store/` in place with its neighbours gone, which substring
 * does and scoring does not.
 *
 * The tree returned keeps `truncated`, because filtering a partial listing
 * still yields a partial answer and hiding that would be a lie.
 */
export function filterTree(tree: FileTree | null, query: string): FileTree | null {
  if (!tree) return null;
  const needle = query.trim().toLowerCase();
  if (needle === "") return tree;
  return {
    ...tree,
    entries: tree.entries.filter((entry) => entry.path.toLowerCase().includes(needle)),
  };
}
