import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";

/** `[workspace, payload]` — the worker tags every answer with what it is about. */
export type Tagged<T> = [string, T];

export type Failure = { workspace: string | null; error: string };

/**
 * Workbench answers.
 *
 * Every one is checked against the workspace the panels are looking at: a read
 * started before a session switch lands after it, and painting the previous
 * checkout's diff under the new branch name is worse than painting nothing.
 */
export function forCurrent(workspace: string | null): boolean {
  return workspace !== null && workspace === activeWorkspace();
}

export function answer<T>(
  event: string,
  surface: string,
  apply: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<Tagged<T>>(event, ({ payload: [workspace, value] }) => {
    if (!forCurrent(workspace)) return;
    apply(value);
    setLoading(surface, false);
  });
}

export function failure(
  event: string,
  surface: string,
  apply: (error: string) => void,
): Promise<UnlistenFn> {
  return listen<Failure>(event, ({ payload }) => {
    if (payload.workspace === null || forCurrent(payload.workspace)) {
      apply(payload.error);
      setLoading(surface, false);
    }
  });
}
