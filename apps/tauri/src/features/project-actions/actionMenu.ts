import type { ProjectAction } from "../../contracts/runtime";
import { openUrl } from "../../runtime/host";
import type { MenuItem } from "../../ui/types";
import { killSession } from "../sessions/commands";
import { focusSession } from "../sessions/sessionActions";
import { chipState, isActive, type ActionSessions } from "./actionState";
import { requestActionsDialog } from "./actionsStore";
import { runAction } from "./commands";

/**
 * What a checkout's ▶ offers: each of its project's actions — run it, or show,
 * open and stop the one already live there — then the project's actions dialog.
 */
export function actionMenuItems(
  workspaceId: string,
  project: { id: string; name: string },
  actions: readonly ProjectAction[],
  index: ActionSessions,
): MenuItem[] {
  const items: MenuItem[] = [];
  for (const action of [...actions].sort((a, b) => a.position - b.position)) {
    const state = chipState(index, workspaceId, action.id);
    if (!isActive(state)) {
      items.push({
        kind: "item",
        label: `Run ${action.label}`,
        icon: "play",
        run: () => void runAction(workspaceId, action.id).catch(() => undefined),
      });
      continue;
    }
    items.push({
      kind: "item",
      label: `Show ${action.label}`,
      icon: "activity",
      run: () => focusSession(state.sessionId),
    });
    if (state.phase === "ready" && state.url) {
      const url = state.url;
      items.push({
        kind: "item",
        label: `Open ${url}`,
        icon: "external-link",
        run: () => void openUrl(url).catch(() => undefined),
      });
    }
    items.push({
      kind: "item",
      label: `Stop ${action.label}`,
      icon: "stop",
      run: () => void killSession(state.sessionId).catch(() => undefined),
    });
  }
  if (items.length > 0) items.push({ kind: "rule" });
  items.push({
    kind: "item",
    label: actions.length > 0 ? "Edit actions…" : "Set up actions…",
    icon: "edit",
    run: () => requestActionsDialog({ projectId: project.id, workspaceId, name: project.name }),
  });
  return items;
}
