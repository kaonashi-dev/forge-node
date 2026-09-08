import { describe, expect, it } from "vitest";
import { parsePatch } from "./patch";

const patch = `diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,4 +10,5 @@ fn main() {
 let a = 1;
-let b = 2;
+let b = 3;
+let c = 4;
 let d = 5;
`;

describe("parsePatch", () => {
  it("keeps the header git writes above the first hunk", () => {
    const meta = parsePatch(patch).filter((row) => row.kind === "meta");
    expect(meta.map((row) => row.text)).toEqual([
      "diff --git a/src/lib.rs b/src/lib.rs",
      "index 111..222 100644",
      "--- a/src/lib.rs",
      "+++ b/src/lib.rs",
    ]);
  });

  // Counted from the hunk header, because the patch body carries no numbers.
  it("numbers each side from the hunk header", () => {
    const rows = parsePatch(patch).filter((row) => row.kind !== "meta" && row.kind !== "hunk");
    expect(rows.map((row) => [row.kind, row.before, row.after])).toEqual([
      ["context", 10, 10],
      ["removed", 11, null],
      ["added", null, 11],
      ["added", null, 12],
      ["context", 12, 13],
    ]);
  });

  it("strips the marker from the text it paints", () => {
    const added = parsePatch(patch).find((row) => row.kind === "added");
    expect(added?.text).toBe("let b = 3;");
  });

  // A truncated or unusual patch should show what it has rather than nothing.
  it("renders a body with no hunk header as meta", () => {
    const rows = parsePatch("Binary files differ\n");
    expect(rows).toEqual([
      { kind: "meta", text: "Binary files differ", before: null, after: null },
    ]);
  });

  it("has nothing to show for an empty patch", () => {
    expect(parsePatch("")).toEqual([]);
  });

  // It belongs to the row above and counts for neither side.
  it("treats the no-newline marker as meta", () => {
    const rows = parsePatch("@@ -1,1 +1,1 @@\n-a\n+b\n\\ No newline at end of file\n");
    expect(rows.at(-1)).toEqual({
      kind: "meta",
      text: "\\ No newline at end of file",
      before: null,
      after: null,
    });
  });

  it("does not invent a trailing line from the final newline", () => {
    const rows = parsePatch("@@ -1,1 +1,1 @@\n a\n");
    expect(rows.filter((row) => row.kind === "context")).toHaveLength(1);
  });
});
