import type { SessionWork } from "../../runtime/work";

export type SessionGlyphFace = "identity" | "working" | "done";

export type SessionGlyphContext = {
  active: boolean;
  isAgent: boolean;
  /** Output arrived while this session was unfocused. */
  unread: boolean;
};

/**
 * Which mark occupies the glyph slot of a session row.
 *
 * Identity is the default — including the session the user is in. `sessionWork`
 * treats any output inside a minute as "working", and attaching a session
 * bumps that clock, so a Codex sitting at a prompt would spin the whole time
 * it was being read. A spinner is only for work the user is not looking at:
 * still starting, or producing unseen output. A check is a finished agent
 * elsewhere. Shells never finish a task, so an exited terminal stays the
 * terminal mark.
 */
export function sessionGlyphFace(
  work: SessionWork | undefined,
  ctx: SessionGlyphContext,
): SessionGlyphFace {
  if (ctx.active) return "identity";
  if (work === "starting" || (work === "working" && ctx.unread)) return "working";
  if (work === "idle") return "done";
  if (ctx.isAgent && work === "exited") return "done";
  return "identity";
}
