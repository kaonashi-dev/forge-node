/**
 * Pure reads for Settings → Storage.
 *
 * Kept out of the component so the prefix rules are testable without Solid's
 * server build: a path under `/x/worktrees2` is not a worktree of `/x`, and an
 * unmanaged checkout is not Forge's to count.
 */

export type StorageWorkspace = {
  managed_by_app: boolean;
  path: string;
};

export function legacyWorktreeCount(
  workspaces: readonly StorageWorkspace[],
  legacy: string | null,
): number {
  if (!legacy) return 0;
  const root = `${legacy.replace(/\/+$/, "")}/worktrees/`;
  return workspaces.filter(
    (workspace) => workspace.managed_by_app && workspace.path.startsWith(root),
  ).length;
}

/** Directory part of an absolute path, without a platform-specific API. */
export function directoryOf(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  return index > 0 ? trimmed.slice(0, index) : "/";
}
