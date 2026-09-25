import type { CenterMode } from "../../../navigation/viewsStore";
import type { ParkedViews, WorkbenchView } from "../../../navigation/views";
import { visibleSplit, type CenterSplitState } from "../../../navigation/centerSplit";

/** What `MOD-W` acts on. */
export type CloseTarget =
  | { kind: "view"; view: WorkbenchView }
  | { kind: "session" }
  | { kind: "unsplit" }
  | { kind: "none" };

/**
 * What "close" means right now.
 *
 * One chord, two weights of action, and the centre column is what decides
 * between them: with a file, diff or PR on screen it closes *that* — the thing
 * the person is looking at — and only on the terminal does it reach for the
 * session. Closing a session because someone wanted to put a file away is not
 * recoverable, so the terminal case is the fallback rather than the default.
 */
export function closeTarget(
  mode: CenterMode,
  views: ParkedViews,
  split?: CenterSplitState,
  settings = false,
): CloseTarget {
  // A remembered split hidden behind a diff or Settings is not what the chord
  // points at; joining it would drop the split and leave the diff open.
  if (
    split &&
    split.focused === "extra" &&
    visibleSplit(split, settings, mode, views.active) !== "closed"
  ) {
    return { kind: "unsplit" };
  }
  if (mode !== "code") return { kind: "session" };
  // The terminal can be `active` while views are parked; it is not a tab in
  // the strip, so there is nothing there to close.
  if (views.active.kind === "terminal") return { kind: "none" };
  return { kind: "view", view: views.active };
}
