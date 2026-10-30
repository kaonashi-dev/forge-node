import type { ProjectAction } from "../../contracts/runtime";
import { sendRuntimeCommand } from "../../runtime/host";
import { sendWorkbenchCommand } from "../../runtime/workbench";
import { setActionsStore } from "./actionsStore";

/** The answer arrives as `workbench:action_suggestions`. */
export async function suggestActions(workspace: string): Promise<void> {
  setActionsStore({ scanning: true, error: null });
  await sendWorkbenchCommand({ type: "suggest_actions", workspace });
}

/** The answer arrives as `workbench:action_launched`, which focuses the session. */
export async function runAction(workspace: string, action: string): Promise<void> {
  await sendWorkbenchCommand({ type: "run_action", workspace, action });
}

export async function setProjectActions(project: string, actions: ProjectAction[]): Promise<void> {
  await sendRuntimeCommand({ type: "set_project_actions", project, actions });
}
