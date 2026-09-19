import { sendWorkbenchCommand } from "../../runtime/workbench";
import { setLoading } from "../../state/loading";
import { setGitStore } from "./state";
import type { JuvaKind } from "../../contracts/workbench";

export async function loadDiff(
  workspace: string,
  contextLines: number | null = null,
): Promise<void> {
  await sendWorkbenchCommand({ type: "load_diff", workspace, context_lines: contextLines });
}

/** Summary only; excludes patches. */
export async function loadSessionChanges(session: string): Promise<void> {
  await sendWorkbenchCommand({ type: "load_session_changes", session });
}

/** The review tab's read: the whole checkout since its sessions began. */
export async function loadWorkspaceReview(
  workspace: string,
  contextLines: number | null = null,
): Promise<void> {
  await sendWorkbenchCommand({
    type: "load_workspace_review",
    workspace,
    context_lines: contextLines,
  });
}

export async function loadRebaseState(workspace: string): Promise<void> {
  await sendWorkbenchCommand({ type: "load_rebase_state", workspace });
}

export async function continueRebase(workspace: string): Promise<void> {
  await sendWorkbenchCommand({ type: "continue_rebase", workspace });
}

export async function abortRebase(workspace: string): Promise<void> {
  await sendWorkbenchCommand({ type: "abort_rebase", workspace });
}

export async function markConflictResolved(workspace: string, paths: string[]): Promise<void> {
  await sendWorkbenchCommand({ type: "mark_conflict_resolved", workspace, paths });
}

export async function listBranches(project: string): Promise<void> {
  await sendWorkbenchCommand({ type: "list_branches", project });
}

/**
 * Start a Juva draft. The text arrives on `runtime:juva_draft`.
 *
 * The `[juva]` endpoint opens a socket, so the daemon acks when the work
 * starts. The loading flag is set here rather than on the answer because there
 * is no answer to set it from.
 */
export async function draftWithJuva(workspace: string, kind: JuvaKind): Promise<void> {
  setLoading("juva", true);
  setGitStore("juvaError", null);
  await sendWorkbenchCommand({ type: "draft_with_juva", workspace, kind });
}

export async function applyJuvaDraft(
  workspace: string,
  kind: JuvaKind,
  title: string,
  body: string,
): Promise<void> {
  await sendWorkbenchCommand({ type: "apply_juva_draft", workspace, kind, title, body });
}
