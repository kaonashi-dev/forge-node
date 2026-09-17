// What a screen reader is told about the terminal editor.
//
// The pane paints a `<canvas>`, which an accessibility tree cannot read at all,
// so the editor's state is mirrored into a hidden DOM node beside it. The
// mirror is never editable and never the input path: the `<textarea>` still
// takes every key, and this only says what happened.
//
// Nothing here reads the cell grid. Everything comes from `Session.editor`,
// which the editor process publishes over its control channel — including the
// caret's line, which is the one piece of document text that travels.

import type { EditorState } from "../runtime/types";

/** What the hidden mirror node says, ready to spread onto an element. */
export type EditorAria = {
  /** The file, for the region's name. */
  label: string;
  /** The caret's line and what is on it. Read on every move. */
  line: string;
  /** Position, flags and selection, as one announceable sentence. */
  status: string;
  /** Whether editing is refused; a reader says "read only" for it. */
  readOnly: boolean;
  /** Total lines, so a reader can say "line 12 of 300". */
  lineCount: number;
};

/** Said instead of a line before the first state arrives. */
export const NO_STATE = "The editor has not reported its state yet.";

/**
 * Describe the buffer for a screen reader.
 *
 * `null` state is not "empty": until the first control message lands there is
 * nothing honest to announce, and inventing `1:1` would be indistinguishable
 * from a caret the editor actually reported.
 */
export function editorAria(state: EditorState | null | undefined, openedPath: string): EditorAria {
  if (!state) {
    return { label: openedPath, line: NO_STATE, status: NO_STATE, readOnly: false, lineCount: 0 };
  }
  const flags: string[] = [];
  if (state.read_only) flags.push("read only");
  flags.push(state.dirty ? "unsaved changes" : "saved");
  if (state.cursor_count > 1) flags.push(`${state.cursor_count} cursors`);
  if (state.selection_length > 0) {
    flags.push(
      `${state.selection_length} ${state.selection_length === 1 ? "byte" : "bytes"} selected`,
    );
  }
  const where = `Line ${state.line} of ${state.total_lines}, column ${state.column}`;
  return {
    label: state.path,
    // An empty line is said as such: a reader that announced nothing would be
    // indistinguishable from one that failed to announce.
    line: state.caret_line.trim() === "" ? "blank line" : state.caret_line,
    status: `${where}. ${flags.join(", ")}.`,
    readOnly: state.read_only,
    lineCount: state.total_lines,
  };
}

/**
 * What the live region should say, or `""` for silence.
 *
 * The editor's transient message is the answer to the gesture just made — a
 * find tally, `no match`, a save refusal — and its status row is exactly what a
 * person looking away from the canvas cannot see. A conflict is added by the
 * pane rather than the editor, because the daemon is what saw the revision
 * mismatch and the editor was only told a reason.
 */
export function editorAnnouncement(state: EditorState | null | undefined): string {
  if (!state) return "";
  if (state.conflict) {
    return "This file changed on disk while you were editing it. Keep mine, take disk, or compare.";
  }
  return state.status.trim();
}
