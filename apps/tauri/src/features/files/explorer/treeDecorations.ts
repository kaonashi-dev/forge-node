// What git has to say about a path, arranged for a tree row. The source is the
// same `LoadDiff` answer the Diff tab renders, so the tree and the patch never
// disagree and nothing new is asked of the daemon to decorate a listing.

import type { DiffFile, DiffStatus, FileEntry } from "../../../contracts/workbench";

/** The letter a decorated row carries beside its tint, so the state is never colour alone. */
export type TreeMark = "A" | "M" | "D" | "R" | "U" | "C";

export type Decoration = {
  mark: TreeMark;
  /** The `--git-*-text` role the row is tinted with. */
  tone: "added" | "modified" | "deleted" | "untracked" | "conflict";
};

const BY_STATUS: Record<string, Decoration> = {
  Added: { mark: "A", tone: "added" },
  Modified: { mark: "M", tone: "modified" },
  Deleted: { mark: "D", tone: "deleted" },
  Renamed: { mark: "R", tone: "modified" },
  Untracked: { mark: "U", tone: "untracked" },
  Conflicted: { mark: "C", tone: "conflict" },
};

/**
 * A status the daemon sent that is not in the table above.
 *
 * `DiffStatus` is a `string` on the wire on purpose — git grows status codes —
 * so an unknown one is shown as modified rather than dropped: "something
 * happened to this file" is true and useful, and silence is not.
 */
const UNKNOWN: Decoration = { mark: "M", tone: "modified" };

export function decorationFor(status: DiffStatus): Decoration {
  return BY_STATUS[status] ?? UNKNOWN;
}

/** Every changed path, by path. */
export function fileDecorations(files: readonly DiffFile[]): Map<string, Decoration> {
  return new Map(files.map((file) => [file.path, decorationFor(file.status)]));
}

/**
 * How many changed files sit under each directory.
 *
 * Every ancestor of a changed path is counted, so a folded `src/` says how
 * much is hidden inside it rather than nothing at all. A path is counted once
 * per ancestor and never for itself: the file already carries its own mark.
 */
export function folderCounts(paths: Iterable<string>): Map<string, number> {
  const counts = new Map<string, number>();
  for (const path of paths) {
    const parts = path.split("/").filter(Boolean);
    // `length - 1` stops before the file's own name.
    for (let depth = 1; depth < parts.length; depth += 1) {
      const directory = parts.slice(0, depth).join("/");
      counts.set(directory, (counts.get(directory) ?? 0) + 1);
    }
  }
  return counts;
}

/**
 * The ignored entries whose parent is not itself ignored.
 *
 * Only these carry the word: inside an expanded `node_modules/` every row is
 * ignored, and repeating it on each would bury the names.
 */
export function ignoredRoots(entries: readonly FileEntry[]): string[] {
  const ignored = new Set<string>();
  for (const entry of entries) if (entry.ignored) ignored.add(entry.path);
  const roots: string[] = [];
  for (const path of ignored) {
    const cut = path.lastIndexOf("/");
    if (cut === -1 || !ignored.has(path.slice(0, cut))) roots.push(path);
  }
  return roots;
}
