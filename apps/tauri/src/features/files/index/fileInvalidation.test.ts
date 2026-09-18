import { describe, expect, it } from "vitest";
import { fileAffected, sameListing } from "./fileInvalidation";
import type { FileTree } from "../../../contracts/workbench";

describe("file invalidation", () => {
  it("reloads on an exact path, a removed parent, or a lost-event resync", () => {
    expect(fileAffected("src/main.rs", "src/main.rs")).toBe(true);
    expect(fileAffected("src/main.rs", "src")).toBe(true);
    expect(fileAffected("src/main.rs", "")).toBe(true);
  });
  it("does not reload for a neighboring file or a shared name prefix", () => {
    expect(fileAffected("src/main.rs", "src/lib.rs")).toBe(false);
    expect(fileAffected("src/main.rs", "sr")).toBe(false);
    expect(fileAffected("src/main.rs", "src/main")).toBe(false);
  });
});

const listing = (paths: string[], truncated = false): FileTree => ({
  workspace_id: "w1",
  entries: paths.map((path) => ({ path, kind: "File", ignored: false })),
  truncated,
});

describe("listing equality", () => {
  it("holds when a save changed contents and no name", () => {
    expect(sameListing(listing(["a.rs", "b.rs"]), listing(["a.rs", "b.rs"]))).toBe(true);
  });
  it("breaks on a created, removed, renamed or reordered path", () => {
    expect(sameListing(listing(["a.rs"]), listing(["a.rs", "b.rs"]))).toBe(false);
    expect(sameListing(listing(["a.rs", "b.rs"]), listing(["a.rs"]))).toBe(false);
    expect(sameListing(listing(["a.rs"]), listing(["c.rs"]))).toBe(false);
    expect(sameListing(listing(["a.rs", "b.rs"]), listing(["b.rs", "a.rs"]))).toBe(false);
  });
  it("breaks on a changed budget, a changed kind, a new ignore rule, or no listing yet", () => {
    expect(sameListing(listing(["a.rs"]), listing(["a.rs"], true))).toBe(false);
    const directory = listing(["a.rs"]);
    directory.entries[0].kind = "Directory";
    expect(sameListing(listing(["a.rs"]), directory)).toBe(false);
    const ignored = listing(["a.rs"]);
    ignored.entries[0].ignored = true;
    expect(sameListing(listing(["a.rs"]), ignored)).toBe(false);
    expect(sameListing(null, listing(["a.rs"]))).toBe(false);
  });
});

it("detects changed symlink target status", () => {
  const before = listing(["link"]);
  const after = listing(["link"]);
  before.entries[0].symlink = "External";
  after.entries[0].symlink = "Broken";
  expect(sameListing(before, after)).toBe(false);
});
