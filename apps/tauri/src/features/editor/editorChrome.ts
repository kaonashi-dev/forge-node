import type { EditorState } from "../../contracts/runtime";

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
  /** `Ln 12, Col 4`, or `null` while no state has arrived. */
  position: string | null;
  /** Further status-bar facts the editor reported, in reading order. */
  details: string[];
  /** The unsaved/read-only mark, or what to say instead of stale values. */
  mark: string;
  /**
   * Where the scrollbar thumb sits, as two fractions of the buffer, or `null`
   * when the whole file is on screen and a thumb would say nothing.
   */
  scroll: { top: number; size: number } | null;
};

/**
 * The thumb for a viewport, or `null` when there is nothing to scroll.
 *
 * Derived from what the editor reports rather than measured off the canvas:
 * the pane paints a passive cell grid and has no second copy of the text, so
 * the only honest denominator is the editor's own line count.
 */
function scrollThumb(state: EditorState): EditorChrome["scroll"] {
  const total = state.total_lines;
  const visible = state.visible_lines;
  if (total <= 0 || visible <= 0 || visible >= total) return null;
  const top = Math.max(0, state.top_line - 1);
  return { top: Math.min(1, top / total), size: Math.min(1, visible / total) };
}

/** Said instead of a position or a mark before the first state arrives. */
export const UNKNOWN_MARK = "state unknown";

/** The path's folders, outermost first, then the file's own name. */
export function pathCrumbs(path: string): { parents: string[]; name: string } {
  const parts = path.split("/").filter(Boolean);
  return { parents: parts.slice(0, -1), name: parts.at(-1) ?? path };
}

/**
 * The file's extension as a label, or `null` when the name has none.
 *
 * Read off the name only: the editor reports no language, and naming one from
 * a guess at the contents would be a fact nobody stated.
 */
export function languageLabel(path: string): string | null {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  // A leading dot is a dotfile's name, not an extension.
  if (dot <= 0 || dot === name.length - 1) return null;
  return name.slice(dot + 1).toUpperCase();
}

function details(state: EditorState, path: string): string[] {
  const facts: string[] = [];
  if (state.total_lines > 0)
    facts.push(`${state.total_lines} ${state.total_lines === 1 ? "line" : "lines"}`);
  if (state.cursor_count > 1) facts.push(`${state.cursor_count} cursors`);
  if (state.selection_length > 0) facts.push(`${state.selection_length} selected`);
  const language = languageLabel(path);
  if (language) facts.push(language);
  return facts;
}

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
  if (!state) {
    const language = languageLabel(openedPath);
    return {
      path: openedPath,
      position: null,
      details: language ? [language] : [],
      mark: UNKNOWN_MARK,
      scroll: null,
    };
  }
  const flags = [state.dirty ? "unsaved" : null, state.read_only ? "read-only" : null].filter(
    (flag): flag is string => flag !== null,
  );
  return {
    path: state.path,
    position: `Ln ${state.line}, Col ${state.column}`,
    details: details(state, state.path),
    mark: flags.join(" · ") || "clean",
    scroll: scrollThumb(state),
  };
}
