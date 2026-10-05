import { sendWorkbenchCommand } from "../../runtime/workbench";

/** Coalesced by the daemon; the answer arrives as `PullRequestsUpdated`. */
export async function refreshPullRequests(): Promise<void> {
  await sendWorkbenchCommand({ type: "refresh_pull_requests" });
}

/**
 * Commit everything once, on `branch` when the checkout is on its default,
 * push and open the PR. Acks when the work starts; the outcome arrives on
 * `runtime:pull_request_opened`.
 */
export async function commitAndOpenPullRequest(
  workspace: string,
  draft: { branch: string; commit_message: string; title: string; body: string },
): Promise<void> {
  const branch = draft.branch.trim();
  await sendWorkbenchCommand({
    type: "commit_and_open_pull_request",
    workspace,
    branch: branch ? branch : null,
    commit_message: draft.commit_message,
    title: draft.title,
    body: draft.body,
  });
}
