import { workbenchStore } from "../store/workbenchStore";
import { freshDirectoryTree } from "./directoryState";
import type { FileEntry, FileTree } from "./types";
import { combineFileIndex } from "./fileIndexData";

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
  const global = workbenchStore.tree;
  const local = freshDirectoryTree();
  const globalEntries = global?.entries;
  const localEntries = local?.entries;
  const workspace = workbenchStore.workspace;
  const truncated =
    !global || global.truncated || workbenchStore.treeStale || (local?.truncated ?? false);
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
