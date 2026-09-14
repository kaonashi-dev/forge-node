import { describe, expect, it } from "vitest";
import { gitChangesFor, gitMarksFor, marksForChanges, patchFor } from "./gitMarks";

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

describe("gitChangesFor", () => {
  it("groups every replacement line under the same modified block", () => {
    const text = "@@ -1,3 +1,4 @@\n keep\n-old one\n-old two\n+new one\n+new two\n+extra\n";
    const changes = gitChangesFor(text);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toMatchObject({ from: 2, to: 4, kind: "modified" });
    expect([...gitMarksFor(text)]).toEqual([
      [2, "modified"],
      [3, "modified"],
      [4, "modified"],
    ]);
    expect(changes[0].rows.map((row) => row.text)).toEqual([
      "old one",
      "old two",
      "new one",
      "new two",
      "extra",
    ]);
  });

  it("separates changes across context within one hunk", () => {
    const changes = gitChangesFor("@@ -1,3 +1,4 @@\n-old\n+new\n keep\n+added\n tail\n");
    expect(changes.map(({ from, to, kind }) => ({ from, to, kind }))).toEqual([
      { from: 1, to: 1, kind: "modified" },
      { from: 3, to: 3, kind: "added" },
    ]);
  });

  it("anchors a deletion at the beginning on the first surviving line", () => {
    expect(gitChangesFor("@@ -1,2 +1 @@\n-gone\n kept\n")[0]).toMatchObject({
      from: 1,
      to: 1,
      kind: "deleted",
    });
  });

  it("keeps a deletion before a later hunk and uses its own line numbers", () => {
    const changes = gitChangesFor("@@ -1,2 +1 @@\n kept\n-gone\n@@ -20 +19 @@\n-old\n+new\n", 25);
    expect(changes.map((change) => change.from)).toEqual([2, 19]);
    expect(changes[0].rows[0].before).toBe(2);
  });

  it("anchors zero-context deletions after the hunk start", () => {
    expect(gitChangesFor("@@ -10,2 +9,0 @@\n-one\n-two\n", 20)[0].from).toBe(10);
  });

  it("clamps a final deletion to a non-newline-terminated document", () => {
    expect(gitChangesFor("@@ -1,2 +1 @@\n keep\n-gone\n", 1)[0].from).toBe(1);
  });

  it("keeps a clickable deletion when the document becomes empty", () => {
    const changes = gitChangesFor("@@ -1 +0,0 @@\n-gone\n", 1);
    expect(changes[0]).toMatchObject({ from: 1, to: 1, kind: "deleted" });
    expect([...marksForChanges(changes)]).toEqual([[1, "deleted"]]);
  });

  it("keeps newline metadata inside a replacement and highlights the changed span", () => {
    const changes = gitChangesFor(
      "@@ -1 +1 @@\n-the CodeMirror document\n\\ No newline at end of file\n+the plain-text editor\n\\ No newline at end of file\n",
    );
    expect(changes).toHaveLength(1);
    expect(changes[0].kind).toBe("modified");
    expect(changes[0].rows.filter((row) => row.kind === "meta")).toHaveLength(2);
    const replacement = gitChangesFor("@@ -1 +1 @@\n-const value = 1;\n+const value = 2;\n")[0];
    expect(replacement.rows[0].emphasis).toEqual({ from: 14, to: 15 });
    expect(replacement.rows[1].emphasis).toEqual({ from: 14, to: 15 });
  });

  it("does not treat file headers or an empty patch as a change", () => {
    expect(gitChangesFor("diff --git a/x b/x\n--- a/x\n+++ b/x\n")).toEqual([]);
    expect(gitChangesFor("")).toEqual([]);
  });
});
