import { describe, expect, it } from "vitest";
import { foldsPayload, parseFolds, toggleFold } from "./railFolds";

describe("parseFolds", () => {
  it("reads back what foldsPayload wrote", () => {
    const folds = new Set(["group-1", "project-2", "workspace-3"]);
    expect(parseFolds(foldsPayload(folds))).toEqual(folds);
  });

  it("treats nothing stored as nothing folded", () => {
    expect(parseFolds(undefined)).toEqual(new Set());
    expect(parseFolds("")).toEqual(new Set());
  });

  it("survives a payload an older build could have written", () => {
    // Not JSON, the wrong shape, and the right shape with wrong members: all
    // of them open the rail rather than throwing on the way to first paint.
    expect(parseFolds("{")).toEqual(new Set());
    expect(parseFolds('{"project":true}')).toEqual(new Set());
    expect(parseFolds('["ok", 7, null]')).toEqual(new Set(["ok"]));
  });
});

describe("foldsPayload", () => {
  it("writes the same string whichever order the rows were folded in", () => {
    expect(foldsPayload(new Set(["b", "a"]))).toBe(foldsPayload(new Set(["a", "b"])));
  });
});

describe("toggleFold", () => {
  it("folds an open row and opens a folded one", () => {
    const open = new Set<string>();
    const folded = toggleFold(open, "project-1");
    expect([...folded]).toEqual(["project-1"]);
    expect([...toggleFold(folded, "project-1")]).toEqual([]);
  });

  it("leaves the set it was given alone, so the signal sees a new one", () => {
    const before = new Set(["a"]);
    toggleFold(before, "b");
    expect([...before]).toEqual(["a"]);
  });
});
