/**
 * Harness feature state, mirrored from `crates/domain/src/harness.rs`.
 *
 * The daemon parses `harness/features.json` and the markdown under
 * `harness/` and hands them over already shaped; nothing here re-parses a
 * file, and nothing here is persisted — a feature row is runtime-only on the
 * wire, with no SQLite column behind it.
 */

/** One row of `harness/features.json`. */
export type HarnessFeature = {
  id: number;
  slug: string;
  title: string | null;
  spec_raw: string | null;
  status: string;
  review_rounds: number | null;
  gate_attempts: number | null;
  crates: string[] | null;
  acceptance: string[] | null;
  source_issue: number | null;
  created_at: string | null;
  /** The orchestrator session driving this feature, when one is running. */
  orchestrator_session_id: string | null;
  /** The checkout it is being implemented in; `null` when registered outside Forge. */
  workspace_id: string | null;
  workspace_path: string | null;
  /**
   * How many times the row has been written.
   *
   * Sent back with a decision so one taken against a stale view is a no-op:
   * a replayed *Approve* used to resolve a second gate and start a second
   * implementer in the same worktree.
   */
  revision: number | null;
  /** Every run of every step, in order — provider, timing and outcome. */
  attempts: HarnessAttempt[] | null;
  /** Why the feature is blocked, without reading the timeline for it. */
  blocked_reason: string | null;
};

/**
 * One run of one step.
 *
 * A retry is a *new* attempt rather than a mutation of the previous one, so
 * the row reads as "the implementer was started three times" and the stepper
 * can show which provider ran what, and for how long.
 */
export type HarnessAttempt = {
  step: HarnessStep;
  n: number;
  job: string | null;
  provider: string | null;
  transport: string | null;
  started_at: string;
  settled_at: string | null;
  outcome: "succeeded" | "failed" | "cancelled" | "blocked" | "superseded" | null;
  detail: string | null;
};

export type HarnessFeatureList = {
  project: string | null;
  features: HarnessFeature[];
  /** `false` when the project has no `harness/` directory at all. */
  initialized: boolean;
};

/** One line of `harness/progress/events_<id>.jsonl`. */
export type HarnessEvent = {
  ts: string;
  kind: string;
  detail: string;
  /**
   * The headless run this event started, when it started one.
   *
   * Jobs live in the daemon's memory, so a restart forgets every row while
   * the transcripts stay on disk; this id is what still points at one.
   */
  job: string | null;
};

/** Which markdown artefact to read under `harness/progress/` or `harness/specs/`. */
export type HarnessArtifactKind =
  | "Gate"
  | "Context"
  | "Impl"
  | "Review"
  | "Requirements"
  | "Design"
  | "Tasks"
  | "Current";

/** The artefacts a feature tab offers, in the order the cycle produces them. */
export const ARTIFACTS: { kind: HarnessArtifactKind; label: string }[] = [
  { kind: "Requirements", label: "Requirements" },
  { kind: "Design", label: "Design" },
  { kind: "Tasks", label: "Tasks" },
  { kind: "Gate", label: "Gate" },
  { kind: "Impl", label: "Implementation" },
  { kind: "Review", label: "Review" },
  { kind: "Context", label: "Context" },
  { kind: "Current", label: "Current" },
];

/**
 * A human decision at the gate, or a step the human starts.
 *
 * Deliberately not the same list as `HarnessStep`: approving a spec is not a
 * step that can be *run*, it is the one moment the machine stops and waits.
 */
export type HarnessAdvanceAction =
  | "ApproveSpec"
  | "ReviseSpec"
  | { Block: { reason: string } }
  | "StartImplement"
  | "StartReview"
  /** Run the step a blocked feature died on, once the machine's budget is spent. */
  | "RetryStep"
  /** Take a `done` or `blocked` feature back to `pending`. */
  | "Reopen";

/** One step of the cycle, as a thing that can be run as a job. */
export type HarnessStep = "Spec" | "Implement" | "Review";

/**
 * Status → how the row reads.
 *
 * The daemon writes exactly six: `pending`, `spec_ready`, `in_progress`,
 * `in_review`, `done`, `blocked` — and only through
 * `harness_service::transition`, which is the one place a status is decided.
 * `spec_ready` is the only one that stops the machine, which is why it is the
 * only one that reaches the attention bar.
 */
export type FeaturePhase = "gate" | "running" | "done" | "blocked" | "idle";

export function featurePhase(feature: HarnessFeature): FeaturePhase {
  switch (feature.status) {
    case "spec_ready":
      return "gate";
    case "pending":
    case "in_progress":
    case "in_review":
      return "running";
    case "done":
      return "done";
    case "blocked":
      return "blocked";
    default:
      return "idle";
  }
}

/** The status line under a feature's name. */
export function statusLabel(status: string): string {
  switch (status) {
    case "spec_ready":
      return "awaiting approval";
    case "in_progress":
      return "in progress";
    case "in_review":
      return "in review";
    default:
      return status.replaceAll("_", " ");
  }
}

/** The short badge the feature *list* shows. */
export function statusBadge(status: string): string {
  switch (status) {
    case "spec_ready":
      return "approve?";
    case "in_progress":
      return "building";
    case "in_review":
      return "review";
    default:
      return status.replaceAll("_", " ");
  }
}

/** Whether the feature still occupies its checkout (anything not finished or abandoned). */
export function featureIsOpen(feature: HarnessFeature): boolean {
  return feature.status !== "done" && feature.status !== "blocked";
}

export function featureNeedsGate(feature: HarnessFeature): boolean {
  return feature.status === "spec_ready";
}

export function featureLabel(feature: HarnessFeature): string {
  return feature.title ?? feature.slug;
}

/**
 * The row the feature tab draws.
 *
 * The list is updated by every advance; the detail is a later read of the
 * same row plus its timeline. Preferring detail unconditionally is how a
 * tab kept showing the gate after the list already said the implementer
 * was running: a stale `load_harness_detail` landing after `harness:advanced`
 * overwrote the newer row. `revision` is the write clock.
 */
export function displayedFeature(
  id: number,
  detail: HarnessFeature | null,
  features: HarnessFeature[],
): HarnessFeature | null {
  const listed = features.find((item) => item.id === id) ?? null;
  if (detail?.id !== id) return listed;
  if (listed === null) return detail;
  return (listed.revision ?? 0) > (detail.revision ?? 0) ? listed : detail;
}

/**
 * The step this feature is waiting to have run, and what the button says.
 *
 * A feature at rest with nothing running is a dead end without the `pending`
 * arm: the spec step is launched at registration and after a revise, and a
 * feature that reached `pending` any other way — a revise under an older
 * build, a step that died with its daemon — would have no way forward at all.
 *
 * `null` while a job is running, and at the gate: approving a spec is a
 * human's answer, not a step to re-run.
 */
export function runnableStep(
  feature: HarnessFeature,
  running: boolean,
): { step: HarnessStep; label: string } | null {
  if (running) return null;
  switch (feature.status) {
    case "pending":
      return { step: "Spec", label: "Run spec" };
    case "in_progress":
      return { step: "Implement", label: "Run implement" };
    case "in_review":
      return { step: "Review", label: "Run review" };
    default:
      return null;
  }
}

/** Whether the artefact documents are worth offering yet. */
export function hasArtifacts(feature: HarnessFeature): boolean {
  return ["in_progress", "in_review", "blocked", "done"].includes(feature.status);
}

/** The filters the features panel offers. */
export const FEATURE_FILTERS = ["All", "Active", "Approve?", "Done", "Blocked"] as const;
export type FeatureFilter = (typeof FEATURE_FILTERS)[number];

export function matchesFilter(status: string, filter: FeatureFilter): boolean {
  switch (filter) {
    case "All":
      return true;
    case "Active":
      return ["pending", "in_progress", "in_review"].includes(status);
    case "Approve?":
      return status === "spec_ready";
    case "Done":
      return status === "done";
    case "Blocked":
      return status === "blocked";
  }
}

/**
 * The features stopped at the human gate, oldest id first.
 *
 * By id and not by recency: ids are minted in order, so the oldest gate is the
 * one at risk of being forgotten — the same reason the rail puts the
 * longest-waiting session first.
 *
 * Never sorts the caller's array in place: it is the store's list, and a store
 * that reorders itself when something reads it is a store nothing can trust.
 */
export function gatedFeatures(features: HarnessFeature[]): HarnessFeature[] {
  return features
    .filter((feature) => feature.status === "spec_ready")
    .slice()
    .sort((left, right) => left.id - right.id);
}
