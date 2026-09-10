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
  file: null as FileContents | null,
  fileError: null as string | null,
  search: null as SearchResults | null,
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
    file: null,
    fileError: null,
    search: null,
    rebase: null,
    rebaseError: null,
    loading: {},
  });
}
