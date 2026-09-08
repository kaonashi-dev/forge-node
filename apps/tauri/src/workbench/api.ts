import { invoke } from "@tauri-apps/api/core";
import { setLoading, setWorkbenchStore } from "../store/workbenchStore";
import { markApplying, setSharesStore } from "../store/sharesStore";
import type { JuvaKind, SearchKind } from "./types";

/**
 * Workbench reads go to their own worker on the host, never to the thread that
 * carries terminal input: a `git diff` of a large checkout is seconds of
 * subprocess, and that thread also writes every keystroke to the PTY.
 */
async function send(command: Record<string, unknown>): Promise<void> {
  try {
    await invoke("send_workbench_command", { command });
  } catch (error) {
    throw normalizeError(error);
  }
}

function normalizeError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (typeof error === "string") return new Error(error);
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return new Error(message);
  }
  try {
    return new Error(JSON.stringify(error) ?? String(error));
  } catch {
    return new Error(String(error));
  }
}

export type WorkbenchReadSurface = "tree" | "file";

export function beginWorkbenchRequest(surface: WorkbenchReadSurface): void {
  setLoading(surface, true);
  if (surface === "tree") setWorkbenchStore("treeError", null);
  else setWorkbenchStore("fileError", null);
}

export function failWorkbenchRequest(surface: WorkbenchReadSurface, error: unknown): void {
  const message = normalizeError(error).message;
  if (surface === "tree") setWorkbenchStore("treeError", message);
  else setWorkbenchStore("fileError", message);
  setLoading(surface, false);
}

export async function loadDiff(
  workspace: string,
  contextLines: number | null = null,
): Promise<void> {
  await send({ type: "load_diff", workspace, context_lines: contextLines });
}

/** The changes split's read: one session, no patches (§16.7). */
export async function loadSessionChanges(session: string): Promise<void> {
  await send({ type: "load_session_changes", session });
}

/** The review tab's read: the whole checkout since its sessions began. */
export async function loadWorkspaceReview(
  workspace: string,
  contextLines: number | null = null,
): Promise<void> {
  await send({ type: "load_workspace_review", workspace, context_lines: contextLines });
}

/**
 * The tail of a session's terminal, for a handoff prompt (§16.8).
 *
 * `null` for either bound takes the daemon's default, which is sized so the
 * prompt built from it still fits in one argv entry.
 */
export async function loadSessionTranscript(
  session: string,
  maxLines: number | null = null,
  maxBytes: number | null = null,
): Promise<void> {
  await send({
    type: "load_session_transcript",
    session,
    max_lines: maxLines,
    max_bytes: maxBytes,
  });
}

export async function loadFileTree(workspace: string): Promise<void> {
  await send({ type: "load_file_tree", workspace });
}

export async function openFile(workspace: string, path: string): Promise<void> {
  await send({ type: "open_file", workspace, path });
}

/**
 * A11: the three path mutations, all of which answer by re-listing the tree.
 *
 * ADR-012 in full: the WebView names a workspace-relative path and the daemon
 * does the work. Nothing here touches a filesystem, and the daemon refuses a
 * path that leaves the checkout, an overwrite, and the checkout root itself.
 */
export async function createPath(
  workspace: string,
  path: string,
  directory: boolean,
): Promise<void> {
  await send({ type: "create_path", workspace, path, directory });
}

export async function renamePath(workspace: string, from: string, to: string): Promise<void> {
  await send({ type: "rename_path", workspace, from, to });
}

export async function deletePath(workspace: string, path: string): Promise<void> {
  await send({ type: "delete_path", workspace, path });
}

/** Conditioned on the revision of the last read (ADR-012). */
export async function saveFile(
  workspace: string,
  path: string,
  text: string,
  revision: string,
): Promise<void> {
  await send({ type: "save_file", workspace, path, text, revision });
}

export async function searchFiles(
  workspace: string,
  query: string,
  kind: SearchKind = "name",
  limit: number | null = null,
): Promise<void> {
  await send({ type: "search_files", workspace, query, kind, limit });
}

export async function loadRebaseState(workspace: string): Promise<void> {
  await send({ type: "load_rebase_state", workspace });
}

export async function continueRebase(workspace: string): Promise<void> {
  await send({ type: "continue_rebase", workspace });
}

export async function abortRebase(workspace: string): Promise<void> {
  await send({ type: "abort_rebase", workspace });
}

export async function markConflictResolved(workspace: string, paths: string[]): Promise<void> {
  await send({ type: "mark_conflict_resolved", workspace, paths });
}

export async function listBranches(project: string): Promise<void> {
  await send({ type: "list_branches", project });
}

/** Coalesced by the daemon; the answer arrives as `PullRequestsUpdated`. */
export async function refreshPullRequests(): Promise<void> {
  await send({ type: "refresh_pull_requests" });
}

export async function loadUsageAnalytics(windowDays: number | null = null): Promise<void> {
  await send({ type: "load_usage_analytics", window_days: windowDays });
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
  setWorkbenchStore("juvaError", null);
  await send({ type: "draft_with_juva", workspace, kind });
}

export async function applyJuvaDraft(
  workspace: string,
  kind: JuvaKind,
  title: string,
  body: string,
): Promise<void> {
  await send({ type: "apply_juva_draft", workspace, kind, title, body });
}

// --- Shared files between worktrees (§14.2) ---------------------------------

/**
 * These are workbench reads for the same reason a diff is: the daemon answers
 * `DetectShareCandidates` with a `git status --ignored` plus a bounded walk,
 * and an apply can run `pnpm install`. None of that may share the thread that
 * carries keystrokes.
 */
export async function detectShareCandidates(project: string): Promise<void> {
  setSharesStore("scanning", true);
  await send({ type: "detect_share_candidates", project });
}

export async function previewShares(workspace: string): Promise<void> {
  await send({ type: "preview_shares", workspace });
}

export async function loadShareStatus(workspace: string): Promise<void> {
  await send({ type: "load_share_status", workspace });
}

/** Ack means *started*: the outcome arrives as `runtime:shares_applied`. */
export async function applyShares(workspace: string, only?: string[]): Promise<void> {
  markApplying(workspace);
  await send({ type: "apply_shares", workspace, only: only ?? null });
}

export async function adoptIntoShareStore(project: string, path: string): Promise<void> {
  await send({ type: "adopt_into_share_store", project, path });
}

export async function materializeFromShareStore(
  project: string,
  path: string,
  workspace?: string,
): Promise<void> {
  await send({
    type: "materialize_from_share_store",
    project,
    path,
    workspace: workspace ?? null,
  });
}
