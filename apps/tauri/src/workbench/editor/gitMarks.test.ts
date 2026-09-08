import { describe, expect, it } from "vitest";
import { gitMarksFor, patchFor } from "./gitMarks";

const patch = (body: string) => body.replace(/^\n/, "");

describe("gitMarksFor", () => {
  it("marks a pure insertion on the lines it inserted", () => {
    const marks = gitMarksFor(
      patch(`
@@ -1,2 +1,4 @@
 one
+two
+three
 four
`),
    );
    expect([...marks]).toEqual([
      [2, "added"],
      [3, "added"],
    ]);
  });

  it("calls a removal followed by an addition a modification, and marks it once", () => {
    const marks = gitMarksFor(
      patch(`
@@ -1,3 +1,3 @@
 one
-old
+new
 three
`),
    );
    expect([...marks]).toEqual([[2, "modified"]]);
  });

  it("marks a pure deletion on the line that closed the gap", () => {
    const marks = gitMarksFor(
      patch(`
@@ -1,3 +1,2 @@
 one
-gone
 three
`),
    );
    // `three` is line 2 of the new file, and it is where `gone` used to be.
    expect([...marks]).toEqual([[2, "deleted"]]);
  });

  it("marks a deletion at the end of a file on the last line there is", () => {
    const marks = gitMarksFor(
      patch(`
@@ -1,2 +1,1 @@
 one
-gone
`),
    );
    expect([...marks]).toEqual([[1, "deleted"]]);
  });

  it("keeps hunks apart, so a deletion does not leak across a gap", () => {
    const marks = gitMarksFor(
      patch(`
@@ -1,2 +1,1 @@
 one
-gone
@@ -20,2 +19,3 @@
 twenty
+new
`),
    );
    expect(marks.get(20)).toBe("added");
    expect(marks.get(19)).toBeUndefined();
  });

  it("returns nothing for a patch with no hunks", () => {
    expect(gitMarksFor("diff --git a/x b/x\nindex 1..2 100644\n").size).toBe(0);
  });
});

describe("patchFor", () => {
  const files = [
    { path: "a.ts", patch: "@@ -1 +1 @@", binary: false, truncated: false },
    { path: "logo.png", patch: "", binary: true, truncated: false },
    { path: "huge.ts", patch: "", binary: false, truncated: true },
  ];

  it("finds the patch for a path", () => {
    expect(patchFor(files, "a.ts")).toBe("@@ -1 +1 @@");
  });

  it("has nothing to say about a binary, a truncated patch, or an unchanged file", () => {
    // A dropped patch is not an unchanged file, but it is equally not a set of
    // line numbers — showing stripes derived from half a patch would be worse.
    expect(patchFor(files, "logo.png")).toBeNull();
    expect(patchFor(files, "huge.ts")).toBeNull();
    expect(patchFor(files, "never.ts")).toBeNull();
  });
});
