import { expect, it } from "vitest";
import { agentRuns, eventSummary, featureProgress, runDuration } from "./progress";
import type { HarnessAttempt, HarnessFeature } from "./types";
import type { Job } from "../runtime/types";

const attempt: HarnessAttempt = {
  step: "Implement",
  n: 2,
  job: "j1",
  provider: "codex",
  transport: "cli",
  started_at: "2026-09-01T10:00:00Z",
  settled_at: "2026-09-01T10:02:15Z",
  outcome: "failed",
  detail: "Rate limit",
};
const feature: HarnessFeature = {
  id: 1,
  slug: "test",
  title: null,
  spec_raw: null,
  status: "in_progress",
  review_rounds: 1,
  gate_attempts: 0,
  crates: null,
  acceptance: null,
  source_issue: null,
  created_at: null,
  orchestrator_session_id: null,
  workspace_id: null,
  workspace_path: null,
  revision: 1,
  attempts: [attempt],
  blocked_reason: null,
};
const job: Job = {
  id: "j1",
  provider_id: "codex",
  workspace_id: "w",
  role: "Executor",
  feature_id: 1,
  parent_session_id: null,
  state: "Running",
  summary: "",
  prompt: "",
  provider_session_id: null,
  exit_code: null,
  last_line: null,
  started_at: attempt.started_at,
  finished_at: null,
  log_path: "",
};

it("distinguishes idle specifications and human approval from running agents", () => {
  expect(featureProgress({ status: "pending", attempts: null }, []).stages[0].state).toBe("ready");
  expect(
    featureProgress({ status: "spec_ready", attempts: [] }, []).stages.map((s) => s.state),
  ).toEqual(["complete", "waiting", "upcoming", "upcoming", "upcoming"]);
});
it("returns review to upcoming when changes are requested", () => {
  expect(featureProgress(feature, [job]).stages.map((s) => s.state)).toEqual([
    "complete",
    "complete",
    "running",
    "upcoming",
    "upcoming",
  ]);
});
it("places blockages on the last attempt and resets the cycle on reopen", () => {
  expect(featureProgress({ ...feature, status: "blocked" }, []).stages[2].state).toBe("blocked");
  expect(featureProgress({ ...feature, status: "pending" }, []).stages[2].state).toBe("upcoming");
  expect(
    featureProgress({ status: "future", attempts: null }, []).stages.every(
      (s) => s.state === "unknown",
    ),
  ).toBe(true);
});
it("preserves provider, outcome and timing after restart", () => {
  expect(agentRuns(feature, [], [])[0]).toMatchObject({
    provider: "codex",
    attempt: 2,
    state: "failed",
    detail: "Rate limit",
    ended: attempt.settled_at,
  });
});
it("merges attempts, jobs and events once and retains the authoritative outcome", () => {
  const rows = agentRuns(
    feature,
    [job],
    [{ ts: attempt.started_at, kind: "impl_started", job: "j1", detail: "" }],
  );
  expect(rows).toHaveLength(1);
  expect(rows[0].state).toBe("failed");
  expect(rows[0].job).toBe(job);
});
it("does not claim a missing process is running", () => {
  expect(
    agentRuns(
      { ...feature, attempts: [{ ...attempt, settled_at: null, outcome: null }] },
      [],
      [],
    )[0].state,
  ).toBe("Unconfirmed");
});
it("retains legacy transcripts without inventing results", () => {
  expect(
    agentRuns(
      { ...feature, attempts: null },
      [],
      [{ ts: "", kind: "spec_started", job: "old", detail: "" }],
    )[0],
  ).toMatchObject({ jobId: "old", state: "Recorded" });
});
it("formats only valid settled durations", () => {
  expect(runDuration(attempt.started_at, attempt.settled_at)).toBe("2m 15s");
  expect(runDuration(attempt.started_at, null)).toBeNull();
  expect(runDuration("invalid", attempt.settled_at)).toBeNull();
});
it("extracts event reasons and preserves unstructured details", () => {
  expect(eventSummary('{"reason":"Rate limit","job":"j1"}')).toBe("Rate limit");
  expect(eventSummary("plain message")).toBe("plain message");
});
