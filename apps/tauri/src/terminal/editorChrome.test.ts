import { describe, expect, it } from "vitest";
import { UNKNOWN_MARK, editorChrome } from "./editorChrome";
import type { EditorState } from "../runtime/types";

const state = (partial: Partial<EditorState> = {}): EditorState => ({
  path: "src/main.rs",
  line: 12,
  column: 4,
  dirty: false,
  read_only: true,
  document_version: 1,
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
});
