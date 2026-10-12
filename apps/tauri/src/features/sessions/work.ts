import type { Session, SessionState } from "../../contracts/runtime";
import { sessionIsActive } from "../../contracts/runtime";

export type SessionWork =
  | "starting"
  | "working"
  | "idle"
  | "running"
  | "needs-you"
  | "exited"
  | "failed";

// Legacy snapshots lack activity; allow two 30 s broadcasts before showing them idle.
export const WORKING_WINDOW_MS = 60_000;

export function sessionWork(
  session: Session,
  attention: { wants_you: boolean },
  now: number,
): SessionWork {
  const state: SessionState = session.state;
  if (state === "Starting") return "starting";
  if (state === "Orphaned" || (typeof state === "object" && "Failed" in state)) return "failed";
  if (!sessionIsActive(state)) return "exited";
  if (attention.wants_you) return "needs-you";
  if (session.kind === "Editor") return "idle";
  if (session.agent_provider_id == null) return "running";

  switch (session.activity?.state) {
    case "working":
      return "working";
    case "idle":
      return "idle";
    case "waiting":
      return "running";
    case "starting":
      return "starting";
    case "unknown":
    case "unrecognized":
      return "running";
  }

  const stamp = session.last_activity_at ?? session.created_at;
  const then = Date.parse(stamp);
  // An unparseable clock is not evidence of idle.
  if (Number.isNaN(then)) return "working";
  return now - then >= WORKING_WINDOW_MS ? "idle" : "working";
}
