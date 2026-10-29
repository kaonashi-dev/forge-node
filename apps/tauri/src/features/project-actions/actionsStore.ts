import { batch } from "solid-js";
import { createStore, produce } from "solid-js/store";
import type { ActionSuggestion, RepoActionsFile } from "../../contracts/runtime";

export type ActionsDialogTarget = {
  projectId: string;
  /** The checkout whose files are read for suggestions. */
  workspaceId: string;
  name: string;
};

export type ActionsSaveResult = {
  project: string;
  /** The daemon's refusal, phrased for the dialog; `null` once saved. */
  error: string | null;
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
  actionsSave: ActionsSaveResult | null;
};

const [actionsStore, setActionsStore] = createStore<ActionsState>({
  dialog: null,
  suggestions: {},
  repoFile: {},
  truncated: {},
  scanning: false,
  error: null,
  actionsSave: null,
});

export { actionsStore, setActionsStore };

export function applySuggestions(
  workspace: string,
  suggestions: ActionSuggestion[],
  repoFile: RepoActionsFile | null,
  truncated: boolean,
): void {
  // One update: the dialog drafts when `suggestions` lands and reads `repoFile` then.
  batch(() => {
    setActionsStore("suggestions", workspace, suggestions);
    setActionsStore("repoFile", workspace, repoFile);
    setActionsStore("truncated", workspace, truncated);
    setActionsStore({ scanning: false, error: null });
  });
}

/** Drop a workspace's last answer so a dialog drafts from the next one, not from it. */
export function forgetSuggestions(workspace: string): void {
  setActionsStore(
    produce((state) => {
      delete state.suggestions[workspace];
      delete state.repoFile[workspace];
      delete state.truncated[workspace];
    }),
  );
}

export function setActionsError(error: string): void {
  setActionsStore({ scanning: false, error });
}

export function requestActionsDialog(target: ActionsDialogTarget): void {
  setActionsStore({ dialog: target });
}
