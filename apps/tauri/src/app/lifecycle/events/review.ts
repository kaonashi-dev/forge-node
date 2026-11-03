import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ReviewReport, ReviewRun, ReviewSettings } from "../../../contracts/workbench";
import { loadReview } from "../../../features/review/commands";
import { applyRun, reviewStore, setReviewStore } from "../../../features/review/state";
import type { Failure, Tagged } from "../workbenchAnswer";

/* Kept per workspace rather than for the focused one only: a review runs for
   minutes, and the person moves on while it does. */
export function bindReviewEvents(): Promise<UnlistenFn[]> {
  const loadIfDone = (run: ReviewRun) => {
    if (run.status === "Completed" && reviewStore.reports[run.workspace_id]?.run.id !== run.id) {
      void loadReview(run.id).catch(() => undefined);
    }
  };
  return Promise.all([
    listen<ReviewRun>("runtime:review_updated", ({ payload }) => {
      if (applyRun(payload)) loadIfDone(payload);
    }),
    listen<Tagged<ReviewReport>>("workbench:review", ({ payload: [workspace, report] }) => {
      setReviewStore("reports", workspace, report);
      applyRun(report.run);
    }),
    listen<Tagged<ReviewRun[]>>("workbench:reviews", ({ payload: [, runs] }) => {
      const newest = runs[0];
      if (newest && applyRun(newest)) loadIfDone(newest);
    }),
    listen<Tagged<ReviewSettings>>(
      "workbench:review_config",
      ({ payload: [workspace, config] }) => {
        setReviewStore("configs", workspace, config);
      },
    ),
    listen<Failure>("workbench:review_failed", ({ payload }) => {
      if (payload.workspace) setReviewStore("errors", payload.workspace, payload.error);
    }),
  ]);
}
