import type { CenterMode } from "../store/viewsStore";
import { hasCode, type ParkedViews, type WorkbenchView } from "../workbench/views";

/** What `MOD-W` acts on. */
export type CloseTarget = { kind: "view"; view: WorkbenchView } | { kind: "session" };

/**
 * What "close" means right now.
 *
 * One chord, two weights of action, and the centre column is what decides
 * between them: with a file, diff or PR on screen it closes *that* — the thing
 * the person is looking at — and only on the terminal does it reach for the
 * session. Closing a session because someone wanted to put a file away is not
 * recoverable, so the terminal case is the fallback rather than the default.
 */
export function closeTarget(mode: CenterMode, views: ParkedViews): CloseTarget {
  if (mode !== "code" || !hasCode(views)) return { kind: "session" };
  // The terminal can be `active` while views are parked; it is not a tab in
  // the strip, so there is nothing there to close.
  if (views.active.kind === "terminal") return { kind: "session" };
  return { kind: "view", view: views.active };
}
