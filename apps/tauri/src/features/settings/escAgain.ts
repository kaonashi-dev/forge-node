/**
 * Two Escapes leave Settings.
 *
 * A sequence, not a chord: the capture-phase keymap would swallow Escape from
 * a dialog, a select, or the keyboard-capture row, so this cannot be a binding.
 */

/** How long the second Escape may follow the first. */
export const ESC_AGAIN_MS = 1_200;

/** Whether this Escape is the second of a pair still inside the window. */
export function isSecondEsc(now: number, firstAt: number | null): boolean {
  return firstAt !== null && now - firstAt <= ESC_AGAIN_MS;
}
