import { describe, expect, it } from "vitest";
import { describeWorktreeRemovalBlock } from "./worktreeRemoval";

describe("describeWorktreeRemovalBlock", () => {
  it("lists only the prechecks that failed", () => {
    expect(
      describeWorktreeRemovalBlock(
        "cannot remove: running_sessions=false, dirty=true, merge_or_rebase=false",
      ),
    ).toEqual(["It has uncommitted changes."]);
  });

  it("keeps an unfamiliar daemon reason visible", () => {
    expect(describeWorktreeRemovalBlock("removal is temporarily unavailable")).toEqual([
      "removal is temporarily unavailable",
    ]);
  });
});
