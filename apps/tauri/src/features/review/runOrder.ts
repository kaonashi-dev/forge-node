import type { ReviewRun, ReviewStage } from "../../contracts/workbench";

const STAGES: readonly ReviewStage[] = ["Diff", "Pack", "Model", "Validate"];

function progress(run: ReviewRun): number {
  if (run.finished_at !== null) return STAGES.length + 1;
  return run.stage === null ? 0 : STAGES.indexOf(run.stage) + 1;
}

/** Whether `next` replaces `current`. UUIDv7 ids order runs; within one run a
    list read taken earlier must not undo a later update, so it never goes back. */
export function supersedes(current: ReviewRun | undefined, next: ReviewRun): boolean {
  if (!current) return true;
  if (current.id !== next.id) return next.id > current.id;
  return progress(next) >= progress(current);
}
