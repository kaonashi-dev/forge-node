import { invoke } from "@tauri-apps/api/core";
import { batch } from "solid-js";
import {
  invalidateFileIndex,
  setLoading,
  setWorkbenchStore,
  workbenchStore,
} from "../store/workbenchStore";
import { markApplying, setSharesStore } from "../store/sharesStore";
import { directories, directoryTree } from "./directoryState";
import {
  pathOperations,
  PathOperationError,
  requestIdentity,
  type PathOperation,
  type PathOperationResult,
} from "./operations";
import { parentPath, validatePath, validateRename } from "./pathOperations";
import { createImageReader } from "./previewImages";
import type { FileTree, JuvaKind, SearchKind } from "./types";
import { sameListing } from "./fileInvalidation";

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

/** The two sides of a refused editor save, for the conflict banner. */
export async function loadEditorConflict(session: string): Promise<void> {
  await send({ type: "load_editor_conflict", session });
}

/** Take disk: replace the editor's buffer with what is on disk now. */
export async function reloadEditorBuffer(session: string): Promise<void> {
  await send({ type: "reload_editor_buffer", session });
}

/** Keep mine: write the editor's draft over what is on disk now. */
export async function overwriteEditorBuffer(session: string): Promise<void> {
  await send({ type: "overwrite_editor_buffer", session });
}

export async function setEditorAutosave(session: string, autosave: boolean): Promise<void> {
  await send({ type: "set_editor_autosave", session, autosave });
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

/**
 * A discovered run's conversation, for a handoff prompt off History (§13.5).
 *
 * The on-disk twin of `loadSessionTranscript`: the run has no PTY, so the
 * daemon reads its transcript file. Named by identity — a path on the wire
 * would be an arbitrary-file primitive.
 */
export async function loadExternalTranscript(
  session: string,
  provider: string,
  profile: string | null = null,
  maxTurns: number | null = null,
  maxBytes: number | null = null,
): Promise<void> {
  await send({
    type: "load_external_transcript",
    session,
    provider,
    profile,
    max_turns: maxTurns,
    max_bytes: maxBytes,
  });
}

/** Remove a discovered run's transcript from disk. Never the checkout. */
export async function deleteExternalSession(
  session: string,
  provider: string,
  profile: string | null = null,
): Promise<void> {
  await send({ type: "delete_external_session", session, provider, profile });
}

export async function loadFileTree(workspace: string): Promise<void> {
  if (workspace !== workbenchStore.workspace) return;
  const request_id = requestIdentity();
  batch(() => {
    setWorkbenchStore("treeRequest", { request_id, version: workbenchStore.treeVersion });
    beginWorkbenchRequest("tree");
  });
  try {
    await send({ type: "load_file_tree", workspace, request_id });
  } catch (error) {
    failFileIndex({ workspace, request_id, error: normalizeError(error).message });
  }
}

export function acceptFileIndex(answer: {
  workspace: string;
  request_id: string;
  tree: FileTree;
}): void {
  const request = workbenchStore.treeRequest;
  if (answer.workspace !== workbenchStore.workspace || request?.request_id !== answer.request_id)
    return;
  const stale = request.version !== workbenchStore.treeVersion;
  batch(() => {
    if (!sameListing(workbenchStore.tree, answer.tree)) setWorkbenchStore("tree", answer.tree);
    setWorkbenchStore({ treeRequest: null, treeError: null, treeStale: stale });
    setLoading("tree", false);
  });
  if (stale) warmFileTree(answer.workspace);
}

export function failFileIndex(failure: {
  workspace: string;
  request_id: string;
  error: string;
}): void {
  if (
    failure.workspace !== workbenchStore.workspace ||
    workbenchStore.treeRequest?.request_id !== failure.request_id
  )
    return;
  batch(() => {
    setWorkbenchStore({ treeRequest: null, treeError: failure.error });
    setLoading("tree", false);
  });
}

directories.configure((request) => send({ type: "load_file_directory", ...request }));
export const ensureDirectory = directories.ensure;
export const invalidateDirectories = directories.invalidate;
export const setDirectoryInterests = directories.setInterests;

export async function loadFileDirectory(workspace: string, path: string): Promise<void> {
  ensureDirectory(workspace, path, true);
}

/** Share one demand-driven navigation read between the palette and terminal references. */
export function warmFileTree(workspace: string): void {
  if (
    workspace !== workbenchStore.workspace ||
    (workbenchStore.tree && !workbenchStore.treeStale) ||
    workbenchStore.loading.tree ||
    workbenchStore.treeError
  )
    return;
  void loadFileTree(workspace);
}

export async function openFile(workspace: string, path: string): Promise<void> {
  await send({ type: "open_file", workspace, path });
}

/** An image a Markdown preview names; answered on `workbench:image`. */
export async function loadImage(workspace: string, path: string): Promise<void> {
  await send({ type: "load_image", workspace, path });
}

export const previewImageReader = createImageReader(loadImage);

function validateOperation(operation: PathOperation, error: string | null): void {
  if (error)
    throw new PathOperationError({
      ...operation,
      operation_id: requestIdentity(),
      success: false,
      uncertain: false,
      error,
    });
}

/** Resolves on the daemon result; a failed refresh cannot undo a successful mutation. */
export async function createPath(
  workspace: string,
  path: string,
  directory: boolean,
): Promise<PathOperationResult> {
  const operation: PathOperation = { workspace, kind: "create", to: path, directory };
  validateOperation(operation, validatePath(path));
  return pathOperations.run(operation, (operation_id) =>
    send({ type: "create_path", workspace, path, directory, operation_id }),
  );
}

export async function renamePath(
  workspace: string,
  from: string,
  to: string,
): Promise<PathOperationResult> {
  const tree = directoryTree();
  const directory =
    tree?.workspace_id === workspace
      ? tree.entries.find((entry) => entry.path === from)?.kind === "Directory"
      : undefined;
  const operation: PathOperation = { workspace, kind: "rename", from, to, directory };
  validateOperation(operation, validateRename(from, to));
  return pathOperations.run(operation, (operation_id) =>
    send({ type: "rename_path", workspace, from, to, operation_id }),
  );
}

export async function deletePath(workspace: string, path: string): Promise<PathOperationResult> {
  const operation: PathOperation = { workspace, kind: "delete", from: path };
  validateOperation(operation, validatePath(path));
  return pathOperations.run(operation, (operation_id) =>
    send({ type: "delete_path", workspace, path, operation_id }),
  );
}

pathOperations.subscribe((result) => {
  directories.operation(result, result.directory);
  invalidateFileIndex(result.workspace);
});
pathOperations.onUncertain((operation) => {
  for (const path of [operation.from, operation.to]) {
    if (path !== undefined) directories.ensure(operation.workspace, parentPath(path), true);
  }
  invalidateFileIndex(operation.workspace);
});

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
  // Palette Name search shares this request, not find-in-files' freshness token.
  if (kind === "content") {
    setWorkbenchStore("searchReadVersion", workbenchStore.treeVersion);
  }
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
