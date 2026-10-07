import { describe, expect, it } from "vitest";
import { baseRefOptions, visibleFiles } from "./compareBase";
import type { Branches, BranchRef, DiffFile } from "../../contracts/workbench";

const ref = (name: string, remote: string | null = null): BranchRef => ({
  name,
  scope: remote ? { Remote: { remote } } : "Local",
  upstream: null,
  committed_at: null,
  subject: null,
  checked_out_in: null,
});

const file = (path: string): DiffFile => ({
  path,
  status: "Modified",
  additions: 1,
  deletions: 0,
  patch: "",
  binary: false,
  truncated: false,
});

describe("baseRefOptions", () => {
  const branches: Branches = {
    branches: [ref("feature"), ref("main"), ref("main", "origin"), ref("feature", "origin")],
    remotes: [],
    default_branch: "main",
  };

  it("puts the resolved base first, then remote refs, then local ones", () => {
    expect(baseRefOptions(branches, "feature", "origin/main")).toEqual([
      "origin/main",
      "origin/feature",
      "main",
    ]);
  });

  it("never offers the branch being compared as its own base", () => {
    expect(baseRefOptions(branches, "main", null)).not.toContain("main");
  });

  it("still offers the resolved base when branches have not loaded", () => {
    expect(baseRefOptions(null, "feature", "main")).toEqual(["main"]);
  });
});

describe("visibleFiles", () => {
  const files = [file("a.ts"), file("b.ts")];

  it("shows every file with nothing picked", () => {
    expect(visibleFiles(files, null)).toEqual(files);
  });

  it("shows only the picked file", () => {
    expect(visibleFiles(files, "b.ts").map((f) => f.path)).toEqual(["b.ts"]);
  });

  // A reload can drop the picked path (it was committed away); showing an
  // empty pane would read as "nothing changed".
  it("falls back to every file when the pick is gone", () => {
    expect(visibleFiles(files, "gone.ts")).toEqual(files);
  });
});
