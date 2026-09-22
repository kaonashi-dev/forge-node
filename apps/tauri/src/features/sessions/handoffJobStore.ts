import { showSession } from "../../navigation/viewsStore";
import { focusWorkspace } from "../../state/workspace";
import { forgeStore } from "../../state/forgeStore";
import {
  closeSession,
  launchBackgroundAgent,
  loadHandoffProgress,
  selectSession,
} from "./commands";
import { createHandoffJobStore } from "./handoffJob";

export { MAX_HANDOFF_SUMMARY_LENGTH, type HandoffJob } from "./handoffJob";

export const {
  handoffJob,
  beginHandoffJob,
  setHandoffProgressOpen,
  setHandoffSummary,
  applyHandoffLaunch,
  openHandoffSession,
  completeHandoff,
  syncHandoffSession,
  applyHandoffProgress,
  refreshHandoffProgress,
  retryHandoffProgress,
  finishHandoffWithSummary,
  cancelHandoffJob,
} = createHandoffJobStore({
  closeSession,
  launchBackgroundAgent,
  loadHandoffProgress,
  selectSession,
  showSession,
  focusWorkspace,
  sessionReady: (id) =>
    forgeStore.sessions.some((session) => session.id === id && session.terminal_id !== null),
});
