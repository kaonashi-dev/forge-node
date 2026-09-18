import { directories } from "../directories/directoryState";
import { invalidateFileIndex } from "../state";
import { parentPath } from "../../../shared/paths";
import { pathOperations } from "./operations";

export function startFileMutationEffects(): () => void {
  const unsubscribeResult = pathOperations.subscribe((result) => {
    directories.operation(result, result.directory);
    invalidateFileIndex(result.workspace);
  });
  const unsubscribeUncertain = pathOperations.onUncertain((operation) => {
    for (const path of [operation.from, operation.to]) {
      if (path !== undefined) directories.ensure(operation.workspace, parentPath(path), true);
    }
    invalidateFileIndex(operation.workspace);
  });
  return () => {
    unsubscribeResult();
    unsubscribeUncertain();
  };
}
