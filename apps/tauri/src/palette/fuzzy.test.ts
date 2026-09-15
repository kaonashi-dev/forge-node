import { describe, expect, it } from "vitest";
import { filter, filterPaths, score } from "./fuzzy";

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

describe("scorePath", () => {
  const rank = (query: string, paths: string[]) =>
    filterPaths(
      paths.map((path) => ({ path })),
      query,
    ).map((entry) => entry.path);

  /*
   * The whole reason this exists. Eight files named `SKILL.md` score the same
   * on their paths and fall back to alphabetical, so the palette reads as a
   * directory dump. A basename hit has to beat any directory hit.
   */
  it("puts a basename hit above a directory hit", () => {
    expect(
      rank("entries", [
        "src/entries-helpers/notes.md",
        "docs/entries/overview.md",
        "src/palette/entries.ts",
      ])[0],
    ).toBe("src/palette/entries.ts");
  });

  it("prefers an exact basename over a longer one that contains it", () => {
    expect(rank("entries.ts", ["src/palette/entries.test.ts", "src/palette/entries.ts"])[0]).toBe(
      "src/palette/entries.ts",
    );
  });

  // JetBrains' camelCase initials: `cp` is how anyone reaches for that file.
  it("matches camelCase initials", () => {
    expect(rank("cp", ["src/store/copyPath.ts", "src/palette/CommandPalette.tsx"])[0]).toBe(
      "src/palette/CommandPalette.tsx",
    );
  });

  /* A separator means the directory *is* the question, so the basename
     shortcut has to step aside and let the whole path be scored. */
  it("treats a query with a separator as being about the path", () => {
    expect(rank("palette/entries", ["docs/entries.md", "src/palette/entries.ts"])[0]).toBe(
      "src/palette/entries.ts",
    );
  });

  it("prefers the shallower file when both matched on their path alone", () => {
    const ranked = rank("srcx", ["src/a/b/c/x.ts", "src/x.ts"]);
    expect(ranked[0]).toBe("src/x.ts");
  });

  it("still requires every character, in order", () => {
    expect(rank("zzz", ["src/palette/entries.ts"])).toEqual([]);
  });

  it("keeps the input order on an empty query", () => {
    const paths = ["b.ts", "a.ts", "c.ts"];
    expect(rank("  ", paths)).toEqual(paths);
  });
});
