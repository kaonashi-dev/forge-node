import type { ProjectAction } from "../../contracts/runtime";
import { openUrl } from "../../runtime/host";
import type { MenuItem, MenuItemButton } from "../../ui/types";
import { killSession, restartSession } from "../sessions/commands";
import { focusSession } from "../sessions/sessionActions";
import {
  browserUrl,
  finishedRun,
  isActive,
  latestRun,
  sessionPhase,
  urlBadge,
  type ActionSessions,
  type LiveChipState,
} from "./actionState";
import { requestActionsDialog } from "./actionsStore";
import { runAction } from "./commands";

/** The status beside a live action's name, and the colour its mark takes. */
function liveStatus(state: LiveChipState): { detail: string; iconClass: string } {
  switch (state.phase) {
    case "ready":
      return { detail: urlBadge(state.url, null) ?? "ready", iconClass: "forge-icon-green" };
    case "waiting":
      return { detail: "waiting", iconClass: "forge-icon-muted" };
    case "probing":
      return { detail: "starting", iconClass: "forge-icon-muted" };
    case "timed_out":
      return { detail: "not ready", iconClass: "forge-icon-amber" };
    case "port_in_use":
      return { detail: `:${state.port} taken`, iconClass: "forge-icon-amber" };
    default:
      return { detail: "running", iconClass: "forge-icon-green" };
  }
}

/**
 * What a checkout's ▶ offers: one row per project action — an idle one runs
 * (in its finished session, if it left one), a live one shows its terminal and
 * carries its own browser, restart and stop, so several running at once stay
 * one row each — then the actions dialog.
 */
export function actionMenuItems(
  workspaceId: string,
  project: { id: string; name: string },
  actions: readonly ProjectAction[],
  index: ActionSessions,
): MenuItem[] {
  const items: MenuItem[] = [];
  for (const action of [...actions].sort((a, b) => a.position - b.position)) {
    const session = latestRun(index, workspaceId, action.id);
    const state = session ? sessionPhase(session) : ({ phase: "idle" } as const);
    if (!session || !isActive(state)) {
      const finished = finishedRun(state);
      items.push({
        kind: "item",
        label: action.label,
        icon: "play",
        run: () =>
          void (finished ? restartSession(finished) : runAction(workspaceId, action.id)).catch(
            () => undefined,
          ),
      });
      continue;
    }
    const sessionId = state.sessionId;
    const buttons: MenuItemButton[] = [];
    const url = browserUrl(session);
    if (url) {
      buttons.push({
        label: `Open ${url} in the browser`,
        icon: "globe",
        run: () => void openUrl(url).catch(() => undefined),
      });
    }
    buttons.push({
      label: `Restart ${action.label}`,
      icon: "refresh",
      run: () => {
        focusSession(sessionId);
        void restartSession(sessionId).catch(() => undefined);
      },
    });
    buttons.push({
      label: `Stop ${action.label}`,
      icon: "stop",
      run: () => void killSession(sessionId).catch(() => undefined),
    });
    items.push({
      kind: "item",
      label: action.label,
      icon: "activity",
      ...liveStatus(state),
      buttons,
      run: () => focusSession(sessionId),
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
