import { createSignal } from "solid-js";
import { LAST_WORKSPACE_KEY, writeChoice } from "./preferences";

/** Which workspace the answers around it belong to, so a stale one is dropped. */
const [activeWorkspace, setActiveWorkspace] = createSignal<string | null>(null);

type WorkspaceListener = (workspace: string | null) => void;
const listeners = new Set<WorkspaceListener>();

export function onWorkspaceChange(listener: WorkspaceListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Point the workbench at a checkout.
 *
 * The one choke point every gesture that moves the window goes through, so it
 * is where the checkout is remembered for the next launch. A `null` focus does
 * not clear it: that is "no session on screen", which happens on every startup
 * before the snapshot lands, not a person choosing to be nowhere.
 */
export function focusWorkspace(workspace: string | null): void {
  if (activeWorkspace() === workspace) return;
  setActiveWorkspace(workspace);
  if (workspace) writeChoice(LAST_WORKSPACE_KEY, workspace);
  for (const listener of listeners) listener(workspace);
}

export { activeWorkspace, setActiveWorkspace };
