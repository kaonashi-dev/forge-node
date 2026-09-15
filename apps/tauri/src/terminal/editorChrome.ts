import type { EditorState } from "../runtime/types";

/**
 * What the terminal editor pane's header shows.
 *
 * `position` is `null` rather than a guess: the state comes from the editor's
 * control channel by way of the daemon (R14/R29), and until the first one
 * lands there is no caret to report. Showing `1:1` there would be a number the
 * editor never said.
 */
export type EditorChrome = {
  path: string;
  /** `line:column`, or `null` while no state has arrived. */
  position: string | null;
  /** The dirty/read-only mark, or what to say instead of stale values. */
  mark: string;
};

/** Said instead of a position or a mark before the first state arrives. */
export const UNKNOWN_MARK = "state unknown";

/**
 * Derive the header from the control state, falling back to what the view was
 * opened with.
 *
 * Every field comes from `Session.editor` — never from ANSI output or an OSC
 * title (R29). The pane paints the editor's own screen; reading its chrome off
 * that screen would mean parsing the TUI's status line back out of the cells
 * it just drew.
 */
export function editorChrome(
  state: EditorState | null | undefined,
  openedPath: string,
): EditorChrome {
  if (!state) return { path: openedPath, position: null, mark: UNKNOWN_MARK };
  const flags = [state.dirty ? "dirty" : null, state.read_only ? "read-only" : null].filter(
    (flag): flag is string => flag !== null,
  );
  return {
    path: state.path,
    position: `${state.line}:${state.column}`,
    mark: flags.join(" · ") || "clean",
  };
}
