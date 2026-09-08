// Reading a unified diff into rows to paint.
//
// The daemon produces the patch with `git diff`; nothing here re-derives it.
// The only job is turning the text into rows that carry their own line numbers,
// because a diff without them is unusable for talking about a change.

export type PatchRowKind = "context" | "added" | "removed" | "hunk" | "meta";

export type PatchRow = {
  kind: PatchRowKind;
  text: string;
  /** Line number on the left (old file), when the row has one. */
  before: number | null;
  /** Line number on the right (new file), when the row has one. */
  after: number | null;
};

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/**
 * Parse a unified diff.
 *
 * Line numbers are counted from each hunk header rather than trusted from the
 * patch body, which carries none. A patch with no header at all still renders —
 * as context with no numbers — because a truncated or unusual patch should show
 * what it has instead of nothing.
 */
export function parsePatch(patch: string): PatchRow[] {
  const rows: PatchRow[] = [];
  let before = 0;
  let after = 0;
  let inHunk = false;

  for (const line of patch.split("\n")) {
    const hunk = HUNK.exec(line);
    if (hunk) {
      before = Number.parseInt(hunk[1], 10);
      after = Number.parseInt(hunk[2], 10);
      inHunk = true;
      rows.push({ kind: "hunk", text: line, before: null, after: null });
      continue;
    }
    if (!inHunk) {
      // `diff --git`, `index`, `---`, `+++`: the header git writes above the
      // first hunk. Kept rather than dropped: a rename or a mode change lives
      // there and nowhere else.
      if (line !== "") rows.push({ kind: "meta", text: line, before: null, after: null });
      continue;
    }
    // "\ No newline at end of file" belongs to the row above it and counts for
    // neither side.
    if (line.startsWith("\\")) {
      rows.push({ kind: "meta", text: line, before: null, after: null });
      continue;
    }
    const marker = line.slice(0, 1);
    const text = line.slice(1);
    if (marker === "+") {
      rows.push({ kind: "added", text, before: null, after });
      after += 1;
    } else if (marker === "-") {
      rows.push({ kind: "removed", text, before, after: null });
      before += 1;
    } else if (marker === " " || line === "") {
      rows.push({ kind: "context", text, before, after });
      before += 1;
      after += 1;
    } else {
      rows.push({ kind: "meta", text: line, before: null, after: null });
    }
  }
  // A patch ends with a newline, which splits into a trailing empty row that
  // is not a line of the file.
  if (rows.at(-1)?.kind === "context" && rows.at(-1)?.text === "") rows.pop();
  return rows;
}
