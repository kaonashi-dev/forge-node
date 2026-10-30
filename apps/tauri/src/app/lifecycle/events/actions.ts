import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { toast } from "../../../ui/index";
import type { ActionSuggestion, RepoActionsFile } from "../../../contracts/runtime";
import {
  applySuggestions,
  setActionsError,
  setActionsStore,
  type ActionsSaveResult,
} from "../../../features/project-actions/actionsStore";
import { focusSession } from "../../../features/sessions/sessionActions";
import { forgeStore } from "../../../state/forgeStore";
import type { Failure } from "../workbenchAnswer";

type Launched = {
  workspace: string;
  action: string;
  session: string;
  outcome: "started" | "queued" | "already_running";
};

export function bindActionsEvents(): Promise<UnlistenFn[]> {
  return Promise.all([
    listen<{
      workspace: string;
      suggestions: ActionSuggestion[];
      repo_file: RepoActionsFile | null;
      truncated: boolean;
    }>("workbench:action_suggestions", ({ payload }) =>
      applySuggestions(
        payload.workspace,
        payload.suggestions,
        payload.repo_file,
        payload.truncated,
      ),
    ),
    listen<Failure>("workbench:action_suggestions_failed", ({ payload }) =>
      setActionsError(payload.error),
    ),
    listen<Launched>("workbench:action_launched", ({ payload }) => {
      // A queued task has no terminal yet; its chip says what it waits on.
      if (payload.outcome === "queued") {
        const label = forgeStore.project_actions.find((a) => a.id === payload.action)?.label;
        toast({ title: `${label ?? "Action"} is waiting for its dependencies`, tone: "info" });
        return;
      }
      focusSession(payload.session);
    }),
    listen<ActionsSaveResult>("runtime:actions_save", ({ payload }) =>
      setActionsStore("actionsSave", payload),
    ),
    listen<Failure>("workbench:action_failed", ({ payload }) =>
      toast({ title: "The action did not start", detail: payload.error, tone: "danger" }),
    ),
  ]);
}
