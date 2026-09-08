import type { Workspace } from "../runtime/types";

export function pathBasename(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

type WorkspaceLike = Pick<Workspace, "branch" | "path">;

/** Folder basename for status bar and rail meta — never the full checkout path. */
export function workspaceFolderLabel(workspace: WorkspaceLike): string {
  return pathBasename(workspace.path);
}

/**
 * The second segment in the status bar when branch and folder name differ.
 * Returns `null` when repeating the branch would add nothing.
 */
export function workspacePathSegment(workspace: WorkspaceLike): string | null {
  const folder = workspaceFolderLabel(workspace);
  if (!workspace.branch || workspace.branch === folder) return null;
  return folder;
}

/** Branch line under a workspace card title — branch, or folder when detached. */
export function workspaceBranchMeta(workspace: WorkspaceLike): string {
  return workspace.branch ?? workspaceFolderLabel(workspace);
}
