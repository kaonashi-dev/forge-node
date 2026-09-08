// D1: a unified patch, arranged for a virtualised renderer.
//
// `@codemirror/merge`'s `unifiedMergeView` was the plan's first answer and it
// wants two whole documents. The daemon sends neither: `DiffFile.patch` is
// `git diff` output — hunks with headers and no file around them — and
// reconstructing "before" and "after" from it produces two files with holes
// where the unchanged regions were, which is a different and worse thing to
// show. So the patch itself becomes the document, and CodeMirror virtualises
// *that*: same 50ms budget on a 2 000-line patch, same intra-line marking, and
// both line-number columns survive, which reconstruction would have lost.
//
// Pure and its own module: the line numbering is arithmetic with a right and a
// wrong answer, and being one off is invisible until someone quotes a line.

import { parsePatch, type PatchRow, type PatchRowKind } from "../patch";

export type PatchDocument = {
  /** The patch body as CodeMirror will hold it, one row per line. */
  text: string;
  rows: PatchRow[];
  /** Row indexes (0-based) that start a hunk, for `]c` / `[c`. */
  hunkStarts: number[];
  /** Widest line number on either side, so the gutters can be sized once. */
  gutterWidth: number;
};

export function patchDocument(patch: string): PatchDocument {
  const rows = parsePatch(patch);
  const hunkStarts: number[] = [];
  let widest = 0;
  rows.forEach((row, index) => {
    if (row.kind === "hunk") hunkStarts.push(index);
    widest = Math.max(widest, row.before ?? 0, row.after ?? 0);
  });
  return {
    // The marker column is drawn separately, so the text is the row's content
    // without its leading `+`/`-`/space — which is also what makes selecting a
    // block of the patch produce pasteable code rather than a patch.
    text: rows.map((row) => row.text).join("\n"),
    rows,
    hunkStarts,
    gutterWidth: String(widest).length,
  };
}

/** The CSS class a row's line carries. */
export function rowClass(kind: PatchRowKind): string {
  return `forge-diff-${kind}`;
}

/* --------------------------------------------------------- intra-line --- */

/** A range of one row's text that differs from its counterpart. */
export type Segment = { from: number; to: number };

/**
 * The part of two lines that actually differs.
 *
 * Common prefix and common suffix, and nothing cleverer. A full edit script
 * would mark more precisely and cost O(n·m) per line pair on a path that runs
 * for every changed line of every expanded file; prefix/suffix is O(n), and on
 * the edit a patch usually carries — a renamed identifier, a changed literal —
 * it lands on exactly the same span.
 *
 * Returns `null` when the whole line differs: marking every character is the
 * same as marking none, and the line class already says the line changed.
 */
export function intraLine(
  before: string,
  after: string,
): { before: Segment; after: Segment } | null {
  if (before === after) return null;
  const limit = Math.min(before.length, after.length);

  let prefix = 0;
  while (prefix < limit && before[prefix] === after[prefix]) prefix += 1;

  let suffix = 0;
  while (
    suffix < limit - prefix &&
    before[before.length - 1 - suffix] === after[after.length - 1 - suffix]
  ) {
    suffix += 1;
  }

  // The two lines have to be more alike than different for "the same line,
  // edited" to be a true description of them. `alpha` and `beta` share a
  // trailing `a` and nothing else; marking almost all of both says the same as
  // marking neither, and the row colour has already said the line changed.
  const common = prefix + suffix;
  if (common * 2 < Math.min(before.length, after.length)) return null;
  return {
    before: { from: prefix, to: before.length - suffix },
    after: { from: prefix, to: after.length - suffix },
  };
}

/**
 * Pair removed rows with the added rows that replaced them.
 *
 * A run of `-` immediately followed by a run of `+` is one edit as far as a
 * reader is concerned, and the pairing is positional within the run: the first
 * removed line against the first added line, and so on. A run with unequal
 * sides pairs as far as the shorter one goes and leaves the rest unmarked,
 * which is honest — there is no counterpart to compare them against.
 */
export function pairedRows(rows: PatchRow[]): Array<{ removed: number; added: number }> {
  const pairs: Array<{ removed: number; added: number }> = [];
  let index = 0;
  while (index < rows.length) {
    if (rows[index].kind !== "removed") {
      index += 1;
      continue;
    }
    const removedStart = index;
    while (index < rows.length && rows[index].kind === "removed") index += 1;
    const addedStart = index;
    while (index < rows.length && rows[index].kind === "added") index += 1;
    const count = Math.min(addedStart - removedStart, index - addedStart);
    for (let offset = 0; offset < count; offset += 1) {
      pairs.push({ removed: removedStart + offset, added: addedStart + offset });
    }
  }
  return pairs;
}

/**
 * The two columns of a split view, aligned.
 *
 * Both sides get one entry per visual row, and a `null` is a filler: an added
 * line has nothing opposite it on the left, and vice versa. Without the filler
 * the two panes drift apart by the net line count of every hunk above, which
 * is the one thing a split view exists to avoid.
 */
export function splitRows(
  rows: PatchRow[],
): Array<{ left: PatchRow | null; right: PatchRow | null }> {
  const out: Array<{ left: PatchRow | null; right: PatchRow | null }> = [];
  let index = 0;
  while (index < rows.length) {
    const row = rows[index];
    if (row.kind === "removed") {
      const removed: PatchRow[] = [];
      while (index < rows.length && rows[index].kind === "removed") removed.push(rows[index++]);
      const added: PatchRow[] = [];
      while (index < rows.length && rows[index].kind === "added") added.push(rows[index++]);
      for (let offset = 0; offset < Math.max(removed.length, added.length); offset += 1) {
        out.push({ left: removed[offset] ?? null, right: added[offset] ?? null });
      }
      continue;
    }
    if (row.kind === "added") {
      out.push({ left: null, right: row });
      index += 1;
      continue;
    }
    out.push({ left: row, right: row });
    index += 1;
  }
  return out;
}
