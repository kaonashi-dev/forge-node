import { describe, expect, it } from "vitest";
import {
  collapseTarget,
  directoryPaths,
  expandTarget,
  filterTree,
  foldUnseen,
  treeRows,
} from "./filetree";
import type { FileTree } from "./types";

const tree: FileTree = {
  workspace_id: "w1",
  truncated: false,
  entries: [
    { path: "Cargo.toml", kind: "File", ignored: false },
    { path: "crates/client/src/app_shell.rs", kind: "File", ignored: false },
    { path: "crates/client/src/lib.rs", kind: "File", ignored: false },
    { path: "crates/domain/lib.rs", kind: "File", ignored: false },
    { path: "README.md", kind: "File", ignored: false },
  ],
};

const paths = (collapsed: string[]) => treeRows(tree, new Set(collapsed)).map((row) => row.path);

describe("ignored rows", () => {
  const mixed: FileTree = {
    workspace_id: "w1",
    truncated: false,
    entries: [
      { path: "src/main.rs", kind: "File", ignored: false },
      { path: "src/main.rs.orig", kind: "File", ignored: true },
      { path: "target/debug/app", kind: "File", ignored: true },
    ],
  };

  it("marks a directory only when everything under it is ignored", () => {
    const marked = new Map(treeRows(mixed, new Set()).map((row) => [row.path, row.ignored]));
    expect(marked.get("target/debug")).toBe(true);
    // One build artifact does not make the source directory holding it ignored.
    expect(marked.get("src")).toBe(false);
    expect(marked.get("src/main.rs")).toBe(false);
    expect(marked.get("src/main.rs.orig")).toBe(true);
  });
});

describe("treeRows", () => {
  it("synthesizes directories and collapses only-child chains", () => {
    const rows = treeRows(tree, new Set());
    expect(rows.map((row) => row.label)).toEqual([
      "crates",
      "client/src",
      "app_shell.rs",
      "lib.rs",
      "domain",
      "lib.rs",
      "Cargo.toml",
      "README.md",
    ]);
    expect(rows.every((row) => !row.ignored)).toBe(true);
    expect(rows.find((row) => row.path === "client/src")).toBeUndefined();
    expect(rows.find((row) => row.path === "crates/client/src")?.label).toBe("client/src");
  });

  it("renders nested files from a files-only listing", () => {
    expect(paths([])).toEqual([
      "crates",
      "crates/client/src",
      "crates/client/src/app_shell.rs",
      "crates/client/src/lib.rs",
      "crates/domain",
      "crates/domain/lib.rs",
      "Cargo.toml",
      "README.md",
    ]);
  });

  it("hides the children of a collapsed directory", () => {
    const rows = treeRows(tree, new Set(["crates"]));
    expect(rows).toHaveLength(3);
    expect(rows.find((row) => row.path === "crates")?.folded).toBe(true);
    expect(rows.some((row) => row.path.startsWith("crates/") && row.isFile)).toBe(false);
  });

  it("sorts directories before files case-insensitively", () => {
    const rows = treeRows(
      {
        ...tree,
        entries: [
          { path: "z-file", kind: "File", ignored: false },
          { path: "Alpha/file", kind: "File", ignored: false },
          { path: "beta/file", kind: "File", ignored: false },
          { path: "a-file", kind: "File", ignored: false },
        ],
      },
      new Set(),
    );
    expect(rows.map((row) => row.path)).toEqual([
      "Alpha",
      "Alpha/file",
      "beta",
      "beta/file",
      "a-file",
      "z-file",
    ]);
  });

  it("tolerates a directory entry from the daemon", () => {
    const rows = treeRows(
      {
        ...tree,
        entries: [
          { path: "src", kind: "Directory", ignored: false },
          { path: "src/main.rs", kind: "File", ignored: false },
        ],
      },
      new Set(),
    );
    expect(rows.map((row) => row.path)).toEqual(["src", "src/main.rs"]);
  });

  it("never folds files", () => {
    const rows = treeRows(tree, new Set());
    expect(rows.filter((row) => row.isFile).every((row) => !row.folded)).toBe(true);
  });

  it("has nothing to show without a tree", () => {
    expect(treeRows(null, new Set())).toEqual([]);
  });
});

describe("collapseTarget", () => {
  const rows = (collapsed: string[]) => treeRows(tree, new Set(collapsed));

  it("folds an expanded directory", () => {
    const list = rows([]);
    const row = list.find((item) => item.path === "crates");
    expect(collapseTarget(row, new Set())).toEqual({ fold: "crates" });
  });

  it("jumps to the parent of a file", () => {
    const list = rows([]);
    const row = list.find((item) => item.path === "crates/client/src/lib.rs");
    expect(collapseTarget(row, new Set())).toEqual({ select: "crates/client/src" });
  });

  it("does nothing at the root", () => {
    const row = rows([]).find((item) => item.path === "Cargo.toml");
    expect(collapseTarget(row, new Set())).toBeNull();
    expect(collapseTarget(undefined, new Set())).toBeNull();
  });
});

describe("expandTarget", () => {
  it("unfolds a folded directory", () => {
    const list = treeRows(tree, new Set(["crates"]));
    const index = list.findIndex((row) => row.path === "crates");
    expect(expandTarget(list[index], new Set(["crates"]), list, index)).toEqual({
      unfold: "crates",
    });
  });

  it("steps into the first child of an expanded one", () => {
    const collapsed = new Set<string>();
    const list = treeRows(tree, collapsed);
    const index = list.findIndex((row) => row.path === "crates");
    expect(expandTarget(list[index], collapsed, list, index)).toEqual({
      select: "crates/client/src",
    });
  });

  it("leaves a file where it is", () => {
    const list = treeRows(tree, new Set());
    const index = list.findIndex((row) => row.path === "README.md");
    expect(expandTarget(list[index], new Set(), list, index)).toBeNull();
  });

  it("stops at an expanded but empty directory", () => {
    const list = treeRows(
      {
        ...tree,
        entries: [{ path: "empty", kind: "Directory", ignored: false }],
      },
      new Set(),
    );
    const index = list.findIndex((row) => row.path === "empty");
    expect(expandTarget(list[index], new Set(), list, index)).toBeNull();
  });
});

describe("directoryPaths", () => {
  it("names every directory, chain ends and intermediates alike", () => {
    const dirs = directoryPaths({
      workspace_id: "w1",
      truncated: false,
      entries: [
        { path: ".claude/skills/feature/SKILL.md", kind: "File", ignored: false },
        { path: "src/main.ts", kind: "File", ignored: false },
        { path: "docs", kind: "Directory", ignored: false },
      ],
    });
    expect(dirs.sort()).toEqual([
      ".claude",
      ".claude/skills",
      ".claude/skills/feature",
      "docs",
      "src",
    ]);
  });

  it("is empty without a tree", () => {
    expect(directoryPaths(null)).toEqual([]);
  });
});

describe("foldUnseen", () => {
  it("folds a directory the panel has not decided about", () => {
    const result = foldUnseen(new Set(), new Set(), ["src", "src/ui"]);
    expect([...result.collapsed].sort()).toEqual(["src", "src/ui"]);
    expect([...result.seen].sort()).toEqual(["src", "src/ui"]);
  });

  it("leaves a folder the person opened open on the next read", () => {
    const seen = new Set(["src", "src/ui"]);
    const collapsed = new Set(["src/ui"]); // `src` was opened by hand.
    const result = foldUnseen(collapsed, seen, ["src", "src/ui"]);
    expect([...result.collapsed]).toEqual(["src/ui"]);
  });

  it("folds a directory that appears later", () => {
    const result = foldUnseen(new Set(), new Set(["src"]), ["src", "src/new"]);
    expect([...result.collapsed]).toEqual(["src/new"]);
  });
});

describe("foldUnseen identity", () => {
  // The effect that calls this compares by identity to decide whether to
  // write. New sets with equal contents made it wake itself until the stack
  // ran out, which is a blank panel, not a slow one.
  it("returns the very same sets when nothing is new", () => {
    const collapsed = new Set(["src"]);
    const seen = new Set(["src"]);
    const result = foldUnseen(collapsed, seen, ["src"]);
    expect(result.collapsed).toBe(collapsed);
    expect(result.seen).toBe(seen);
  });

  it("returns new sets only when it actually folded something", () => {
    const seen = new Set(["src"]);
    const result = foldUnseen(new Set(), seen, ["src", "docs"]);
    expect(result.seen).not.toBe(seen);
    expect([...result.collapsed]).toEqual(["docs"]);
  });
});

describe("filterTree", () => {
  const tree = {
    workspace_id: "w",
    truncated: true,
    entries: [
      { path: "src/store/a.ts", kind: "File", ignored: false },
      { path: "src/ui/b.ts", kind: "File", ignored: false },
      { path: "README.md", kind: "File", ignored: false },
    ],
  };

  it("keeps the paths that contain the query, case-insensitively", () => {
    expect(filterTree(tree, "STORE")?.entries.map((entry) => entry.path)).toEqual([
      "src/store/a.ts",
    ]);
  });

  it("returns the tree untouched for an empty or blank query", () => {
    expect(filterTree(tree, "")).toBe(tree);
    expect(filterTree(tree, "   ")).toBe(tree);
  });

  it("keeps the truncation flag: filtering a partial listing is still partial", () => {
    expect(filterTree(tree, "src")?.truncated).toBe(true);
  });

  it("has nothing to filter when there is no tree", () => {
    expect(filterTree(null, "x")).toBeNull();
  });
});
