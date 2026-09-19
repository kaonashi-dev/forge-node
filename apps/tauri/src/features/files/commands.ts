import { batch } from "solid-js";
import { normalizeError, sendWorkbenchCommand } from "../../runtime/workbench";
import { loading, setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";
import { directories, directoryTree } from "./directories/directoryState";
import { sameListing } from "./index/fileInvalidation";
import {
  pathOperations,
  PathOperationError,
  requestIdentity,
  type PathOperation,
  type PathOperationResult,
} from "./operations/operations";
import { validatePath, validateRename } from "../../shared/paths";
import { createImageReader } from "./preview/previewImages";
import { filesStore, setFilesStore } from "./state";
import type { FileTree, SearchKind } from "../../contracts/workbench";

export type WorkbenchReadSurface = "tree" | "file";

export function beginWorkbenchRequest(surface: WorkbenchReadSurface): void {
  setLoading(surface, true);
  if (surface === "tree") setFilesStore("treeError", null);
  else setFilesStore("fileError", null);
}

export function failWorkbenchRequest(surface: WorkbenchReadSurface, error: unknown): void {
  const message = normalizeError(error).message;
  if (surface === "tree") setFilesStore("treeError", message);
  else setFilesStore("fileError", message);
  setLoading(surface, false);
}

export async function loadFileTree(workspace: string): Promise<void> {
  if (workspace !== activeWorkspace()) return;
  const request_id = requestIdentity();
  batch(() => {
    setFilesStore("treeRequest", { request_id, version: filesStore.treeVersion });
    beginWorkbenchRequest("tree");
  });
  try {
    await sendWorkbenchCommand({ type: "load_file_tree", workspace, request_id });
  } catch (error) {
    failFileIndex({ workspace, request_id, error: normalizeError(error).message });
  }
}

export function acceptFileIndex(answer: {
  workspace: string;
  request_id: string;
  tree: FileTree;
}): void {
  const request = filesStore.treeRequest;
  if (answer.workspace !== activeWorkspace() || request?.request_id !== answer.request_id) return;
  const stale = request.version !== filesStore.treeVersion;
  batch(() => {
    if (!sameListing(filesStore.tree, answer.tree)) setFilesStore("tree", answer.tree);
    setFilesStore({ treeRequest: null, treeError: null, treeStale: stale });
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
    failure.workspace !== activeWorkspace() ||
    filesStore.treeRequest?.request_id !== failure.request_id
  )
    return;
  batch(() => {
    setFilesStore({ treeRequest: null, treeError: failure.error });
    setLoading("tree", false);
  });
}

directories.configure((request) =>
  sendWorkbenchCommand({ type: "load_file_directory", ...request }),
);
export const ensureDirectory = directories.ensure;
export const invalidateDirectories = directories.invalidate;
export const setDirectoryInterests = directories.setInterests;

export async function loadFileDirectory(workspace: string, path: string): Promise<void> {
  ensureDirectory(workspace, path, true);
}

/** Share one demand-driven navigation read between the palette and terminal references. */
export function warmFileTree(workspace: string): void {
  if (
    workspace !== activeWorkspace() ||
    (filesStore.tree && !filesStore.treeStale) ||
    loading.tree ||
    filesStore.treeError
  )
    return;
  void loadFileTree(workspace);
}

export async function openFile(workspace: string, path: string): Promise<void> {
  await sendWorkbenchCommand({ type: "open_file", workspace, path });
}

/** An image a Markdown preview names; answered on `workbench:image`. */
export async function loadImage(workspace: string, path: string): Promise<void> {
  await sendWorkbenchCommand({ type: "load_image", workspace, path });
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
    sendWorkbenchCommand({ type: "create_path", workspace, path, directory, operation_id }),
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
    sendWorkbenchCommand({ type: "rename_path", workspace, from, to, operation_id }),
  );
}

export async function deletePath(workspace: string, path: string): Promise<PathOperationResult> {
  const operation: PathOperation = { workspace, kind: "delete", from: path };
  validateOperation(operation, validatePath(path));
  return pathOperations.run(operation, (operation_id) =>
    sendWorkbenchCommand({ type: "delete_path", workspace, path, operation_id }),
  );
}

/** Conditioned on the revision of the last read (ADR-012). */
export async function saveFile(
  workspace: string,
  path: string,
  text: string,
  revision: string,
): Promise<void> {
  await sendWorkbenchCommand({ type: "save_file", workspace, path, text, revision });
}

export async function searchFiles(
  workspace: string,
  query: string,
  kind: SearchKind = "name",
  limit: number | null = null,
): Promise<void> {
  // Palette Name search shares this request, not find-in-files' freshness token.
  if (kind === "content") {
    setFilesStore("searchReadVersion", filesStore.treeVersion);
  }
  await sendWorkbenchCommand({ type: "search_files", workspace, query, kind, limit });
}
