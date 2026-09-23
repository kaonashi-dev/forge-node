import { describe, expect, it } from "vitest";
import { highlight } from "./highlight";

const lit = (label: string, query: string) =>
  highlight(label, query)
    .filter((segment) => segment.hit)
    .map((segment) => segment.text);

describe("palette match highlight", () => {
  it("lights a contiguous hit as one word", () => {
    expect(lit("New worktree from branch…", "worktree")).toEqual(["worktree"]);
  });

  it("falls back to the subsequence the ranking matched", () => {
    expect(lit("New Terminal", "nt")).toEqual(["N", "T"]);
  });

  it("lights nothing when the label alone does not hold the query", () => {
    expect(lit("entries.ts", "srcpal")).toEqual([]);
  });

  it("keeps the label whole", () => {
    const segments = highlight("Open File…", "fi");
    expect(segments.map((segment) => segment.text).join("")).toBe("Open File…");
  });
});
