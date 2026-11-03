import { createStore } from "solid-js/store";
import type {
  Branches,
  JuvaDraft,
  RebaseState,
  BranchCompare,
  WorkspaceDiff,
} from "../../contracts/workbench";

export const [gitStore, setGitStore] = createStore({
  diff: null as WorkspaceDiff | null,
  diffError: null as string | null,
  compare: null as BranchCompare | null,
  compareError: null as string | null,
  rebase: null as RebaseState | null,
  rebaseError: null as string | null,
  /** The conflicted path the three-way view is showing. */
  conflictFocus: null as string | null,
  branches: null as Branches | null,
  juvaDraft: null as JuvaDraft | null,
  juvaError: null as string | null,
});

/** Branches are a project's, not a checkout's, so they survive a workspace move. */
export function resetGitAnswers(): void {
  setGitStore({
    diff: null,
    diffError: null,
    compare: null,
    compareError: null,
    rebase: null,
    rebaseError: null,
    conflictFocus: null,
    juvaDraft: null,
    juvaError: null,
  });
}
