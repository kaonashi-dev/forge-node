import { createEffect, createMemo, createRoot, onCleanup } from "solid-js";
import { currentViews } from "../store/viewsStore";
import { workbenchStore } from "../store/workbenchStore";
import { watchFiles } from "./fileWatch";
import { parentPath } from "./pathOperations";

/** Open documents keep their parents watched even with the explorer closed. */
export function startDocumentWatch(): () => void {
  return createRoot((dispose) => {
    const interests = createMemo(() => {
      const paths = currentViews().open.flatMap((view) =>
        view.kind === "editor-terminal" || view.kind === "preview" ? [parentPath(view.path)] : [],
      );
      return JSON.stringify([...new Set(paths)].sort());
    });
    createEffect(() => {
      const workspace = workbenchStore.workspace;
      const paths: string[] = JSON.parse(interests());
      if (workspace && paths.length) onCleanup(watchFiles(workspace, paths, () => undefined));
    });
    return dispose;
  });
}
