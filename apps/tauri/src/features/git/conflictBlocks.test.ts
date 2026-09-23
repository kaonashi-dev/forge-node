import { describe, expect, it } from "vitest";
import { conflictBlocksOf } from "./conflictBlocks";

const HEADER = "diff --git a/a.css b/a.css\n--- a/a.css\n+++ b/a.css\n";

describe("conflictBlocksOf", () => {
  it("reads ours and theirs out of a merge-style conflict", () => {
    const patch =
      HEADER +
      "@@ -1,4 +1,8 @@\n" +
      " .rail {\n" +
      "+<<<<<<< HEAD\n" +
      "   gap: var(--space-1);\n" +
      "+=======\n" +
      "+  gap: 2px;\n" +
      "+>>>>>>> 4a91f0c (Rail)\n" +
      " }\n";
    const [block, ...rest] = conflictBlocksOf(patch);
    expect(rest).toEqual([]);
    expect(block).toEqual({
      line: 2,
      ours: ["  gap: var(--space-1);"],
      base: null,
      theirs: ["  gap: 2px;"],
      complete: true,
    });
  });

  it("keeps the diff3 base when git wrote one", () => {
    const patch =
      HEADER +
      "@@ -1,1 +1,7 @@\n" +
      "+<<<<<<< ours\n" +
      " a\n" +
      "+||||||| base\n" +
      "+b\n" +
      "+=======\n" +
      "+c\n" +
      "+>>>>>>> theirs\n";
    const [block] = conflictBlocksOf(patch);
    expect(block?.base).toEqual(["b"]);
    expect(block?.theirs).toEqual(["c"]);
  });

  it("flags a region the patch context cut through", () => {
    // The second hunk resumes inside the same region: the lines between were
    // never in the patch, so the view must not present the block as whole.
    const patch =
      HEADER +
      "@@ -1,2 +1,3 @@\n" +
      "+<<<<<<< HEAD\n" +
      " one\n" +
      " two\n" +
      "@@ -20,1 +21,3 @@\n" +
      " twenty\n" +
      "+=======\n" +
      "+theirs\n" +
      "+>>>>>>> x\n";
    const [block] = conflictBlocksOf(patch);
    expect(block?.complete).toBe(false);
    expect(block?.ours).toEqual(["one", "two", "twenty"]);
  });

  it("finds nothing once the markers are gone", () => {
    const patch = HEADER + "@@ -1,1 +1,1 @@\n-a\n+b\n";
    expect(conflictBlocksOf(patch)).toEqual([]);
  });

  it("does not take a longer run of equals signs for a separator", () => {
    const patch =
      HEADER +
      "@@ -1,1 +1,6 @@\n" +
      "+<<<<<<< HEAD\n" +
      "+========\n" +
      "+=======\n" +
      "+t\n" +
      "+>>>>>>> x\n";
    const [block] = conflictBlocksOf(patch);
    expect(block?.ours).toEqual(["========"]);
    expect(block?.theirs).toEqual(["t"]);
  });
});
