import { createStore } from "solid-js/store";
import type {
  Branches,
  JuvaDraft,
  RebaseState,
  WorkspaceDiff,
  WorkspaceReview,
} from "../../contracts/workbench";

export const [gitStore, setGitStore] = createStore({
  diff: null as WorkspaceDiff | null,
  diffError: null as string | null,
  /** The review tab's answer, and when it was generated. Regenerating
   *  replaces it in place rather than opening a second tab. */
  review: null as WorkspaceReview | null,
  reviewError: null as string | null,
  reviewAt: null as number | null,
  rebase: null as RebaseState | null,
  rebaseError: null as string | null,
  branches: null as Branches | null,
  juvaDraft: null as JuvaDraft | null,
  juvaError: null as string | null,
});

/** Branches are a project's, not a checkout's, so they survive a workspace move. */
export function resetGitAnswers(): void {
  setGitStore({
    diff: null,
    diffError: null,
    review: null,
    reviewError: null,
    reviewAt: null,
    rebase: null,
    rebaseError: null,
    juvaDraft: null,
    juvaError: null,
  });
}
