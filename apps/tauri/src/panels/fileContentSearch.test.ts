import { describe, expect, it } from "vitest";
import { groupContentHits, shouldSearchContent, type ContentHitGroup } from "./fileContentSearch";
import type { SearchMatch } from "../workbench/types";

function hit(path: string, line: number, text = "x"): SearchMatch {
  return { path, line, column: 1, text };
}

describe("shouldSearchContent", () => {
  it("refuses empty and one-character queries", () => {
    expect(shouldSearchContent("")).toBe(false);
    expect(shouldSearchContent(" ")).toBe(false);
    expect(shouldSearchContent("a")).toBe(false);
  });

  it("accepts two characters after trim", () => {
    expect(shouldSearchContent("ab")).toBe(true);
    expect(shouldSearchContent("  ab  ")).toBe(true);
  });
});

describe("groupContentHits", () => {
  it("keeps daemon order and groups by path", () => {
    const groups: ContentHitGroup[] = groupContentHits([
      hit("a.rs", 1),
      hit("b.rs", 2),
      hit("a.rs", 3),
    ]);
    expect(groups.map((group) => group.path)).toEqual(["a.rs", "b.rs"]);
    expect(groups[0].matches.map((match) => match.line)).toEqual([1, 3]);
    expect(groups[1].matches.map((match) => match.line)).toEqual([2]);
  });
});
