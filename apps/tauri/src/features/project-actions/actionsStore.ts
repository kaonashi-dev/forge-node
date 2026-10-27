import { createStore } from "solid-js/store";
import type { ActionSuggestion, RepoActionsFile } from "../../contracts/runtime";

export type ActionsDialogTarget = {
  projectId: string;
  /** The checkout whose files are read for suggestions. */
  workspaceId: string;
  name: string;
};

// Read results and the open dialog; saved actions live in `forgeStore.project_actions`.
export type ActionsState = {
  /** Kept here so the palette can open it and collapsing the sidebar cannot close it. */
  dialog: ActionsDialogTarget | null;
  /** Per workspace id. */
  suggestions: Record<string, ActionSuggestion[]>;
  /** `null` when the workspace has no `.forge/actions.toml`. */
  repoFile: Record<string, RepoActionsFile | null>;
  truncated: Record<string, boolean>;
  scanning: boolean;
  error: string | null;
};

const [actionsStore, setActionsStore] = createStore<ActionsState>({
  dialog: null,
  suggestions: {},
  repoFile: {},
  truncated: {},
  scanning: false,
  error: null,
});

export { actionsStore, setActionsStore };

export function applySuggestions(
  workspace: string,
  suggestions: ActionSuggestion[],
  repoFile: RepoActionsFile | null,
  truncated: boolean,
): void {
  setActionsStore("suggestions", workspace, suggestions);
  setActionsStore("repoFile", workspace, repoFile);
  setActionsStore("truncated", workspace, truncated);
  setActionsStore({ scanning: false, error: null });
}

export function setActionsError(error: string): void {
  setActionsStore({ scanning: false, error });
}

export function requestActionsDialog(target: ActionsDialogTarget): void {
  setActionsStore({ dialog: target });
}
