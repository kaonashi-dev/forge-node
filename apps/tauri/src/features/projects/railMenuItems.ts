import { requestNewWorktree } from "./dialogs";
import type { MenuItem } from "../../ui/index";

/**
 * Named on the row the menu opened from, not the active session: ⌘⇧N uses
 * that session's project, and a click on a different checkout would otherwise
 * create the worktree in the wrong repository.
 */
export function newWorktreeItem(project: { id: string; name: string } | undefined): MenuItem {
  return {
    kind: "item",
    label: "New worktree…",
    icon: "git-branch",
    disabled: !project,
    run: () => {
      if (!project) return;
      requestNewWorktree(project);
    },
  };
}
