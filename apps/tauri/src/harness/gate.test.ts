import { describe, expect, it } from "vitest";
import { displayedFeature, gatedFeatures, type HarnessFeature } from "./types";

function feature(id: number, status: string, revision: number | null = null): HarnessFeature {
  return {
    id,
    slug: `f${id}`,
    title: null,
    spec_raw: null,
    status,
    review_rounds: null,
    gate_attempts: null,
    crates: null,
    acceptance: null,
    source_issue: null,
    created_at: null,
    orchestrator_session_id: null,
    workspace_id: null,
    workspace_path: null,
    revision,
    attempts: null,
    blocked_reason: null,
  };
}

describe("gatedFeatures", () => {
  // Only a spec at the gate is a question for a person; a step that is still
  // running is the machine's business.
  it("takes only the features at the gate, oldest id first", () => {
    const gated = gatedFeatures([
      feature(3, "in_progress"),
      feature(2, "spec_ready"),
      feature(1, "spec_ready"),
      feature(4, "done"),
    ]);
    expect(gated.map((item) => item.id)).toEqual([1, 2]);
  });

  it("is empty when nothing is waiting", () => {
    expect(gatedFeatures([feature(1, "pending")])).toEqual([]);
  });

  it("does not reorder the caller's list", () => {
    const features = [feature(2, "spec_ready"), feature(1, "spec_ready")];
    gatedFeatures(features);
    expect(features.map((item) => item.id)).toEqual([2, 1]);
  });
});

describe("displayedFeature", () => {
  // The list is what an advance updates; a later detail read must not put the
  // gate back on a tab that already moved on.
  it("prefers the list row when its revision is newer than detail", () => {
    const detail = feature(11, "spec_ready", 5);
    const listed = feature(11, "in_progress", 6);
    const shown = displayedFeature(11, detail, [listed]);
    expect(shown?.status).toBe("in_progress");
  });

  it("keeps detail when it is the same write or newer", () => {
    const detail = feature(11, "in_progress", 6);
    expect(displayedFeature(11, detail, [feature(11, "spec_ready", 5)])?.status).toBe(
      "in_progress",
    );
    expect(displayedFeature(11, detail, [feature(11, "in_progress", 6)])).toBe(detail);
  });

  it("falls back to the list when detail is a different feature", () => {
    const listed = feature(11, "in_progress", 2);
    expect(displayedFeature(11, feature(10, "spec_ready", 9), [listed])).toBe(listed);
  });
});
