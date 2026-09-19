import { describe, expect, it } from "vitest";
import { RECENT_MEMORY, initialFiles, parseRecent, withOpened } from "./recentFiles";

describe("parseRecent", () => {
  it("reads the lists it wrote", () => {
    expect(parseRecent(JSON.stringify({ w1: ["a.ts", "b.ts"] }))).toEqual({ w1: ["a.ts", "b.ts"] });
  });

  /* Written by another build, or half-written. It has to come back as "nothing
     remembered" rather than throw inside the memo that renders the palette. */
  it("treats anything unexpected as nothing remembered", () => {
    expect(parseRecent(undefined)).toEqual({});
    expect(parseRecent("")).toEqual({});
    expect(parseRecent("{ not json")).toEqual({});
    expect(parseRecent('"a string"')).toEqual({});
    expect(parseRecent("[1,2,3]")).toEqual({});
    expect(parseRecent(JSON.stringify({ w1: "not-a-list" }))).toEqual({});
    expect(parseRecent(JSON.stringify({ w1: [1, null, "a.ts"] }))).toEqual({ w1: ["a.ts"] });
  });

  it("caps a list that was written longer than the memory", () => {
    const long = Array.from({ length: RECENT_MEMORY + 20 }, (_, i) => `f${i}.ts`);
    expect(parseRecent(JSON.stringify({ w1: long })).w1).toHaveLength(RECENT_MEMORY);
  });
});

describe("withOpened", () => {
  it("puts the newest first", () => {
    expect(withOpened({}, "w1", "a.ts").w1).toEqual(["a.ts"]);
    expect(withOpened({ w1: ["a.ts"] }, "w1", "b.ts").w1).toEqual(["b.ts", "a.ts"]);
  });

  /* Moved, not appended: opening a file again is what makes it recent, and a
     list that kept the first visit would go stale while being written to. */
  it("moves a path that was already there instead of duplicating it", () => {
    expect(withOpened({ w1: ["a.ts", "b.ts", "c.ts"] }, "w1", "c.ts").w1).toEqual([
      "c.ts",
      "a.ts",
      "b.ts",
    ]);
  });

  it("keeps each checkout's list to itself", () => {
    const next = withOpened({ w1: ["a.ts"] }, "w2", "b.ts");
    expect(next).toEqual({ w1: ["a.ts"], w2: ["b.ts"] });
  });

  it("forgets the oldest past the memory", () => {
    const full = Array.from({ length: RECENT_MEMORY }, (_, i) => `f${i}.ts`);
    const next = withOpened({ w1: full }, "w1", "new.ts").w1!;
    expect(next).toHaveLength(RECENT_MEMORY);
    expect(next[0]).toBe("new.ts");
    expect(next).not.toContain(`f${RECENT_MEMORY - 1}.ts`);
  });
});

describe("initialFiles", () => {
  const known = new Set(["a.ts", "b.ts", "c.ts", "d.ts"]);

  it("lists what was opened, then what has uncommitted changes", () => {
    expect(initialFiles(["a.ts"], ["c.ts"], known)).toEqual(["a.ts", "c.ts"]);
  });

  it("names a file once even when it is both", () => {
    expect(initialFiles(["a.ts", "b.ts"], ["b.ts", "c.ts"], known)).toEqual([
      "a.ts",
      "b.ts",
      "c.ts",
    ]);
  });

  /* A remembered path can have been deleted, renamed, or belong to a branch
     that has since been checked out over. Offering it opens nothing. */
  it("drops a remembered path the tree no longer has", () => {
    expect(initialFiles(["gone.ts", "a.ts"], [], known)).toEqual(["a.ts"]);
  });

  it("stops at the limit, keeping the most recent", () => {
    expect(initialFiles(["a.ts", "b.ts", "c.ts"], ["d.ts"], known, 2)).toEqual(["a.ts", "b.ts"]);
  });

  it("is empty when nothing has been opened and nothing has changed", () => {
    expect(initialFiles([], [], known)).toEqual([]);
  });
});
