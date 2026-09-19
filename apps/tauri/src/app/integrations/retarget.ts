import { batch } from "solid-js";
import { retargetWorkspaceViews } from "../../navigation/viewsStore";
import { retargetRecentFiles } from "../../features/files/index/recentFiles";
import { retargetPath } from "../../shared/paths";
import { filesStore, setFilesStore } from "../../features/files/state";
import { activeWorkspace } from "../../state/workspace";

/** Only an explicit successful mutation may map unrelated path consumers. */
export function retargetWorkbenchPaths(workspace: string, from: string, to: string): void {
  batch(() => {
    retargetWorkspaceViews(workspace, from, to);
    retargetRecentFiles(workspace, from, to);
    if (activeWorkspace() !== workspace) return;
    const index = filesStore.tree;
    if (index) {
      setFilesStore("tree", {
        ...index,
        entries: index.entries.map((entry) => {
          const path = retargetPath(entry.path, from, to);
          return path === entry.path ? entry : { ...entry, path };
        }),
      });
    }
    const file = filesStore.file;
    if (file) {
      const path = retargetPath(file.path, from, to);
      if (path !== file.path) setFilesStore("file", { ...file, path });
    }
    // Search answers are observations of the old paths, never evidence of a move.
    setFilesStore("search", null);
  });
}
