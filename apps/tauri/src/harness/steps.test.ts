import { describe, expect, it } from "vitest";
import { jobLabel, jobStateTone, jobStepLabel, recordedSteps } from "./steps";
import type { Job } from "../runtime/types";
import type { HarnessEvent } from "./types";

function event(kind: string, job: string | null): HarnessEvent {
  return { ts: "2026-08-30T10:00:00Z", kind, detail: "", job };
}

function job(id: string, role: string): Job {
  return {
    id,
    provider_id: "claude",
    workspace_id: "w",
    role,
    feature_id: 4,
    parent_session_id: null,
    state: "Running",
    summary: "",
    prompt: "",
    provider_session_id: null,
    exit_code: null,
    last_line: null,
    started_at: "2026-08-30T10:00:00Z",
    finished_at: null,
    log_path: "/tmp/j.log",
  };
}

describe("harness steps", () => {
  it("names a step by the role that ran it", () => {
    expect(jobStepLabel("Orchestrator")).toBe("spec");
    expect(jobStepLabel("Executor")).toBe("implement");
    expect(jobStepLabel("Reviewer")).toBe("review");
    expect(jobStepLabel("Something")).toBe("step");
    expect(jobLabel(job("j1", "Reviewer"))).toBe("review · claude");
  });

  // Restarting the daemon empties the job list; the log is what still points
  // at the transcripts on disk.
  it("recovers the steps the daemon has forgotten", () => {
    const timeline = [
      event("feature_registered", null),
      event("spec_started", "j1"),
      event("impl_started", "j2"),
      event("review_started", "j3"),
    ];
    expect(recordedSteps(timeline, [job("j2", "Executor")])).toEqual([
      { label: "spec", job: "j1" },
      { label: "review", job: "j3" },
    ]);
  });

  // A step re-read from two events would otherwise list twice.
  it("lists one row per job, and only for events that started a step", () => {
    const timeline = [
      event("spec_started", "j1"),
      event("spec_started", "j1"),
      event("note", "j9"),
    ];
    expect(recordedSteps(timeline, [])).toEqual([{ label: "spec", job: "j1" }]);
  });

  it("maps every job state to one badge tone", () => {
    expect(jobStateTone("Running")).toBe("warn");
    expect(jobStateTone("Succeeded")).toBe("good");
    expect(jobStateTone("Failed")).toBe("bad");
    expect(jobStateTone("Queued")).toBe("neutral");
    expect(jobStateTone("Cancelled")).toBe("neutral");
  });
});
