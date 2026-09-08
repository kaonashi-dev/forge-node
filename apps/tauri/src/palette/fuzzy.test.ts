import { describe, expect, it } from "vitest";
import { filter, score } from "./fuzzy";

describe("fuzzy score", () => {
  it("requires every query character, in order", () => {
    expect(score("nt", "New Terminal")).not.toBeNull();
    // `lt` has both letters but in the wrong order: the `l` is the last
    // character, so no `t` follows it.
    expect(score("lt", "New Terminal")).toBeNull();
    expect(score("xyz", "New Terminal")).toBeNull();
  });

  it("matches everything on an empty query", () => {
    expect(score("", "anything")).toBe(0);
  });

  // The whole reason the word-boundary bonus exists.
  it("finds New Terminal ahead of Agent Settings for `nt`", () => {
    expect(score("nt", "New Terminal")!).toBeGreaterThan(score("nt", "Agent Settings")!);
  });

  // Compared against a candidate with no word boundaries either, so the run
  // bonus is the only difference between the two.
  it("rewards consecutive runs", () => {
    expect(score("abcd", "abcd")!).toBeGreaterThan(score("abcd", "axbxcxd")!);
  });

  it("prefers a short label that used most of its characters", () => {
    expect(score("git", "Git")!).toBeGreaterThan(score("git", "Git Panel Configuration Screen")!);
  });

  it("ignores spaces in the query and case on both sides", () => {
    expect(score("n t", "New Terminal")).toBe(score("NT", "New Terminal"));
  });

  it("treats a path separator as a word start", () => {
    expect(score("sf", "src/forge.ts")!).toBeGreaterThan(score("sf", "srcforge.ts")!);
  });
});

describe("filter", () => {
  const entries = [
    { label: "New Terminal" },
    { label: "New Agent" },
    { label: "Toggle Sidebar" },
    { label: "Never Again" },
  ];

  // The palette opens on a menu, not on a ranking.
  it("keeps presentation order for an empty query", () => {
    expect(filter(entries, "  ").map((entry) => entry.label)).toEqual(
      entries.map((entry) => entry.label),
    );
  });

  it("drops what does not match", () => {
    expect(filter(entries, "sidebar")).toHaveLength(1);
  });

  // A query that matches a whole group must not shuffle it on every keystroke.
  it("keeps the original order on ties", () => {
    const tied = [{ label: "aa x" }, { label: "aa y" }, { label: "aa z" }];
    expect(filter(tied, "aa").map((entry) => entry.label)).toEqual(["aa x", "aa y", "aa z"]);
  });
});
