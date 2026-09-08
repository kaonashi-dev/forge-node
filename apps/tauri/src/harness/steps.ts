// Reading a feature's steps out of what two different places remember: the
// daemon's job list, and the event log on disk.

import type { Job } from "../runtime/types";
import type { HarnessEvent } from "./types";

/** Which step of the cycle a job is, named the way the harness names it. */
export function jobStepLabel(role: string): string {
  switch (role) {
    case "Orchestrator":
      return "spec";
    case "Executor":
      return "implement";
    case "Reviewer":
      return "review";
    default:
      return "step";
  }
}

/** `spec · claude`: the step, then who ran it. */
export function jobLabel(job: Job): string {
  return `${jobStepLabel(job.role)} · ${job.provider_id}`;
}

/** Badge tone for a job state, shared by the panel and the tab. */
export type JobTone = "neutral" | "good" | "warn" | "bad";

export function jobStateTone(state: string): JobTone {
  switch (state) {
    case "Running":
      return "warn";
    case "Succeeded":
      return "good";
    case "Failed":
      return "bad";
    default:
      return "neutral";
  }
}

/** One step the event log remembers and this daemon does not. */
export type RecordedStep = { label: string; job: string };

/**
 * Steps on record in the log but not in the daemon's memory.
 *
 * A job is a row in memory; restart the daemon and every step of every feature
 * disappears from the tab while its transcript stays on disk. The `*_started`
 * events name the job that ran, and the daemon serves a log by id whether or
 * not it recalls running it.
 */
export function recordedSteps(timeline: HarnessEvent[], jobs: Job[]): RecordedStep[] {
  const known = new Set(jobs.map((job) => job.id));
  const seen = new Set<string>();
  const out: RecordedStep[] = [];
  for (const event of timeline) {
    if (event.job === null || known.has(event.job) || seen.has(event.job)) continue;
    const label = STARTED[event.kind];
    if (label === undefined) continue;
    seen.add(event.job);
    out.push({ label, job: event.job });
  }
  return out;
}

const STARTED: Record<string, string> = {
  spec_started: "spec",
  impl_started: "implement",
  review_started: "review",
};
