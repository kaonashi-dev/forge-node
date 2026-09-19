import { sendWorkbenchCommand } from "../../runtime/workbench";

/** Coalesced by the daemon; the answer arrives as `PullRequestsUpdated`. */
export async function refreshPullRequests(): Promise<void> {
  await sendWorkbenchCommand({ type: "refresh_pull_requests" });
}
