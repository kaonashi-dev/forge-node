import { describe, expect, it } from "vitest";
import { parsePatch } from "../patch";
import { intraLine, pairedRows, patchDocument, splitRows } from "./patchDocument";

const PATCH = `diff --git a/x.ts b/x.ts
@@ -1,4 +1,4 @@
 const a = 1;
-const b = 2;
+const b = 3;
 const c = 4;
`;

describe("patchDocument", () => {
  it("holds one document line per patch row", () => {
    const doc = patchDocument(PATCH);
    expect(doc.text.split("\n")).toHaveLength(doc.rows.length);
  });

  it("strips the +/- marker from the text, so a selection pastes as code", () => {
    const doc = patchDocument(PATCH);
    expect(doc.text).toContain("const b = 3;");
    expect(doc.text).not.toContain("+const b = 3;");
  });

  it("indexes the hunk headers for next/previous hunk", () => {
    const doc = patchDocument(PATCH);
    expect(doc.hunkStarts.map((index) => doc.rows[index].kind)).toEqual(["hunk"]);
  });

  it("sizes the gutter from the widest line number in the patch", () => {
    expect(patchDocument(PATCH).gutterWidth).toBe(1);
    expect(patchDocument(PATCH.replace("-1,4 +1,4", "-1200,4 +1200,4")).gutterWidth).toBe(4);
  });
});

describe("intraLine", () => {
  it("marks only the part that changed", () => {
    expect(intraLine("const b = 2;", "const b = 3;")).toEqual({
      before: { from: 10, to: 11 },
      after: { from: 10, to: 11 },
    });
  });

  it("handles an insertion with an empty span on the other side", () => {
    const spans = intraLine("ab", "axb");
    expect(spans).toEqual({ before: { from: 1, to: 1 }, after: { from: 1, to: 2 } });
  });

  it("says nothing for identical lines", () => {
    expect(intraLine("same", "same")).toBeNull();
  });

  it("says nothing when the lines are more different than alike", () => {
    // `alpha` and `beta` share only a trailing `a`. Marking four of five
    // characters says the same as marking none, and the row colour has
    // already said the line changed.
    expect(intraLine("alpha", "beta")).toBeNull();
  });
});

describe("pairedRows", () => {
  it("pairs a removal run with the addition run under it, positionally", () => {
    const rows = parsePatch(`@@ -1,4 +1,4 @@
-one
-two
+ONE
+TWO
`);
    expect(pairedRows(rows)).toEqual([
      { removed: 1, added: 3 },
      { removed: 2, added: 4 },
    ]);
  });

  it("leaves the surplus side of an uneven run unpaired", () => {
    const rows = parsePatch(`@@ -1,3 +1,2 @@
-one
-two
+ONE
`);
    expect(pairedRows(rows)).toHaveLength(1);
  });

  it("does not pair across a context line", () => {
    const rows = parsePatch(`@@ -1,4 +1,4 @@
-one
 keep
+ONE
`);
    expect(pairedRows(rows)).toEqual([]);
  });
});

describe("splitRows", () => {
  it("keeps the two sides aligned by padding the shorter run", () => {
    const rows = parsePatch(`@@ -1,3 +1,4 @@
 keep
-one
+ONE
+TWO
`);
    const split = splitRows(rows);
    // hunk header, context, then the paired edit, then the orphan addition.
    expect(split.map((pair) => [pair.left?.kind ?? null, pair.right?.kind ?? null])).toEqual([
      ["hunk", "hunk"],
      ["context", "context"],
      ["removed", "added"],
      [null, "added"],
    ]);
  });
});
