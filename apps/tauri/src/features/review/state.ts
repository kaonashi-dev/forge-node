import { createStore } from "solid-js/store";
import type { ReviewReport, ReviewRun, ReviewSettings } from "../../contracts/workbench";

/** The newest run and the newest loaded report, per workspace. */
export const [reviewStore, setReviewStore] = createStore({
  latest: {} as Record<string, ReviewRun>,
  reports: {} as Record<string, ReviewReport>,
  errors: {} as Record<string, string | null>,
  configs: {} as Record<string, ReviewSettings>,
  /** The workspace the preset picker is open for. */
  picker: null as string | null,
});

/** Keeps the newest run; ids are UUIDv7, so a later id is a later run. */
export function applyRun(run: ReviewRun): boolean {
  const current = reviewStore.latest[run.workspace_id];
  if (current && current.id > run.id) return false;
  setReviewStore("latest", run.workspace_id, run);
  return true;
}
