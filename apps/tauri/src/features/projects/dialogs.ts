import { createStore } from "solid-js/store";

export type WorktreeRemoval = {
  workspace: string;
  /** `null` is the initial confirmation; a reason asks permission to force. */
  reason: string | null;
};

export type WorktreeCreationFailure = {
  project: string;
  branch: string;
  reason: string;
};

export const [projectDialogsStore, setProjectDialogsStore] = createStore({
  worktreeRemoval: null as WorktreeRemoval | null,
  worktreeCreationFailure: null as WorktreeCreationFailure | null,
  /** Context-menu target, independent of the active session and sidebar lifetime. */
  branchPicker: null as { id: string; name: string } | null,
  /** Kept here so collapsing the sidebar cannot dismiss the removal dialog. */
  projectRemoval: null as string | null,
});

/** Open the new-worktree dialog for the project the caller named. */
export function requestNewWorktree(project: { id: string; name: string }): void {
  setProjectDialogsStore("branchPicker", project);
}
