import { batch } from "solid-js";
import { reconcile } from "solid-js/store";
import { directories } from "../../features/files/directories/directoryState";
import { pathOperations } from "../../features/files/operations/operations";
import { resetFilesAnswers } from "../../features/files/state";
import { resetGitAnswers } from "../../features/git/state";
import { parentPath } from "../../shared/paths";
import { setLoading } from "../../state/loading";
import { onWorkspaceChange } from "../../state/workspace";
import { joinPanes } from "../../features/sessions/sessionActions";

/**
 * Clear every answer when the workbench moves to another checkout.
 *
 * The panels are about *this* workspace, and showing the previous one's diff
 * under a new branch name is worse than showing nothing.
 */
export function startWorkspaceFocus(): () => void {
  return onWorkspaceChange((workspace) => {
    directories.focus(workspace);
    pathOperations.reconcile(
      (operation) => {
        if (operation.workspace !== workspace) return;
        for (const path of [operation.from, operation.to]) {
          if (path !== undefined) directories.ensure(operation.workspace, parentPath(path), true);
        }
      },
      () => {
        if (workspace) directories.invalidate(workspace);
      },
    );
    batch(() => {
      resetFilesAnswers();
      resetGitAnswers();
      setLoading(reconcile({}));
    });
    joinPanes();
  });
}
