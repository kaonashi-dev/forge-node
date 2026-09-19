import { describe, expect, it } from "vitest";

import { NO_STATE, editorAnnouncement, editorAria } from "./editorAria";
import type { EditorState } from "../../contracts/runtime";

const state = (partial: Partial<EditorState> = {}): EditorState => ({
  path: "src/main.rs",
  line: 12,
  column: 4,
  dirty: false,
  read_only: false,
  document_version: 1,
  top_line: 1,
  visible_lines: 30,
  total_lines: 120,
  caret_line: "    let total = 1;",
  selection_length: 0,
  cursor_count: 1,
  status: "",
  conflict: false,
  ...partial,
});

describe("what a screen reader is told about the editor", () => {
  it("names the file, the position and the line", () => {
    const aria = editorAria(state(), "opened/with.rs");
    expect(aria.label).toBe("src/main.rs");
    expect(aria.status).toContain("Line 12 of 120");
    expect(aria.status).toContain("column 4");
    expect(aria.status).toContain("saved");
    expect(aria.line).toBe("    let total = 1;");
    expect(aria.readOnly).toBe(false);
  });

  /*
   * A reader that announced nothing for an empty line would be
   * indistinguishable from one that failed to announce at all.
   */
  it("says a blank line is blank", () => {
    expect(editorAria(state({ caret_line: "   " }), "x").line).toBe("blank line");
  });

  it("announces the flags a person cannot see", () => {
    const aria = editorAria(
      state({ dirty: true, read_only: true, cursor_count: 3, selection_length: 7 }),
      "x",
    );
    expect(aria.status).toContain("read only");
    expect(aria.status).toContain("unsaved changes");
    expect(aria.status).toContain("3 cursors");
    expect(aria.status).toContain("7 bytes selected");
    expect(aria.readOnly).toBe(true);
    expect(editorAria(state({ selection_length: 1 }), "x").status).toContain("1 byte selected");
  });

  /*
   * R29 again: until the first control message lands there is nothing honest
   * to announce, and an invented `1:1` would be indistinguishable from one the
   * editor reported.
   */
  it("says the state is unknown rather than inventing a position", () => {
    for (const missing of [null, undefined]) {
      const aria = editorAria(missing, "opened/with.rs");
      expect(aria.label).toBe("opened/with.rs");
      expect(aria.line).toBe(NO_STATE);
      expect(aria.status).toBe(NO_STATE);
    }
  });

  /*
   * The find tally and the save refusal live in the editor's own status row,
   * which is exactly what a person looking away from the canvas cannot see.
   */
  it("repeats the editor's transient message", () => {
    expect(editorAnnouncement(state({ status: "alpha: 3/41" }))).toBe("alpha: 3/41");
    expect(editorAnnouncement(state({ status: "  " }))).toBe("");
    expect(editorAnnouncement(null)).toBe("");
  });

  /* The conflict is the daemon's word, not the editor's, so the pane says it. */
  it("announces a conflict over whatever the editor last said", () => {
    const said = editorAnnouncement(state({ conflict: true, status: "saved" }));
    expect(said).toContain("changed on disk");
  });
});
