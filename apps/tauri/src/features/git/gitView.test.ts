import { describe, expect, it } from "vitest";
import {
  MAX_SEGMENTS,
  commitLabel,
  commitMessage,
  firstHunk,
  operationName,
  rebaseSteps,
  statusLetter,
  statusWord,
} from "./gitView";

describe("statusLetter", () => {
  it("uses git's porcelain letters, where U is unmerged and ? is untracked", () => {
    expect(statusLetter("Added")).toBe("A");
    expect(statusLetter("Modified")).toBe("M");
    expect(statusLetter("Deleted")).toBe("D");
    expect(statusLetter("Conflicted")).toBe("U");
    expect(statusLetter("Untracked")).toBe("?");
  });

  it("names the state in words for the accessible label", () => {
    expect(statusWord("Conflicted")).toBe("unresolved conflict");
    expect(statusWord("Modified")).toBe("modified");
  });
});

describe("rebaseSteps", () => {
  it("draws one segment per commit and says the position in words", () => {
    const steps = rebaseSteps({ step: 3, total: 7 });
    expect(steps?.label).toBe("step 3 of 7");
    expect(steps?.segments).toEqual([
      "done",
      "done",
      "current",
      "pending",
      "pending",
      "pending",
      "pending",
    ]);
  });

  it("has nothing to draw when git reported no position", () => {
    expect(rebaseSteps({ step: null, total: null })).toBeNull();
    expect(rebaseSteps({ step: 1, total: 0 })).toBeNull();
    expect(rebaseSteps(null)).toBeNull();
  });

  it("scales a long replay but keeps the sentence exact", () => {
    const steps = rebaseSteps({ step: 50, total: 100 });
    expect(steps?.segments).toHaveLength(MAX_SEGMENTS);
    expect(steps?.segments.indexOf("current")).toBe(9);
    expect(steps?.label).toBe("step 50 of 100");
  });

  it("clamps a step past the end", () => {
    const steps = rebaseSteps({ step: 9, total: 4 });
    expect(steps?.label).toBe("step 4 of 4");
    expect(steps?.segments.at(-1)).toBe("current");
  });
});

describe("commit box", () => {
  it("splits the subject from the body on the first blank line", () => {
    expect(commitMessage("  Subject\n\nBody one\n\nBody two ")).toEqual({
      title: "Subject",
      body: "Body one\n\nBody two",
    });
    expect(commitMessage("Only a subject")).toEqual({ title: "Only a subject", body: "" });
  });

  it("counts the files the commit will stage", () => {
    expect(commitLabel(0)).toBe("Nothing to commit");
    expect(commitLabel(1)).toBe("Commit 1 file");
    expect(commitLabel(3)).toBe("Commit 3 files");
  });
});

describe("firstHunk", () => {
  const patch =
    "diff --git a/x b/x\n--- a/x\n+++ b/x\n" +
    "@@ -31,2 +31,3 @@ function x() {\n-a\n+b\n+c\n d\n" +
    "@@ -60,1 +61,1 @@\n-e\n+f\n";

  it("returns the first hunk's rows and counts the hunks after it", () => {
    const hunk = firstHunk(patch);
    expect(hunk?.header).toBe("@@ -31,2 +31,3 @@");
    expect(hunk?.rows.map((row) => row.kind)).toEqual(["removed", "added", "added", "context"]);
    expect(hunk?.more).toBe(1);
    expect(hunk?.hiddenRows).toBe(0);
  });

  it("cuts a long hunk and says how much was left out", () => {
    const hunk = firstHunk(patch, 2);
    expect(hunk?.rows).toHaveLength(2);
    expect(hunk?.hiddenRows).toBe(2);
  });

  it("has no preview for a patch without hunks", () => {
    expect(firstHunk("Binary files differ\n")).toBeNull();
  });
});

describe("operationName", () => {
  it("spells the sequencer operation the way a sentence needs it", () => {
    expect(operationName("CherryPick")).toBe("Cherry-pick");
    expect(operationName("Merge")).toBe("Merge");
    expect(operationName(null)).toBe("Rebase");
    expect(operationName("Bisect")).toBe("Bisect");
  });
});
