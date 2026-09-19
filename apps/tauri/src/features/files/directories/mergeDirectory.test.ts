import { describe, expect, it } from "vitest";
import { mergeDirectory, retainExpandedDirectories } from "./mergeDirectory";
import type { FileEntry, FileTree } from "../../../contracts/workbench";
const file = (path: string): FileEntry => ({ path, kind: "File", ignored: false });
const folder = (path: string): FileEntry => ({ path, kind: "Directory", ignored: false });
const tree = (entries: FileEntry[], extra: Partial<FileTree> = {}): FileTree => ({
  workspace_id: "w",
  entries,
  truncated: false,
  ...extra,
});

describe("one-level directory merge", () => {
  it("preserves a real empty parent and its metadata", () => {
    const parent = { ...folder("empty"), ignored: false, symlink: "Directory" as const };
    const next = mergeDirectory(tree([parent, file("other")]), "empty", tree([]));
    expect(next.entries).toEqual([parent, file("other")]);
    expect(next.loadedDirectories).toEqual(["empty"]);
  });
  it("refreshes root without dropping the children of surviving directories", () => {
    const before = tree(
      [folder("src"), file("src/a"), folder("gone"), file("gone/a"), file("old")],
      { loadedDirectories: ["", "src", "gone"] },
    );
    const next = mergeDirectory(before, "", tree([folder("src"), file("new")]));
    expect(next.entries).toEqual([file("new"), folder("src"), file("src/a")]);
    expect(next.loadedDirectories).toEqual(["", "src"]);
  });
  it("only replaces direct children and retains a loaded surviving subtree", () => {
    const before = tree([
      folder("src"),
      folder("src/nested"),
      file("src/nested/a"),
      file("src/old"),
      file("neighbor"),
    ]);
    const next = mergeDirectory(before, "src", tree([folder("src/nested"), file("src/new")]));
    expect(next.entries).toEqual([
      file("neighbor"),
      folder("src"),
      folder("src/nested"),
      file("src/nested/a"),
      file("src/new"),
    ]);
  });
  it("does not infer deletion from partial listings", () => {
    const next = mergeDirectory(
      tree([file("confirmed"), folder("dir"), file("dir/a")]),
      "",
      tree([file("other")], { truncated: true }),
    );
    expect(next.entries).toEqual([file("confirmed"), folder("dir"), file("dir/a"), file("other")]);
    expect(next.truncated).toBe(true);
  });
  it("drops descendants when a directory becomes a file", () => {
    expect(
      mergeDirectory(tree([folder("src"), file("src/a")]), "", tree([file("src")])).entries,
    ).toEqual([file("src")]);
  });
  it("refuses an answer from another workspace", () => {
    const before = tree([file("private")]);
    expect(mergeDirectory(before, "", tree([], { workspace_id: "other" }))).toBe(before);
  });
  it("retains expanded children only under directories still present", () => {
    const before = tree([folder("src"), file("src/a"), folder("gone"), file("gone/a")]);
    expect(retainExpandedDirectories(before, tree([folder("src")])).entries).toEqual([
      folder("src"),
      file("src/a"),
    ]);
  });
});
