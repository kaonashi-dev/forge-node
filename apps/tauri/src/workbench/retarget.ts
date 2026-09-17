import { batch } from "solid-js";
import { retargetWorkspaceViews } from "../store/viewsStore";
import { setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import { retargetRecentFiles } from "./recentFiles";
import { retargetPath } from "./pathOperations";

/** Only an explicit successful mutation may map unrelated path consumers. */
export function retargetWorkbenchPaths(workspace: string, from: string, to: string): void {
  batch(() => {
    retargetWorkspaceViews(workspace, from, to);
    retargetRecentFiles(workspace, from, to);
    if (workbenchStore.workspace !== workspace) return;
    const index = workbenchStore.tree;
    if (index) {
      setWorkbenchStore("tree", {
        ...index,
        entries: index.entries.map((entry) => {
          const path = retargetPath(entry.path, from, to);
          return path === entry.path ? entry : { ...entry, path };
        }),
      });
    }
    const file = workbenchStore.file;
    if (file) {
      const path = retargetPath(file.path, from, to);
      if (path !== file.path) setWorkbenchStore("file", { ...file, path });
    }
    // Search answers are observations of the old paths, never evidence of a move.
    setWorkbenchStore("search", null);
  });
}
