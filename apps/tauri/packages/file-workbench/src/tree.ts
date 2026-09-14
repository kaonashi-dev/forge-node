// Flat listings become directory rows; filtering preserves the source listing budget.

export type FileEntry = { path: string; kind: string; ignored: boolean };
export type FileTree = { entries: readonly FileEntry[]; truncated: boolean };

type Node = {
  dirs: Map<string, Node>;
  files: { name: string; ignored: boolean }[];

  opaque: boolean;
};

export type TreeRow = {
  depth: number;

  label: string;

  path: string;
  isFile: boolean;

  folded: boolean;

  ignored: boolean;

  opaque: boolean;
};

function emptyNode(): Node {
  return { dirs: new Map(), files: [], opaque: false };
}

function allIgnored(node: Node): boolean {
  if (node.opaque) return true;
  if (node.files.length === 0 && node.dirs.size === 0) return false;
  return (
    node.files.every((file) => file.ignored) &&
    [...node.dirs.values()].every((dir) => allIgnored(dir))
  );
}

export function treeRows<T extends FileTree>(
  tree: T | null,
  collapsed: Set<string>,
  openedIgnored: ReadonlySet<string> = new Set(),
): TreeRow[] {
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
      const child = node.dirs.get(name) ?? emptyNode();
      if (entry.ignored) child.opaque = true;
      node.dirs.set(name, child);
    } else {
      node.files.push({ name, ignored: entry.ignored });
    }
  }

  const rows: TreeRow[] = [];
  flatten(root, "", 0, collapsed, openedIgnored, rows);
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
  openedIgnored: ReadonlySet<string>,
  rows: TreeRow[],
): void {
  const dirs = [...node.dirs.entries()].sort(([a], [b]) => compareNames(a, b));
  for (const [name, initialChild] of dirs) {
    let label = name;
    let path = join(prefix, name);
    let child = initialChild;
    while (!child.opaque && child.files.length === 0 && child.dirs.size === 1) {
      const [only, next] = child.dirs.entries().next().value as [string, Node];
      label = `${label}/${only}`;
      path = join(path, only);
      child = next;
    }

    // Children listed under an ignored directory clear opacity; an explicit
    // peel of an empty folder uses `openedIgnored` for the same.
    const opaque =
      child.opaque && child.files.length === 0 && child.dirs.size === 0 && !openedIgnored.has(path);
    const folded = opaque || collapsed.has(path);
    rows.push({
      depth,
      label,
      path,
      isFile: false,
      folded,
      ignored: allIgnored(child),
      opaque,
    });
    if (!folded) flatten(child, path, depth + 1, collapsed, openedIgnored, rows);
  }

  for (const file of [...node.files].sort((a, b) => compareNames(a.name, b.name))) {
    rows.push({
      depth,
      label: file.name,
      path: join(prefix, file.name),
      isFile: true,
      folded: false,
      ignored: file.ignored,
      opaque: false,
    });
  }
}

function join(prefix: string, name: string): string {
  return prefix ? `${prefix}/${name}` : name;
}

export function directoryPaths<T extends FileTree>(tree: T | null): string[] {
  if (!tree) return [];
  const dirs = new Set<string>();
  for (const entry of tree.entries) {
    const parts = entry.path.split("/").filter(Boolean);
    const upto = entry.kind === "Directory" && !entry.ignored ? parts.length : parts.length - 1;
    let prefix = "";
    for (let i = 0; i < upto; i += 1) {
      prefix = prefix ? `${prefix}/${parts[i]}` : parts[i];
      dirs.add(prefix);
    }
  }
  return [...dirs];
}

export function foldUnseen(
  collapsed: Set<string>,
  seen: Set<string>,
  dirs: string[],
): { collapsed: Set<string>; seen: Set<string> } {
  const fresh = dirs.filter((dir) => !seen.has(dir));
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

export function collapseTarget(
  row: TreeRow | undefined,
  collapsed: Set<string>,
): { fold: string } | { select: string } | null {
  if (!row) return null;
  if (!row.isFile && !row.opaque && !collapsed.has(row.path)) return { fold: row.path };
  const parent = parentOf(row.path);
  return parent === null ? null : { select: parent };
}

export function expandTarget(
  row: TreeRow | undefined,
  collapsed: Set<string>,
  rows: TreeRow[],
  index: number,
): { unfold: string } | { select: string } | null {
  if (!row || row.isFile || row.opaque) return null;
  if (collapsed.has(row.path)) return { unfold: row.path };
  const next = rows[index + 1];
  return next && next.path.startsWith(`${row.path}/`) ? { select: next.path } : null;
}

export function filterTree<T extends FileTree>(tree: T | null, query: string): T | null {
  if (!tree) return null;
  const needle = query.trim().toLowerCase();
  if (needle === "") return tree;
  return {
    ...tree,
    entries: tree.entries.filter((entry) => entry.path.toLowerCase().includes(needle)),
  };
}
