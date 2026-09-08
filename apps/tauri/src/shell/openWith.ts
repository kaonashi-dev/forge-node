import { isMac } from "../actions/keys";
import type { ShellSnapshot } from "../runtime/types";

/** `app_state` key holding the target the split button runs on a plain click. */
export const OPEN_WITH_KEY = "ui.open_with";

export type OpenTargetId = "zed" | "cursor" | "files";

export type OpenTarget = {
  id: OpenTargetId;
  label: string;
  /** The `open_in_editor` name; `null` routes to the file manager instead. */
  editor: string | null;
};

/** Not a constant: the file manager's name is only "Finder" on macOS. */
export function openTargets(): OpenTarget[] {
  return [
    { id: "zed", label: "Zed", editor: "zed" },
    { id: "cursor", label: "Cursor", editor: "cursor" },
    { id: "files", label: isMac() ? "Finder" : "File manager", editor: null },
  ];
}

/**
 * The directory the bar opens: the checkout on screen, never its project root.
 *
 * A worktree's whole point is that it is not the main clone, so opening the
 * project would hand the editor the wrong branch.
 */
export function openTargetPath(
  snapshot: Pick<ShellSnapshot, "sessions" | "workspaces">,
  activeSession: string | null,
  focusedWorkspace: string | null,
): string | null {
  const session = snapshot.sessions.find((item) => item.id === activeSession);
  const workspace = session?.workspace_id ?? focusedWorkspace;
  if (!workspace) return null;
  return snapshot.workspaces.find((item) => item.id === workspace)?.path ?? null;
}

/** Falls back to the first target, so the button always has something to run. */
export function preferredTarget(appState: Record<string, string>): OpenTarget {
  const targets = openTargets();
  const stored = appState[OPEN_WITH_KEY];
  return targets.find((target) => target.id === stored) ?? targets[0];
}
