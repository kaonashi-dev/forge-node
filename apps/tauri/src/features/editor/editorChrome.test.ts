import { describe, expect, it } from "vitest";
import { UNKNOWN_MARK, editorChrome } from "./editorChrome";
import type { EditorState } from "../../contracts/runtime";

const state = (partial: Partial<EditorState> = {}): EditorState => ({
  path: "src/main.rs",
  line: 12,
  column: 4,
  dirty: false,
  read_only: true,
  document_version: 1,
  top_line: 1,
  visible_lines: 30,
  total_lines: 120,
  caret_line: "fn main() {}",
  selection_length: 0,
  cursor_count: 1,
  status: "",
  conflict: false,
  ...partial,
});

describe("the terminal editor chrome", () => {
  it("the_editor_chrome_renders_editor_state", () => {
    const chrome = editorChrome(state({ dirty: true, read_only: false }), "opened/with.rs");
    // The control state's path, not the one the view was opened with: the
    // daemon resolved it, and that is what the editor has open.
    expect(chrome.path).toBe("src/main.rs");
    expect(chrome.position).toBe("12:4");
    expect(chrome.mark).toBe("dirty");

    expect(editorChrome(state(), "opened/with.rs").mark).toBe("read-only");
    expect(editorChrome(state({ dirty: true }), "opened/with.rs").mark).toBe("dirty · read-only");
    expect(editorChrome(state({ read_only: false }), "opened/with.rs").mark).toBe("clean");
  });

  /*
   * R29: before the first state arrives there is nothing honest to show. A
   * position invented here would be indistinguishable from one the editor
   * reported, and on a reconnect it would be a stale one.
   */
  it("says the state is unknown rather than inventing a position", () => {
    for (const missing of [null, undefined]) {
      const chrome = editorChrome(missing, "opened/with.rs");
      expect(chrome.path).toBe("opened/with.rs");
      expect(chrome.position).toBeNull();
      expect(chrome.mark).toBe(UNKNOWN_MARK);
    }
  });
  /*
   * The thumb is the editor's own viewport, not a measurement of the canvas:
   * the pane has no copy of the text, so `total_lines` is the only honest
   * denominator, and a file that fits gets no thumb at all.
   */
  it("reports the scroll position the editor published", () => {
    expect(
      editorChrome(state({ top_line: 31, visible_lines: 30, total_lines: 120 }), "x").scroll,
    ).toEqual({ top: 0.25, size: 0.25 });
    expect(
      editorChrome(state({ top_line: 1, visible_lines: 30, total_lines: 30 }), "x").scroll,
    ).toBeNull();
    expect(
      editorChrome(state({ top_line: 1, visible_lines: 0, total_lines: 0 }), "x").scroll,
    ).toBeNull();
    expect(editorChrome(null, "x").scroll).toBeNull();
  });
});
