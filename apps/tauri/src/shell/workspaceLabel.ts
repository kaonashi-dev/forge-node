import type { Workspace } from "../runtime/types";

export function pathBasename(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

type WorkspaceLike = Pick<Workspace, "branch" | "path">;

/** Folder basename for rail meta — never the full checkout path. */
export function workspaceFolderLabel(workspace: WorkspaceLike): string {
  return pathBasename(workspace.path);
}

/** Branch line under a workspace card title — branch, or folder when detached. */
export function workspaceBranchMeta(workspace: WorkspaceLike): string {
  return workspace.branch ?? workspaceFolderLabel(workspace);
}
