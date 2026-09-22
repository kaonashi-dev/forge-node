// Conflict regions read back out of a working-tree patch.
//
// A stopped replay leaves `<<<<<<<` / `|||||||` / `=======` / `>>>>>>>` in the
// file, and `git diff HEAD` carries them as ordinary lines of the new side. So
// the three-way view is a reading of the diff the panel already has, never a
// read of the index stages the daemon does not expose.

import { parsePatch, type PatchRow } from "./diff/patch";

export type ConflictBlock = {
  /** Working-tree line of the opening `<<<<<<<` marker. */
  line: number | null;
  ours: string[];
  /** `null` when git wrote the conflict without diff3 markers. */
  base: string[] | null;
  theirs: string[];
  /** False when the patch's context skipped lines inside the region. */
  complete: boolean;
};

const OPEN = /^<{7}(?: |$)/;
const BASE = /^\|{7}(?: |$)/;
const SPLIT = /^={7}$/;
const CLOSE = /^>{7}(?: |$)/;

type Section = "ours" | "base" | "theirs";

export function conflictBlocks(rows: readonly PatchRow[]): ConflictBlock[] {
  const blocks: ConflictBlock[] = [];
  let open: (ConflictBlock & { section: Section; next: number | null }) | null = null;

  for (const row of rows) {
    if (row.kind === "hunk") {
      if (open) open.complete = false;
      continue;
    }
    if (row.after === null) continue;
    if (open && open.next !== null && row.after !== open.next) open.complete = false;

    if (OPEN.test(row.text)) {
      if (open) blocks.push(finish(open, false));
      open = {
        line: row.after,
        ours: [],
        base: null,
        theirs: [],
        complete: true,
        section: "ours",
        next: row.after + 1,
      };
      continue;
    }
    if (!open) continue;
    open.next = row.after + 1;

    if (BASE.test(row.text) && open.section === "ours") {
      open.section = "base";
      open.base = [];
    } else if (SPLIT.test(row.text) && open.section !== "theirs") {
      open.section = "theirs";
    } else if (CLOSE.test(row.text) && open.section === "theirs") {
      blocks.push(finish(open, open.complete));
      open = null;
    } else if (open.section === "ours") {
      open.ours.push(row.text);
    } else if (open.section === "base") {
      open.base?.push(row.text);
    } else {
      open.theirs.push(row.text);
    }
  }
  if (open) blocks.push(finish(open, false));
  return blocks;
}

function finish(
  block: ConflictBlock & { section: Section; next: number | null },
  complete: boolean,
): ConflictBlock {
  return {
    line: block.line,
    ours: block.ours,
    base: block.base,
    theirs: block.theirs,
    complete,
  };
}

export function conflictBlocksOf(patch: string): ConflictBlock[] {
  return conflictBlocks(parsePatch(patch));
}
