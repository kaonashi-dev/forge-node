import { describe, expect, it } from "vitest";
import { mergeDirectory } from "./mergeDirectory";
import type { FileTree } from "./types";

const root = (entries: FileTree["entries"]): FileTree => ({
  workspace_id: "w1",
  truncated: false,
  entries,
});

describe("mergeDirectory", () => {
  it("replaces an opaque directory with its children", () => {
    const tree = root([
      { path: "app.js", kind: "File", ignored: false },
      { path: "plan", kind: "Directory", ignored: true },
    ]);
    const next = mergeDirectory(
      tree,
      "plan",
      root([
        { path: "plan/nested", kind: "Directory", ignored: true },
        { path: "plan/notes.md", kind: "File", ignored: true },
      ]),
    );
    expect(next.entries).toEqual([
      { path: "app.js", kind: "File", ignored: false },
      { path: "plan/nested", kind: "Directory", ignored: true },
      { path: "plan/notes.md", kind: "File", ignored: true },
    ]);
  });

  it("keeps an empty peeled directory visible", () => {
    const tree = root([{ path: "plan", kind: "Directory", ignored: true }]);
    const next = mergeDirectory(tree, "plan", root([]));
    expect(next.entries).toEqual([{ path: "plan", kind: "Directory", ignored: true }]);
  });

  it("or-s the truncated flag from the peel", () => {
    const tree = root([{ path: "dist", kind: "Directory", ignored: true }]);
    const next = mergeDirectory(tree, "dist", {
      workspace_id: "w1",
      truncated: true,
      entries: [{ path: "dist/a.js", kind: "File", ignored: true }],
    });
    expect(next.truncated).toBe(true);
  });
});
