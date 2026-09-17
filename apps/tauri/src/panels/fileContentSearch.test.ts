import { describe, expect, it } from "vitest";
import {
  buildExcerpts,
  groupContentHits,
  hitSegments,
  rowHitSegments,
  shouldSearchContent,
  type ContentHitGroup,
} from "./fileContentSearch";
import type { SearchMatch } from "../workbench/types";

function hit(
  path: string,
  line: number,
  text = "x",
  before: string[] = [],
  after: string[] = [],
): SearchMatch {
  return { path, line, column: 1, text, before, after };
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

describe("hitSegments", () => {
  it("marks every occurrence, case-sensitively, without overlap", () => {
    expect(hitSegments("aaa Wompi wompi", "aa")).toEqual([
      { text: "aa", hit: true },
      { text: "a Wompi wompi", hit: false },
    ]);
    expect(hitSegments("x wompi.wompi", "wompi")).toEqual([
      { text: "x ", hit: false },
      { text: "wompi", hit: true },
      { text: ".", hit: false },
      { text: "wompi", hit: true },
    ]);
  });

  it("leaves a line with no hit whole", () => {
    expect(hitSegments("nothing", "wompi")).toEqual([{ text: "nothing", hit: false }]);
    expect(hitSegments("", "wompi")).toEqual([]);
  });
});

describe("rowHitSegments", () => {
  it("drops indentation and keeps a near hit where it is", () => {
    expect(rowHitSegments("    return wompi;", "wompi", 12)[0]).toEqual({
      text: "return ",
      hit: false,
    });
  });

  // The sidebar ellipsis would otherwise hide the very thing the row is for.
  it("cuts the start of the line so a far hit is visible", () => {
    const segments = rowHitSegments(`import { A, B, C, D } from './wompi.types';`, "wompi", 4);
    expect(segments[0]).toEqual({ text: "…", hit: false });
    expect(segments.map((segment) => segment.text).join("")).toBe("… './wompi.types';");
    expect(segments.find((segment) => segment.hit)?.text).toBe("wompi");
  });
});

describe("buildExcerpts", () => {
  it("merges touching context into one run and keeps gaps apart", () => {
    const [file] = buildExcerpts([
      hit("a.ts", 3, "hit 3", ["one", "two"], ["four", "hit 5"]),
      hit("a.ts", 5, "hit 5", ["hit 3", "four"], ["six", "seven"]),
      hit("a.ts", 20, "hit 20", ["nineteen"], []),
    ]);
    expect(file.hits).toBe(3);
    expect(file.excerpts.map((run) => run.map((line) => line.line))).toEqual([
      [1, 2, 3, 4, 5, 6, 7],
      [19, 20],
    ]);
    expect(file.excerpts[0].filter((line) => line.hit).map((line) => line.line)).toEqual([3, 5]);
  });

  it("never numbers context above line one", () => {
    const [file] = buildExcerpts([hit("a.ts", 1, "hit", ["ghost"], ["two"])]);
    expect(file.excerpts[0].map((line) => line.line)).toEqual([1, 2]);
  });
});
