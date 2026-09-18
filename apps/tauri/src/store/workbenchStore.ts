import { pathOperations } from "../workbench/operations";
import { parentPath } from "../workbench/pathOperations";
import { directories } from "../workbench/directoryState";
import { createStore } from "solid-js/store";
import { LAST_WORKSPACE_KEY, writeChoice } from "../shell/layout";
import type {
  Branches,
  FileContents,
  FileTree,
  RebaseState,
  SearchResults,
  UsageAnalytics,
  WorkspaceDiff,
  WorkspaceReview,
} from "../workbench/types";

/**
 * Answers to questions a panel asked.
 *
 * Keyed by nothing: one workspace is on screen at a time, and holding a cache
 * per checkout would mean deciding when it goes stale — which for a `git diff`
 * is "whenever anything wrote a file", a question this side cannot answer. A
 * panel that comes back asks again.
 *
 * `error` is per surface rather than global: a failed diff must not blank the
 * file tree beside it.
 */
export const [workbenchStore, setWorkbenchStore] = createStore({
  /** Which workspace the answers below belong to, so a stale one is dropped. */
  workspace: null as string | null,
  diff: null as WorkspaceDiff | null,
  diffError: null as string | null,
  /** The review tab's answer, and when it was generated. Regenerating
   *  replaces it in place rather than opening a second tab. */
  review: null as WorkspaceReview | null,
  reviewError: null as string | null,
  reviewAt: null as number | null,
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
  rebase: null as RebaseState | null,
  rebaseError: null as string | null,
  branches: null as Branches | null,
  usage: null as UsageAnalytics | null,
  usageError: null as string | null,
  juvaDraft: null as import("../workbench/types").JuvaDraft | null,
  juvaError: null as string | null,
  /** Reads in flight, by surface, so a panel can say it is working. */
  loading: {} as Record<string, boolean>,
});

export function setLoading(surface: string, value: boolean): void {
  setWorkbenchStore("loading", surface, value);
}

/**
 * Point the workbench at a checkout.
 *
 * Clears every answer: the panels are about *this* workspace, and showing the
 * previous one's diff under a new branch name is worse than showing nothing.
 *
 * Also the one choke point every gesture that moves the window goes through,
 * so it is where the checkout is remembered for the next launch. A `null`
 * focus does not clear it: that is "no session on screen", which happens on
 * every startup before the snapshot lands, not a person choosing to be
 * nowhere.
 */
export function focusWorkspace(workspace: string | null): void {
  if (workbenchStore.workspace === workspace) return;
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
  if (workspace) writeChoice(LAST_WORKSPACE_KEY, workspace);
  setWorkbenchStore({
    workspace,
    diff: null,
    diffError: null,
    review: null,
    reviewError: null,
    reviewAt: null,
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
    rebase: null,
    rebaseError: null,
    loading: {},
  });
}

/** The next global-index consumer refreshes it; explorer updates stay directory-local. */
export function invalidateFileIndex(workspace: string): void {
  if (workspace !== workbenchStore.workspace) return;
  setWorkbenchStore({
    treeStale: true,
    treeError: null,
    searchStale: true,
    treeVersion: workbenchStore.treeVersion + 1,
  });
}
