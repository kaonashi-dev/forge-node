import type { ActionKind, ProjectAction, Session } from "../../contracts/runtime";

/** What a chip shows for one (workspace, action). */
export type ChipState =
  | { phase: "idle" }
  | { phase: "waiting"; sessionId: string }
  | { phase: "probing"; sessionId: string }
  | { phase: "running"; sessionId: string }
  | { phase: "ready"; sessionId: string; url: string | null }
  | { phase: "timed_out"; sessionId: string }
  | { phase: "port_in_use"; sessionId: string; port: number }
  | { phase: "exited"; sessionId: string; code: number | null }
  | { phase: "failed"; sessionId: string; reason: string };

export type ActionSessions = ReadonlyMap<string, readonly Session[]>;

const key = (workspaceId: string, actionId: string): string => `${workspaceId}\u0000${actionId}`;

export function isLive(session: Session): boolean {
  return session.state === "Starting" || session.state === "Running";
}

/**
 * Task sessions by (workspace, action), newest first. Built when the sessions
 * store changes, so a chip only looks its key up.
 */
export function indexActionSessions(sessions: readonly Session[]): ActionSessions {
  const out = new Map<string, Session[]>();
  for (const session of sessions) {
    const run = session.action;
    if (!run) continue;
    const k = key(session.workspace_id, run.action_id);
    const list = out.get(k);
    if (list) list.push(session);
    else out.set(k, [session]);
  }
  for (const list of out.values()) {
    list.sort((a, b) => b.created_at.localeCompare(a.created_at));
  }
  return out;
}

/** The live session if there is one, else the most recent run still in the store. */
export function latestRun(
  index: ActionSessions,
  workspaceId: string,
  actionId: string,
): Session | null {
  const runs = index.get(key(workspaceId, actionId));
  if (!runs || runs.length === 0) return null;
  return runs.find(isLive) ?? runs[0]!;
}

export function chipState(index: ActionSessions, workspaceId: string, actionId: string): ChipState {
  const session = latestRun(index, workspaceId, actionId);
  return session ? sessionPhase(session) : { phase: "idle" };
}

/**
 * Where a running task serves, for "open in the browser": the URL it printed
 * once ready, else its port on localhost. `null` before it is up, once it has
 * stopped, or while its port belongs to another process.
 */
export function browserUrl(session: Session): string | null {
  const state = sessionPhase(session);
  if (state.phase === "ready" && state.url) return state.url;
  const port = session.action?.port ?? null;
  const up = state.phase === "ready" || state.phase === "running" || state.phase === "timed_out";
  return up && port !== null ? `http://localhost:${port}` : null;
}

export function sessionPhase(session: Session): ChipState {
  const sessionId = session.id;
  const state = session.state;
  if (typeof state === "object" && "Exited" in state && state.Exited) {
    return { phase: "exited", sessionId, code: state.Exited.code };
  }
  if (typeof state === "object" && "Failed" in state && state.Failed) {
    return { phase: "failed", sessionId, reason: state.Failed.reason };
  }
  if (!isLive(session)) return { phase: "exited", sessionId, code: null };
  const readiness = session.action?.readiness;
  switch (readiness?.state) {
    case "waiting":
      return { phase: "waiting", sessionId };
    case "probing":
      return { phase: "probing", sessionId };
    case "ready":
      return { phase: "ready", sessionId, url: readiness.url };
    case "timed_out":
      return { phase: "timed_out", sessionId };
    case "port_in_use":
      return { phase: "port_in_use", sessionId, port: readiness.port };
    default:
      return { phase: "running", sessionId };
  }
}

/** Whether a click focuses the session rather than starting a run. */
export type LiveChipState = Exclude<ChipState, { phase: "idle" | "exited" | "failed" }>;

export function isActive(state: ChipState): state is LiveChipState {
  return (
    state.phase === "waiting" ||
    state.phase === "probing" ||
    state.phase === "running" ||
    state.phase === "ready" ||
    state.phase === "timed_out" ||
    state.phase === "port_in_use"
  );
}

/** The finished run to start again in place, so a rerun does not leave another row behind. */
export function finishedRun(state: ChipState): string | null {
  return state.phase === "exited" || state.phase === "failed" ? state.sessionId : null;
}

/** Pinned first, then the rest, each in saved order. */
export function chipOrder(actions: readonly ProjectAction[]): {
  pinned: ProjectAction[];
  rest: ProjectAction[];
} {
  const sorted = [...actions].sort((a, b) => a.position - b.position);
  return {
    pinned: sorted.filter((action) => action.pinned),
    rest: sorted.filter((action) => !action.pinned),
  };
}

export const KIND_LABELS: Record<ActionKind, string> = {
  dev: "Dev",
  test: "Test",
  lint: "Lint",
  build: "Build",
  custom: "Custom",
};

/** The address a ready chip links to, shortened for its badge: `:5173`. */
export function urlBadge(url: string | null, port: number | null): string | null {
  if (url) {
    try {
      const parsed = new URL(url);
      return parsed.port ? `:${parsed.port}` : parsed.host;
    } catch {
      return null;
    }
  }
  return port === null ? null : `:${port}`;
}
