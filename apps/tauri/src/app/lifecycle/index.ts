import type { UnlistenFn } from "@tauri-apps/api/event";
import { connect } from "../../runtime/host";
import { startDocumentWatch } from "../../features/files/watches/documentWatch";
import { startWorkspaceFocus } from "../integrations/workspaceFocus";
import { bindTerminalEvents } from "./events/terminal";
import { bindEditorEvents } from "./events/editor";
import { bindFilesEvents } from "./events/files";
import { bindGitEvents } from "./events/git";
import { bindProjectsEvents } from "./events/projects";
import { bindHarnessEvents } from "./events/harness";
import { bindSettingsEvents } from "./events/settings";
import { bindAll } from "./bind";
import { applyConnected } from "./connection";
import { startDecorationEffects, startPathMutationEffects } from "./effects";
import { bindRuntimeEvents } from "./runtime";

export type StartAppRuntimeOptions = {
  onConnected?: () => void;
};

export async function startAppRuntime(options: StartAppRuntimeOptions = {}): Promise<() => void> {
  const stops: (() => void)[] = [];
  let unlisteners: UnlistenFn[] = [];
  try {
    unlisteners = await bindAll([
      bindRuntimeEvents,
      bindTerminalEvents,
      bindEditorEvents,
      bindFilesEvents,
      bindGitEvents,
      bindProjectsEvents,
      bindHarnessEvents,
      bindSettingsEvents,
    ]);
    for (const start of [
      startDocumentWatch,
      startWorkspaceFocus,
      startPathMutationEffects,
      startDecorationEffects,
    ]) {
      stops.push(start());
    }
    const snapshot = await connect();
    if (snapshot) {
      applyConnected(snapshot);
      options.onConnected?.();
    }
  } catch (error) {
    for (const stop of stops) stop();
    for (const unlisten of unlisteners) unlisten();
    throw error;
  }
  return () => {
    for (const stop of stops) stop();
    for (const unlisten of unlisteners) unlisten();
  };
}
