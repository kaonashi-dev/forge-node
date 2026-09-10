// Who holds the caret when the window changes session.
//
// `CenterStack` hides the terminal pane rather than unmounting it — a switch
// must not cost a PTY resync — so the pane mounts once for the life of the
// window and nothing outside it can reach the hidden textarea through a ref.
// Two ways in: a gesture that names a terminal takes the caret, a switch the
// window made on its own asks first.

import { isTypingTarget } from "../actions/dispatch";
import type { CenterMode } from "../store/viewsStore";

let focuser: (() => void) | null = null;

/** The mounted pane offers its keyboard target; the returned call withdraws it. */
export function registerTerminalFocus(focus: () => void): () => void {
  focuser = focus;
  return () => {
    if (focuser === focus) focuser = null;
  };
}

/**
 * Put the caret in the grid. A no-op until a pane has registered one.
 *
 * Unconditional: re-selecting the session that is already active changes
 * nothing for [`mayTakeCaret`]'s effect to observe, and the gesture still
 * means "type here".
 */
export function focusTerminal(): void {
  focuser?.();
}

/**
 * Whether a session change the *window* made may take the caret.
 *
 * Not while the Code tab is up, where the keyboard belongs to the editor, and
 * never out of a field somebody is typing in: an agent exiting in the
 * background moves the active session, and that must not empty an open input
 * mid-sentence. Its own textarea is the exception — that is the target.
 */
export function mayTakeCaret(
  mode: CenterMode,
  session: string | null,
  active: EventTarget | null,
  keys: EventTarget | null,
): boolean {
  if (session === null || mode !== "session") return false;
  return active === keys || !isTypingTarget(active);
}
