import { describe, expect, it } from "vitest";
import { decorationFor, fileDecorations, folderCounts } from "./treeDecorations";
import type { DiffFile } from "./types";

const file = (path: string, status: string): DiffFile => ({
  path,
  status,
  additions: 0,
  deletions: 0,
  patch: "",
  binary: false,
  truncated: false,
});

describe("decorationFor", () => {
  it("gives each git status its own glyph and tone", () => {
    expect(decorationFor("Added")).toEqual({ mark: "A", tone: "added" });
    expect(decorationFor("Untracked")).toEqual({ mark: "?", tone: "untracked" });
  });

  it("shows an unrecognised status as modified rather than dropping it", () => {
    // `DiffStatus` is a bare string on the wire because git grows codes;
    // "something happened here" is true and useful, silence is not.
    expect(decorationFor("TypeChanged")).toEqual({ mark: "M", tone: "modified" });
  });
});

describe("fileDecorations", () => {
  it("keys every changed file by its path", () => {
    const marks = fileDecorations([file("src/a.ts", "Modified"), file("b.ts", "Added")]);
    expect(marks.get("src/a.ts")?.mark).toBe("M");
    expect(marks.get("b.ts")?.mark).toBe("A");
    expect(marks.has("never.ts")).toBe(false);
  });
});

describe("folderCounts", () => {
  it("rolls a change up through every ancestor", () => {
    const counts = folderCounts(["src/ui/a.ts", "src/ui/b.ts", "src/c.ts"]);
    expect(counts.get("src")).toBe(3);
    expect(counts.get("src/ui")).toBe(2);
  });

  it("does not count a file as a directory of itself", () => {
    expect(folderCounts(["a.ts"]).size).toBe(0);
    expect(folderCounts(["src/a.ts"]).get("src/a.ts")).toBeUndefined();
  });
});
