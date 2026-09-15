/**
 * Whether `EditorView` offers "Open in terminal editor" (R26, feature 19).
 *
 * Both gates have to hold. The flag is the opt-in: H1 is a second surface for
 * the same file, and the DOM editor stays the default until the route is the
 * product. `unopenableReason` is what the DOM editor is already showing
 * instead of the file — binary, or past the read budget — and the daemon
 * refuses an editor session for exactly those (R5), so offering the action
 * there would promise a buffer that comes back as an error.
 *
 * "Open in terminal editor" is not "Open in editor": that one hands the path
 * to the machine's external editor and is not gated on either of these.
 */
export function offersTerminalEditor(flagOn: boolean, unopenableReason: string | null): boolean {
  return flagOn && unopenableReason === null;
}
