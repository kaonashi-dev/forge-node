import { beforeEach, describe, expect, it } from "vitest";
import type { EditorState } from "../../contracts/runtime";
import { sessionFixture } from "../../contracts/sessions.fixture";
import {
  centerMode,
  currentViews,
  openEditorTerminal,
  showSession,
} from "../../navigation/viewsStore";
import { setForgeStore } from "../../state/forgeStore";
import { setActiveWorkspace } from "../../state/workspace";
import { syncEditorViewPaths } from "./editorTabs";

const editor: EditorState = {
  path: "draft.ts",
  line: 1,
  column: 1,
  dirty: true,
  read_only: false,
  document_version: 1,
  top_line: 1,
  visible_lines: 1,
  total_lines: 1,
  caret_line: "draft",
  selection_length: 0,
  cursor_count: 1,
  status: "",
  conflict: false,
};

let workspace = 0;
beforeEach(() => {
  setActiveWorkspace(`close-test-${++workspace}`);
  setForgeStore("sessions", [
    sessionFixture({ id: "dirty", kind: "Editor", editor: { ...editor, dirty: true } }),
  ]);
  openEditorTerminal("dirty", "draft.ts");
});

describe("editor tabs", () => {
  it("adopts authoritative editor paths without opening another session or switching modes", () => {
    showSession();
    syncEditorViewPaths([
      sessionFixture({ id: "dirty", kind: "Editor", editor: { ...editor, path: "renamed.ts" } }),
    ]);
    expect(currentViews().active).toEqual({
      kind: "editor-terminal",
      session: "dirty",
      path: "renamed.ts",
    });
    expect(currentViews().open).toHaveLength(1);
    expect(centerMode()).toBe("session");
  });
});
