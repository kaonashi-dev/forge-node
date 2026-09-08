import { type Job } from "../runtime/types";
import { jobStepLabel, recordedSteps } from "./steps";
import type { HarnessEvent, HarnessFeature } from "./types";

const STAGES = ["Specification", "Approval", "Implementation", "Review", "Complete"];
const STAGE_INDEX: Record<string, number> = {
  pending: 0,
  spec_ready: 1,
  in_progress: 2,
  in_review: 3,
  done: 4,
};
const STEP_INDEX: Record<string, number> = { Spec: 0, Implement: 2, Review: 3 };

export function featureProgress(feature: Pick<HarnessFeature, "status" | "attempts">, jobs: Job[]) {
  const attempts = feature.attempts ?? [];
  const blocked = feature.status === "blocked";
  const current = blocked
    ? (STEP_INDEX[attempts.at(-1)?.step ?? "Spec"] ?? 0)
    : STAGE_INDEX[feature.status];
  const live = jobs.some((job) => job.state === "Running");
  const queued = jobs.some((job) => job.state === "Queued");
  const stages = STAGES.map((label, index) => ({
    label,
    state:
      current === undefined
        ? "unknown"
        : feature.status === "done" || index < current
          ? "complete"
          : index > current
            ? "upcoming"
            : blocked
              ? "blocked"
              : index === 1
                ? "waiting"
                : live
                  ? "running"
                  : queued
                    ? "queued"
                    : "ready",
  }));
  const summary =
    current === undefined
      ? `Unknown stage: ${feature.status}`
      : feature.status === "done"
        ? "Review approved · cycle complete"
        : blocked
          ? `${STAGES[current]} blocked · intervention needed`
          : current === 1
            ? "Specification ready · waiting for your decision"
            : `${STAGES[current]} · ${live ? "agent running" : queued ? "agent queued" : "ready to run"}`;
  return { stages, summary };
}

export type AgentRun = {
  key: string;
  jobId: string | null;
  job: Job | undefined;
  role: string;
  provider: string;
  attempt: number | null;
  state: string;
  started: string;
  ended: string | null;
  detail: string | null;
};

/** Persisted outcomes win over a late job exit after a human blocks an attempt. */
export function agentRuns(
  feature: HarnessFeature,
  jobs: Job[],
  timeline: HarnessEvent[],
): AgentRun[] {
  const byId = new Map(jobs.map((job) => [job.id, job]));
  const known = new Set<string>();
  const rows: AgentRun[] = (feature.attempts ?? []).map((attempt, index) => {
    const job = attempt.job ? byId.get(attempt.job) : undefined;
    if (attempt.job) known.add(attempt.job);
    return {
      key: `attempt-${index}`,
      jobId: attempt.job,
      job,
      role:
        attempt.step === "Spec"
          ? "Specification author"
          : attempt.step === "Implement"
            ? "Implementer"
            : "Reviewer",
      provider: attempt.provider ?? job?.provider_id ?? "Provider not recorded",
      attempt: attempt.n,
      state: attempt.outcome ?? job?.state ?? "Unconfirmed",
      started: attempt.started_at,
      ended: attempt.settled_at ?? job?.finished_at ?? null,
      detail: attempt.detail,
    };
  });
  for (const job of jobs) {
    if (known.has(job.id)) continue;
    known.add(job.id);
    rows.push({
      key: job.id,
      jobId: job.id,
      job,
      role: jobStepLabel(job.role),
      provider: job.provider_id,
      attempt: null,
      state: job.state,
      started: job.started_at,
      ended: job.finished_at,
      detail: null,
    });
  }
  for (const step of recordedSteps(timeline, jobs)) {
    if (known.has(step.job)) continue;
    known.add(step.job);
    rows.push({
      key: step.job,
      jobId: step.job,
      job: undefined,
      role: step.label,
      provider: "Provider not recorded",
      attempt: null,
      state: "Recorded",
      started: "",
      ended: null,
      detail: null,
    });
  }
  return rows.reverse();
}

export function runTone(state: string): "neutral" | "good" | "warn" | "bad" {
  switch (state.toLowerCase()) {
    case "succeeded":
      return "good";
    case "running":
    case "queued":
      return "warn";
    case "failed":
    case "blocked":
      return "bad";
    default:
      return "neutral";
  }
}

export function runDuration(started: string, ended: string | null): string | null {
  if (!ended) return null;
  const seconds = Math.floor((Date.parse(ended) - Date.parse(started)) / 1000);
  if (!Number.isFinite(seconds) || seconds < 0) return null;
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

export function eventSummary(detail: string): string {
  try {
    const data: unknown = JSON.parse(detail);
    if (typeof data !== "object" || data === null || Array.isArray(data)) return detail;
    const fields = data as Record<string, unknown>;
    return (
      ["reason", "verdict", "decision", "message", "step"]
        .flatMap((key) => (typeof fields[key] === "string" ? [fields[key]] : []))
        .join(" · ") || detail
    );
  } catch {
    return detail;
  }
}
