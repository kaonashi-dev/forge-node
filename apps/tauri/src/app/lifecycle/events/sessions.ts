import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AgentLaunchResult, HandoffProgress } from "../../../contracts/workbench";
import {
  applyHandoffLaunch,
  applyHandoffProgress,
} from "../../../features/sessions/handoffJobStore";

export function bindSessionEvents(): Promise<UnlistenFn[]> {
  return Promise.all([
    listen<AgentLaunchResult>("workbench:agent_launched", ({ payload }) =>
      applyHandoffLaunch(payload),
    ),
    listen<HandoffProgress>("workbench:handoff_progress", ({ payload }) =>
      applyHandoffProgress(payload),
    ),
  ]);
}
