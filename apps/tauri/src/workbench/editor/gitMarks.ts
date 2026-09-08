// A5: which lines of the working copy a patch touched.
//
// Derived from the same `git diff` the Diff tab renders — the daemon is asked
// once and both surfaces read the answer, so the gutter and the patch can
// never disagree about what changed.
//
// Its own module because the mapping has a right and a wrong answer and the
// wrong one is invisible: a stripe one line off is worse than no stripe.

import { parsePatch } from "../patch";
import type { GitMark, GitMarks } from "./createEditor";

/**
 * Mark every line of the *new* file the patch reaches.
 *
 * Three cases and one subtlety. An added line is marked where it now is. A run
 * of removals with no additions beside it has no line of its own in the new
 * file, so it is marked on the line that now follows the deletion — the only
 * place a reader can be shown that something used to be there. A removal
 * immediately followed by additions is a modification, and the additions carry
 * the mark, because marking both would draw two stripes for one edit.
 */
export function gitMarksFor(patch: string): GitMarks {
  const marks = new Map<number, GitMark>();
  const rows = parsePatch(patch);

  /** Lines removed since the last row that had a place in the new file. */
  let pendingDeletion = false;
  /** The new-file line number the next row would occupy. */
  let next = 1;

  for (const row of rows) {
    if (row.kind === "added" && row.after !== null) {
      // A removal directly above an addition is one edit, not two.
      marks.set(row.after, pendingDeletion ? "modified" : "added");
      pendingDeletion = false;
      next = row.after + 1;
      continue;
    }
    if (row.kind === "removed") {
      pendingDeletion = true;
      continue;
    }
    if (row.kind === "context" && row.after !== null) {
      // A deletion with nothing added in its place: the gap closed, and the
      // line that closed it is where it is shown. `deleted` never overwrites a
      // mark already on that line — an addition there says more.
      if (pendingDeletion && !marks.has(row.after)) marks.set(row.after, "deleted");
      pendingDeletion = false;
      next = row.after + 1;
      continue;
    }
    if (row.kind === "hunk") pendingDeletion = false;
  }

  // A file that ends with a deletion has no following line; the mark goes on
  // the last line there is.
  if (pendingDeletion && next > 1 && !marks.has(next - 1)) marks.set(next - 1, "deleted");
  return marks;
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
