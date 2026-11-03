import { describe, expect, it } from "vitest";
import type { ReviewRun } from "../../contracts/workbench";
import { supersedes } from "./runOrder";

function run(id: string, patch: Partial<ReviewRun> = {}): ReviewRun {
  return {
    id,
    workspace_id: "w",
    target: { kind: "workspace", workspace_id: "w", base: null },
    head_oid: "h",
    base_oid: null,
    tier: "base",
    preset: "standard",
    provider: "claude",
    status: "Running",
    stage: "Model",
    summary: "",
    score: null,
    verdict: null,
    error: null,
    session_id: null,
    coverage: { included: [], excluded: [], truncated: [], redactions: 0, notes: [], rules: [] },
    created_at: "2026-11-03T00:00:00Z",
    finished_at: null,
    ...patch,
  };
}

describe("supersedes", () => {
  it("takes a newer run and refuses an older one", () => {
    expect(supersedes(undefined, run("a"))).toBe(true);
    expect(supersedes(run("a"), run("b"))).toBe(true);
    expect(supersedes(run("b"), run("a"))).toBe(false);
  });

  it("never lets an earlier read of the same run undo a finished one", () => {
    const done = run("a", {
      status: "Completed",
      stage: "Validate",
      finished_at: "2026-11-03T00:01:00Z",
    });
    expect(supersedes(done, run("a"))).toBe(false);
    expect(supersedes(run("a", { stage: "Validate" }), run("a", { stage: "Pack" }))).toBe(false);
    expect(supersedes(run("a"), done)).toBe(true);
    expect(supersedes(done, done)).toBe(true);
  });
});
