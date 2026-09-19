import { createStore } from "solid-js/store";
import { activeWorkspace } from "../../state/workspace";
import type { FileContents, FileTree, SearchResults } from "../../contracts/workbench";

export const [filesStore, setFilesStore] = createStore({
  tree: null as FileTree | null,
  treeError: null as string | null,
  treeStale: false,
  treeVersion: 0,
  treeRequest: null as { request_id: string; version: number } | null,
  file: null as FileContents | null,
  fileError: null as string | null,
  search: null as SearchResults | null,
  searchStale: false,
  searchReadVersion: 0,
  /* Its own field and not `fileError`: a failed `git grep` must not read as a
     failed `ReadFile`, which is what the editor's re-read is guarded by. */
  searchError: null as string | null,
  /** Palette file ranking. Separate from `search` so Cmd+P cannot blank find-in-files. */
  nameSearch: null as SearchResults | null,
  nameSearchError: null as string | null,
});

export function resetFilesAnswers(): void {
  setFilesStore({
    tree: null,
    treeError: null,
    treeStale: false,
    treeRequest: null,
    file: null,
    fileError: null,
    search: null,
    searchStale: false,
    searchError: null,
    nameSearch: null,
    nameSearchError: null,
  });
}

/** The next global-index consumer refreshes it; explorer updates stay directory-local. */
export function invalidateFileIndex(workspace: string): void {
  if (workspace !== activeWorkspace()) return;
  setFilesStore({
    treeStale: true,
    treeError: null,
    searchStale: true,
    treeVersion: filesStore.treeVersion + 1,
  });
}
