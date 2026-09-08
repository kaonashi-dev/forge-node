import type { Session, SessionState } from "./types";
import { sessionIsActive } from "./types";

/**
 * What a session is doing, as opposed to what its process is.
 *
 * `SessionState` answers "is there a program on the other end of this PTY",
 * which is not the question anyone reading the rail is asking. Every agent
 * that has not crashed is `Running`, so a column of green dots said only that
 * four agents existed — not which one is still thinking, which one is done and
 * which one is waiting to be told to go on.
 */
export type SessionWork =
  | "starting"
  /** An agent producing output right now. */
  | "working"
  /** An agent that has stopped producing and is waiting to be told what next. */
  | "idle"
  /** A shell with a live process, which is neither working nor finished. */
  | "running"
  | "needs-you"
  | "exited"
  | "failed";

/**
 * How long after its last output an agent counts as finished rather than busy.
 *
 * Sixty seconds because the daemon coalesces activity bumps and broadcasts one
 * at most every `terminal::ACTIVITY_BROADCAST` (30 s): a client watching a
 * genuinely busy agent still sees its clock move only twice a minute, so a
 * window any tighter than two broadcast periods would blink between "working"
 * and "done" while nothing changed.
 */
export const WORKING_WINDOW_MS = 60_000;

/**
 * The state a marker paints, in the order the answers outrank each other.
 *
 * A shell is neither working nor finished: nothing was asked of it, so there is
 * nothing for it to have finished, and it sits at a prompt for most of its
 * life. It gets `running` — the meaning green already had before agents needed
 * two states — so a terminal's dot is the colour it has always been.
 */
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
  if (session.agent_provider_id == null) return "running";

  const stamp = session.last_activity_at ?? session.created_at;
  const then = Date.parse(stamp);
  // An unparseable clock is not evidence of being finished.
  if (Number.isNaN(then)) return "working";
  return now - then >= WORKING_WINDOW_MS ? "idle" : "working";
}
