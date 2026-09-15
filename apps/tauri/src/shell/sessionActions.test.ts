import { describe, expect, it } from "vitest";
import { applyShellSnapshot, emptySnapshot } from "../store/forgeStore";
import { centerMode, currentViews, showSession } from "../store/viewsStore";
import { focusWorkspace, workbenchStore } from "../store/workbenchStore";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { EditorState, Workspace } from "../runtime/types";
import { focusSession } from "./sessionActions";

const workspace = (id: string): Workspace => ({
  id,
  project_id: "dev",
  kind: "GitWorktree",
  path: `/tmp/${id}`,
  branch: "main",
  display_name: null,
  managed_by_app: false,
  status: { dirty: false, head: null, ahead: null, behind: null, measured_at: null },
});

const editorState = (path: string): EditorState => ({
  path,
  line: 0,
  column: 0,
  dirty: false,
  read_only: false,
  document_version: 0,
  conflict: false,
});

describe("focusSession", () => {
  it("opens an editor in the Code strip rather than the main terminal", () => {
    // Start on a terminal, so a wrong route would show as the mode staying on
    // "session" instead of raising Code.
    focusWorkspace(null);
    showSession();
    applyShellSnapshot({
      ...emptySnapshot(),
      workspaces: [workspace("dev-main")],
      sessions: [
        sessionFixture({
          id: "e1",
          workspace_id: "dev-main",
          kind: "Editor",
          editor: editorState("src/main.rs"),
        }),
      ],
    });

    focusSession("e1");

    expect(workbenchStore.workspace).toBe("dev-main");
    expect(centerMode()).toBe("code");
    expect({ ...currentViews().active }).toEqual({
      kind: "editor-terminal",
      session: "e1",
      path: "src/main.rs",
    });

    focusWorkspace(null);
  });
});
