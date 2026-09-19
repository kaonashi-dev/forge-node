import { pathOperations } from "../../features/files/operations/operations";
import { startFileMutationEffects } from "../../features/files/operations/effects";
import { retargetWorkbenchPaths } from "../integrations/retarget";

export { startDecorationEffects } from "../../features/git/decorations";

export function startPathMutationEffects(): () => void {
  const stopFileEffects = startFileMutationEffects();
  const unsubscribeRetarget = pathOperations.subscribe((result) => {
    if (result.kind === "rename" && result.from && result.to) {
      retargetWorkbenchPaths(result.workspace, result.from, result.to);
    }
  });
  return () => {
    stopFileEffects();
    unsubscribeRetarget();
  };
}
