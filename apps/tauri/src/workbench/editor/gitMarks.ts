// Gutter marks and change details share the cached workspace patch.

import { parsePatch } from "../patch";
import { intraLine } from "../diff/patchDocument";
import type { GitChange } from "@forge-node/file-workbench/editor";
import type { GitMark, GitMarks } from "./createEditor";

export function gitMarksFor(patch: string): GitMarks {
  return marksForChanges(gitChangesFor(patch));
}

export function marksForChanges(changes: readonly GitChange[]): GitMarks {
  const marks = new Map<number, GitMark>();
  for (const change of changes) {
    for (let line = change.from; line <= change.to; line += 1) {
      if (change.kind !== "deleted" || !marks.has(line)) marks.set(line, change.kind);
    }
  }
  return marks;
}

/** `lineCount` clamps deletion anchors to the saved document, including an empty file. */
export function gitChangesFor(patch: string, lineCount?: number): GitChange[] {
  const changes: GitChange[] = [];
  let rows: GitChange["rows"] = [];
  let next = 1;
  let hunkEnd = 1;
  let inHunk = false;

  function flush(): void {
    if (!rows.length) return;
    const added = rows.filter((row) => row.kind === "added");
    const removed = rows.filter((row) => row.kind === "removed");
    if (!added.length && !removed.length) {
      rows = [];
      return;
    }
    const anchor = Math.max(1, Math.min(next, lineCount ?? hunkEnd));
    for (let index = 0; index < Math.min(added.length, removed.length); index += 1) {
      const spans = intraLine(removed[index].text, added[index].text);
      if (spans) {
        removed[index].emphasis = spans.before;
        added[index].emphasis = spans.after;
      }
    }
    changes.push({
      from: added[0]?.after ?? anchor,
      to: added.at(-1)?.after ?? anchor,
      kind: added.length ? (removed.length ? "modified" : "added") : "deleted",
      rows,
    });
    rows = [];
  }

  for (const row of parsePatch(patch)) {
    if (row.kind === "hunk") {
      flush();
      const header = /\+(\d+)(?:,(\d+))? @@/.exec(row.text);
      const start = Number(header?.[1] ?? 1);
      const count = Number(header?.[2] ?? 1);
      next = count === 0 ? start + 1 : start;
      hunkEnd = Math.max(1, start + count - 1);
      inHunk = true;
    } else if (row.kind === "context") {
      flush();
      next = (row.after ?? next) + 1;
    } else if (inHunk && (row.kind === "added" || row.kind === "removed")) {
      rows.push({ ...row, kind: row.kind });
      if (row.after !== null) next = row.after + 1;
    } else if (rows.length && row.text.startsWith("\\")) {
      rows.push({ ...row, kind: "meta" });
    }
  }
  flush();
  return changes;
}

/** The patch for one path in a workspace diff, or `null`. */
export function patchFor(
  files: ReadonlyArray<{ path: string; patch: string; binary: boolean; truncated: boolean }>,
  path: string,
): string | null {
  const file = files.find((entry) => entry.path === path);
  if (!file || file.binary || file.truncated) return null;
  return file.patch;
}
