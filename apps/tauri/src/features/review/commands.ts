import { openReview } from "../../navigation/viewsStore";
import { sendWorkbenchCommand } from "../../runtime/workbench";
import { focusWorkspace } from "../../state/workspace";
import type { Tier } from "../../contracts/workbench";
import { setReviewStore } from "./state";

export type ReviewOptions = {
  preset: string | null;
  tier: Tier | null;
  instructions: string | null;
};

/** Open the workspace's review tab and start a review of it; no preset is the repository's default. */
export async function startReview(
  workspace: string,
  options: ReviewOptions = { preset: null, tier: null, instructions: null },
): Promise<void> {
  focusWorkspace(workspace);
  openReview(workspace);
  setReviewStore("errors", workspace, null);
  await sendWorkbenchCommand({ type: "start_review", workspace, ...options });
}

export async function loadReviewConfig(workspace: string): Promise<void> {
  await sendWorkbenchCommand({ type: "load_review_config", workspace });
}

export async function cancelReview(review: string): Promise<void> {
  await sendWorkbenchCommand({ type: "cancel_review", review });
}

export async function loadReview(review: string): Promise<void> {
  await sendWorkbenchCommand({ type: "load_review", review });
}

export async function listReviews(workspace: string): Promise<void> {
  await sendWorkbenchCommand({ type: "list_reviews", workspace });
}
