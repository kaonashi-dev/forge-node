import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { applyShellSnapshot, emptySnapshot } from "../../state/forgeStore";
import {
  centerMode,
  currentViews,
  showSession,
  openDiff,
  openEditorTerminal,
  viewsStore,
} from "../../navigation/viewsStore";
import { sessionFixture } from "../../contracts/sessions.fixture";
import type { EditorState, Workspace } from "../../contracts/runtime";
import { focusSession, hasExited, launchShell, splitPane } from "./sessionActions";
import { centerSplit, closeSplit, openCodeSplit } from "../../navigation/centerSplitStore";
import { selectSession } from "./commands";
import { activeWorkspace, focusWorkspace } from "../../state/workspace";
import {
  clearSessionSelection,
  connectionStore,
  pendingSessionSelection,
  setConnectionStore,
} from "../../state/connection";
import { activeSwitcherKey, switcherTargets, trackTabFocus } from "../../navigation/switcherRing";
import {
  bindTabSwitcherCommit,
  commitTabSwitcher,
  resetTabSwitcher,
  stepTabSwitcher,
} from "../../navigation/tabSwitcher";
import { applyStatePayload } from "../../app/lifecycle/connection";
import { bindRuntimeEvents } from "../../app/lifecycle/runtime";
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

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
  top_line: 1,
  visible_lines: 1,
  total_lines: 1,
  caret_line: "",
  selection_length: 0,
  cursor_count: 1,
  status: "",
  conflict: false,
});

describe("session focus history", () => {
  let dispose: (() => void) | undefined;
  let unbind: (() => void) | undefined;
  const committed = vi.fn();
  const snapshot = () => ({
    ...emptySnapshot(),
    workspaces: [workspace("focus-history")],
    sessions: ["t1", "t2", "t3"].map((id) => sessionFixture({ id, workspace_id: "focus-history" })),
  });

  function receiveSession(id: string): void {
    applyStatePayload({ store: snapshot(), active_session: id, active_terminal: `term-${id}` });
  }

  function switchToPrevious(): void {
    stepTabSwitcher(1, switcherTargets(["t1", "t2", "t3"]), activeSwitcherKey());
    commitTabSwitcher();
  }

  beforeEach(() => {
    clearSessionSelection();
    resetTabSwitcher();
    committed.mockClear();
    vi.stubGlobal("window", {
      crypto,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    });
    mockIPC(() => undefined, { shouldMockEvents: true });
    focusWorkspace("focus-history");
    setConnectionStore("activeSession", "t1");
    applyShellSnapshot(snapshot());
    showSession();
    unbind = bindTabSwitcherCommit(committed);
    dispose = createRoot((cleanup) => {
      trackTabFocus();
      return cleanup;
    });
    openEditorTerminal("e1", "src/main.ts");
  });

  afterEach(() => {
    dispose?.();
    unbind?.();
    resetTabSwitcher();
    clearSessionSelection();
    focusWorkspace(null);
    clearMocks();
    vi.unstubAllGlobals();
  });

  it("returns to Code after selecting a different terminal with a delayed runtime update", async () => {
    focusSession("t2");
    await Promise.resolve();
    expect(centerMode()).toBe("session");
    expect(connectionStore.activeSession).toBe("t1");
    expect(activeSwitcherKey()).toBeNull();

    receiveSession("t1");
    expect(activeSwitcherKey()).toBeNull();
    receiveSession("t2");
    expect(pendingSessionSelection()).toBeNull();
    expect(activeSwitcherKey()).toBe("session:t2");
    switchToPrevious();
    expect(committed).toHaveBeenCalledWith({
      kind: "view",
      workspace: "focus-history",
      view: { kind: "editor-terminal", session: "e1", path: "src/main.ts" },
    });
  });

  it("can return to an already-active terminal without waiting for a runtime event", () => {
    focusSession("t1");
    expect(activeSwitcherKey()).toBe("session:t1");
    expect(pendingSessionSelection()).toBeNull();
    switchToPrevious();
    expect(committed).toHaveBeenCalledWith(expect.objectContaining({ kind: "view" }));
  });

  it("ignores an intermediate selection when the user chooses another terminal", () => {
    focusSession("t2");
    focusSession("t3");
    receiveSession("t2");
    expect(activeSwitcherKey()).toBeNull();
    receiveSession("t3");
    switchToPrevious();
    expect(committed).toHaveBeenCalledWith(expect.objectContaining({ kind: "view" }));
  });

  it("tracks a queued return to the original terminal through the intermediate update", () => {
    focusSession("t2");
    focusSession("t1");
    receiveSession("t2");
    expect(activeSwitcherKey()).toBeNull();
    receiveSession("t1");
    expect(pendingSessionSelection()).toBeNull();
    switchToPrevious();
    expect(committed).toHaveBeenCalledWith(expect.objectContaining({ kind: "view" }));
  });

  it("leaves the code pane first when the ring commits a terminal that is not active", () => {
    // The effect wakes inside `showSession()`, before the daemon has answered,
    // while `activeSession` still names the terminal being left. Recording it
    // there would put that terminal — not the file on screen — at index 1.
    focusSession("t2");
    expect(activeSwitcherKey()).toBeNull();

    receiveSession("t2");
    expect(activeSwitcherKey()).toBe("session:t2");
    switchToPrevious();

    expect(committed).toHaveBeenCalledWith(expect.objectContaining({ kind: "view" }));
  });

  it("leaves the code pane first across a launch that has no session id yet", () => {
    // `launchShell` raises the centre column before the daemon has minted the
    // session, so there is no id to wait on — only the payload that follows.
    void launchShell("focus-history");
    expect(pendingSessionSelection()).toBeNull();
    expect(activeSwitcherKey()).toBeNull();

    receiveSession("t3");
    expect(activeSwitcherKey()).toBe("session:t3");
    switchToPrevious();

    expect(committed).toHaveBeenCalledWith(expect.objectContaining({ kind: "view" }));
  });

  it("clears the pending selection when enqueueing fails", async () => {
    mockIPC(() => Promise.reject(new Error("Command channel closed")));
    focusSession("t2");
    await vi.waitFor(() => expect(pendingSessionSelection()).toBeNull());
    expect(activeSwitcherKey()).toBe("session:t1");
    switchToPrevious();
    expect(committed).toHaveBeenCalledWith(expect.objectContaining({ kind: "view" }));
  });

  it("does not cancel a newer selection when an older enqueue fails", async () => {
    let rejectFirst: ((error: Error) => void) | undefined;
    const failed = new Promise<void>((_resolve, reject) => {
      rejectFirst = reject;
    });
    mockIPC((name, args) => {
      if (
        name === "send_runtime_command" &&
        (args as { command: { session_id?: string } }).command.session_id === "t2"
      ) {
        return failed;
      }
    });
    const first = selectSession("t2");
    focusSession("t3");
    rejectFirst?.(new Error("Command channel closed"));
    await expect(first).rejects.toThrow("Command channel closed");
    expect(pendingSessionSelection()).toBe("t3");
    receiveSession("t3");
    switchToPrevious();
    expect(committed).toHaveBeenCalledWith(expect.objectContaining({ kind: "view" }));
  });

  it.each(["runtime:notice", "runtime:disconnected"])(
    "clears the pending selection on %s",
    async (event) => {
      const listeners = await bindRuntimeEvents();
      try {
        focusSession("t2");
        await emit(event, { reason: "Session unavailable" });
        expect(pendingSessionSelection()).toBeNull();
        expect(activeSwitcherKey()).toBe("session:t1");
      } finally {
        for (const unlisten of listeners) unlisten();
      }
    },
  );
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

    const invoke = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("window", { __TAURI_INTERNALS__: { invoke } });
    focusSession("e1");

    expect(activeWorkspace()).toBe("dev-main");
    expect(invoke).toHaveBeenCalledWith(
      "send_runtime_command",
      {
        command: {
          type: "reopen_editor",
          session_id: "e1",
        },
      },
      undefined,
    );
    expect(centerMode()).toBe("session");
    openEditorTerminal("e1", "src/main.rs", "dev-main");
    expect(centerMode()).toBe("code");
    expect({ ...currentViews().active }).toEqual({
      kind: "editor-terminal",
      session: "e1",
      path: "src/main.rs",
    });

    focusWorkspace(null);
    vi.unstubAllGlobals();
  });
  it("parks a late editor response in its workspace without raising it", () => {
    focusWorkspace("other-workspace");
    showSession();
    openEditorTerminal("late-editor", "original.rs", "original-workspace");
    expect(centerMode()).toBe("session");
    expect(activeWorkspace()).toBe("other-workspace");
    expect(viewsStore.byWorkspace["original-workspace"]?.active).toEqual({
      kind: "editor-terminal",
      session: "late-editor",
      path: "original.rs",
    });
    expect(currentViews().active).not.toMatchObject({ session: "late-editor" });
    focusWorkspace(null);
  });
});

describe("hasExited", () => {
  it("is a session whose process is gone and whose terminal went with it", () => {
    expect(hasExited({ state: { Exited: { code: 0, signal: null } }, terminal_id: null })).toBe(
      true,
    );
    expect(hasExited({ state: "Orphaned", terminal_id: null })).toBe(true);
  });

  it("is never a live session, even between spawn and its first terminal", () => {
    expect(hasExited({ state: "Starting", terminal_id: null })).toBe(false);
    expect(hasExited({ state: "Running", terminal_id: "t" })).toBe(false);
  });
});

describe("splitPane", () => {
  afterEach(() => {
    closeSplit();
    focusWorkspace(null);
    showSession();
  });

  it("puts a file beside the current session without leaving Code", () => {
    focusWorkspace("split-ws");
    setConnectionStore("activeSession", "t1");
    applyShellSnapshot({
      ...emptySnapshot(),
      workspaces: [workspace("split-ws")],
      sessions: [sessionFixture({ id: "t1", workspace_id: "split-ws" })],
    });
    openEditorTerminal("e1", "src/main.rs", "split-ws");
    expect(centerMode()).toBe("code");
    splitPane();
    expect(centerSplit().kind).toBe("code");
    expect(centerMode()).toBe("code");
  });

  it("keeps the file on screen when a session tab is clicked in a code split", async () => {
    focusWorkspace("split-ws");
    setConnectionStore("activeSession", "t1");
    applyShellSnapshot({
      ...emptySnapshot(),
      workspaces: [workspace("split-ws")],
      sessions: [
        sessionFixture({ id: "t1", workspace_id: "split-ws" }),
        sessionFixture({ id: "t2", workspace_id: "split-ws" }),
      ],
    });
    openEditorTerminal("e1", "src/main.rs", "split-ws");
    openCodeSplit();
    vi.stubGlobal("window", { crypto, addEventListener: vi.fn(), removeEventListener: vi.fn() });
    mockIPC(() => undefined);
    focusSession("t2");
    await Promise.resolve();
    expect(centerMode()).toBe("code");
    expect(centerSplit().focused).toBe("extra");
    clearMocks();
    vi.unstubAllGlobals();
  });

  it("does not split a diff", () => {
    focusWorkspace("split-ws");
    applyShellSnapshot({
      ...emptySnapshot(),
      workspaces: [workspace("split-ws")],
    });
    openDiff();
    expect(currentViews().active).toEqual({ kind: "diff" });
    splitPane();
    expect(centerSplit().kind).toBe("closed");
  });
});

describe("opening an exited session", () => {
  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
  });

  it("restarts it instead of asking to attach to a terminal that is gone", async () => {
    vi.stubGlobal("window", { crypto, addEventListener: vi.fn(), removeEventListener: vi.fn() });
    const sent: unknown[] = [];
    mockIPC((_cmd, args) => {
      sent.push((args as { command?: unknown } | undefined)?.command);
    });
    applyShellSnapshot({
      ...emptySnapshot(),
      workspaces: [workspace("exited")],
      sessions: [
        sessionFixture({
          id: "gone",
          workspace_id: "exited",
          state: { Exited: { code: 0, signal: null } },
          terminal_id: null,
        }),
      ],
    });
    focusSession("gone");
    await Promise.resolve();
    expect(sent).toContainEqual(
      expect.objectContaining({ type: "restart_session", session_id: "gone" }),
    );
    expect(sent).not.toContainEqual(expect.objectContaining({ type: "select_session" }));
    focusWorkspace(null);
  });
});
