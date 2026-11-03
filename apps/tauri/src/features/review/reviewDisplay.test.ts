import { describe, expect, it } from "vitest";
import type { Finding, ReviewRun } from "../../contracts/workbench";
import { response_review } from "../../contracts/generated/fixtures";
import {
  duration,
  groupFindings,
  isStale,
  scoreLabel,
  stageLabel,
  stageProgress,
  verdictLabel,
  verdictTone,
} from "./reviewDisplay";

const fixture = response_review.Review.run;
// Assigned field by field so a wire shape the hand-written type drifted from fails to compile.
const run: ReviewRun = {
  ...fixture,
  coverage: {
    ...fixture.coverage,
    included: [...fixture.coverage.included],
    excluded: [...fixture.coverage.excluded],
    truncated: [...fixture.coverage.truncated],
    notes: [...fixture.coverage.notes],
    rules: [...fixture.coverage.rules],
  },
};

function finding(id: number, path: string, line: number | null, severity: Finding["severity"]) {
  const base: Finding = response_review.Review.findings[0]!;
  return { ...base, id, path, line, severity };
}

describe("review view", () => {
  it("decodes the daemon's own encoding of a report", () => {
    expect(run.status).toBe("Completed");
    expect(run.tier).toBe("base");
    expect(run.target.kind).toBe("workspace");
    expect(response_review.Review.findings[0]?.severity).toBe("P1");
  });

  const findings = [
    finding(1, "b.rs", 9, "P2"),
    finding(2, "a.rs", 4, "P1"),
    finding(3, "b.rs", 2, "P0"),
    finding(4, "a.rs", null, "P2"),
  ];

  it("groups by severity, most severe first, and leaves empty groups out", () => {
    const groups = groupFindings(findings, "severity");
    expect(groups.map((group) => group.key)).toEqual(["P0", "P1", "P2"]);
    expect(groups[2]?.findings.map((f) => f.id)).toEqual([4, 1]);
    expect(groupFindings([findings[0]!], "severity").map((group) => group.key)).toEqual(["P2"]);
  });

  it("groups by file, general findings first, then by line", () => {
    const groups = groupFindings(findings, "file");
    expect(groups.map((group) => group.key)).toEqual(["a.rs", "b.rs"]);
    expect(groups[0]?.findings.map((f) => f.id)).toEqual([4, 2]);
    expect(groups[1]?.findings.map((f) => f.id)).toEqual([3, 1]);
  });

  it("labels and tones each verdict", () => {
    expect(verdictLabel("Block")).toBe("Bloquea");
    expect(verdictLabel("Review")).toBe("Revisar");
    expect(verdictLabel("Ok")).toBe("OK");
    expect(verdictTone("Block")).toBe("bad");
    expect(verdictTone("Review")).toBe("warn");
    expect(verdictTone("Ok")).toBe("good");
  });

  it("is stale only when the checkout's head is known and moved", () => {
    expect(isStale(run, run.head_oid)).toBe(false);
    expect(isStale(run, "f00d")).toBe(true);
    expect(isStale(run, null)).toBe(false);
  });

  it("names each stage and advances through them", () => {
    expect(stageLabel(null)).toBe("Queued");
    expect(stageLabel("Model")).toBe("Waiting for the reviewer");
    const steps = [null, "Diff", "Pack", "Model", "Validate"] as const;
    const values = steps.map(stageProgress);
    expect(values[0]).toBe(0);
    expect(values).toEqual([...values].sort((a, b) => a - b));
    expect(values[4]).toBeLessThan(100);
  });

  it("formats the duration from start to finish, or to now while running", () => {
    const start = Date.parse("2026-11-02T10:00:00Z");
    const timed = { created_at: "2026-11-02T10:00:00Z", finished_at: "2026-11-02T10:03:05Z" };
    expect(duration(timed, start)).toBe("3m 05s");
    expect(duration({ ...timed, finished_at: null }, start + 42_000)).toBe("42s");
    expect(duration({ ...timed, finished_at: "2026-11-02T11:02:00Z" }, start)).toBe("1h 02m");
  });

  it("shows a score out of five", () => {
    expect(scoreLabel(3.5)).toBe("3.5 / 5");
    expect(scoreLabel(5)).toBe("5.0 / 5");
    expect(scoreLabel(null)).toBe("—");
  });
});
