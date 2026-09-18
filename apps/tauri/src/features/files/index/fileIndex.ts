import { freshDirectoryTree } from "../directories/directoryState";
import type { FileEntry, FileTree } from "../../../contracts/workbench";
import { combineFileIndex } from "./fileIndexData";
import { filesStore } from "../state";
import { activeWorkspace } from "../../../state/workspace";

let cached:
  | {
      globalEntries: FileEntry[] | undefined;
      localEntries: FileEntry[] | undefined;
      workspace: string | null;
      truncated: boolean;
      value: FileTree | null;
      files: string[] | null;
    }
  | undefined;

function filePathsOf(tree: FileTree | null): string[] | null {
  if (!tree) return null;
  const files: string[] = [];
  for (const entry of tree.entries) {
    if (entry.kind === "File") files.push(entry.path);
  }
  return files;
}

export function navigationFileIndex(): FileTree | null {
  const global = filesStore.tree;
  const local = freshDirectoryTree();
  const globalEntries = global?.entries;
  const localEntries = local?.entries;
  const workspace = activeWorkspace();
  const truncated =
    !global || global.truncated || filesStore.treeStale || (local?.truncated ?? false);
  if (
    !cached ||
    cached.globalEntries !== globalEntries ||
    cached.localEntries !== localEntries ||
    cached.workspace !== workspace ||
    cached.truncated !== truncated
  ) {
    const value = combineFileIndex(
      global?.workspace_id === workspace ? { ...global, truncated } : null,
      local?.workspace_id === workspace ? local : null,
    );
    cached = {
      globalEntries,
      localEntries,
      workspace,
      truncated,
      value,
      files: filePathsOf(value),
    };
  }
  return cached.value;
}

export function navigationFilePaths(): string[] | null {
  navigationFileIndex();
  return cached?.files ?? null;
}
