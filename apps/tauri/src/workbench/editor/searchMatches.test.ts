import { describe, expect, it } from "vitest";
import { matchLabel, tallyMatches } from "./searchMatches";

const cursor = (...ranges: [number, number][]) =>
  ranges.map(([from, to]) => ({ from, to }))[Symbol.iterator]();

describe("tallyMatches", () => {
  it("numbers the match the selection sits on", () => {
    const tally = tallyMatches(cursor([0, 3], [10, 13], [20, 23]), { from: 10, to: 13 });
    expect(tally).toEqual({ total: 3, current: 2, capped: false });
  });

  it("leaves the ordinal unknown for a caret that is not on a match", () => {
    const tally = tallyMatches(cursor([0, 3], [10, 13]), { from: 5, to: 5 });
    expect(tally).toEqual({ total: 2, current: null, capped: false });
  });

  it("stops at the cap and reports the total as a floor", () => {
    const many: [number, number][] = Array.from({ length: 10 }, (_, i) => [i * 2, i * 2 + 1]);
    expect(tallyMatches(cursor(...many), { from: 0, to: 1 }, 4)).toEqual({
      total: 4,
      current: 1,
      capped: true,
    });
  });
});

describe("matchLabel", () => {
  it("says nothing until there is a query", () => {
    expect(matchLabel("empty")).toBe("");
  });

  it("names a broken pattern rather than reporting zero matches", () => {
    expect(matchLabel("invalid")).toBe("Bad pattern");
  });

  it("counts, positions, and marks a capped count", () => {
    expect(matchLabel({ total: 0, current: null, capped: false })).toBe("No results");
    expect(matchLabel({ total: 1, current: null, capped: false })).toBe("1 match");
    expect(matchLabel({ total: 17, current: null, capped: false })).toBe("17 matches");
    expect(matchLabel({ total: 17, current: 3, capped: false })).toBe("3 of 17");
    expect(matchLabel({ total: 5000, current: 2, capped: true })).toBe("2 of 5000+");
  });
});
