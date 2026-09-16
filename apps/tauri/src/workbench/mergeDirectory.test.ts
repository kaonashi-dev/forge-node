import { describe, expect, it } from "vitest";
import { mergeDirectory, retainExpandedDirectories } from "./mergeDirectory";
import { treeRows, unloadedDirectories, watchDirectories } from "./filetree";
import { sameListing } from "./fileInvalidation";
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
    expect(next.loadedDirectories).toEqual(["plan"]);
    expect(sameListing(next, tree)).toBe(false);
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

  it("refreshes expanded ignored folders without dropping their watches between answers", () => {
    const listing = root([{ path: "plan", kind: "Directory", ignored: true }]);
    let tree = mergeDirectory(
      listing,
      "plan",
      root([
        { path: "plan/old.md", kind: "File", ignored: true },
        { path: "plan/nested", kind: "Directory", ignored: true },
      ]),
    );
    tree = mergeDirectory(
      tree,
      "plan/nested",
      root([{ path: "plan/nested/old.md", kind: "File", ignored: true }]),
    );
    const opened = new Set(["plan", "plan/nested"]);
    const rows = (next: FileTree) => treeRows(next, new Set(), opened);
    const watches = watchDirectories(rows(tree));

    tree = retainExpandedDirectories(tree, listing);
    expect(watchDirectories(rows(tree))).toEqual(watches);
    expect(unloadedDirectories(tree, rows(tree), opened)).toEqual(["plan"]);

    tree = mergeDirectory(
      tree,
      "plan",
      root([
        { path: "plan/new.md", kind: "File", ignored: true },
        { path: "plan/nested", kind: "Directory", ignored: true },
      ]),
    );
    expect(watchDirectories(rows(tree))).toEqual(watches);
    expect(tree.entries.some((entry) => entry.path === "plan/old.md")).toBe(false);
    expect(unloadedDirectories(tree, rows(tree), opened)).toEqual(["plan/nested"]);

    tree = mergeDirectory(tree, "plan/nested", root([]));
    expect(watchDirectories(rows(tree))).toEqual(watches);
    expect(tree.entries.some((entry) => entry.path === "plan/nested/old.md")).toBe(false);
    expect(unloadedDirectories(tree, rows(tree), opened)).toEqual([]);
  });

  it("does not restore ignored directories that disappeared from the root listing", () => {
    const tree = root([{ path: "plan/old.md", kind: "File", ignored: true }]);
    expect(retainExpandedDirectories(tree, root([])).entries).toEqual([]);
  });

  it("does not retain another workspace's ignored files", () => {
    const previous = root([{ path: "plan/private.md", kind: "File", ignored: true }]);
    const next = {
      ...root([{ path: "plan", kind: "Directory", ignored: true }]),
      workspace_id: "w2",
    };
    expect(retainExpandedDirectories(previous, next)).toBe(next);
  });

  it("leaves closed ignored directories unread until they are expanded", () => {
    const tree = root([{ path: "plan", kind: "Directory", ignored: true }]);
    const opened = new Set(["plan"]);
    const rows = treeRows(tree, new Set(["plan"]), opened);
    expect(unloadedDirectories(tree, rows, opened)).toEqual([]);
  });
});
