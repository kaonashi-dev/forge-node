import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorState } from "../runtime/types";
import { sessionFixture } from "../runtime/sessions.fixture";
import { setForgeStore } from "./forgeStore";
import { runtimeStore, setRuntimeStore } from "./runtimeStore";
import { setWorkbenchStore } from "./workbenchStore";
import {
  close,
  closeCode,
  closeOtherViews,
  closeViewsToRight,
  centerMode,
  codeOpen,
  clearFindInFiles,
  currentViews,
  findInFilesPending,
  focus,
  openEditorTerminal,
  requestFindInFiles,
  showSession,
  showCode,
} from "./viewsStore";

vi.mock("../runtime/api", () => ({ closeEditor: vi.fn().mockResolvedValue(undefined) }));

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
  clearFindInFiles();
  setWorkbenchStore("workspace", `close-test-${++workspace}`);
  setRuntimeStore("confirm", null);
  setForgeStore("sessions", [
    sessionFixture({ id: "dirty", kind: "Editor", editor: { ...editor, dirty: true } }),
  ]);
  openEditorTerminal("dirty", "draft.ts");
});

describe("Find in Files", () => {
  it("opens a Code search tab from a session and reuses it on repeated requests", () => {
    showSession();
    requestFindInFiles();
    expect(centerMode()).toBe("code");
    expect(currentViews().active).toEqual({ kind: "search" });
    expect(findInFilesPending()).toEqual({ query: null });
    clearFindInFiles();
    requestFindInFiles();
    expect(currentViews().open.filter((view) => view.kind === "search")).toHaveLength(1);
    expect(findInFilesPending()).toEqual({ query: null });
  });

  it("keeps a supplied query pending until the search tab consumes it", () => {
    requestFindInFiles("wompi");
    expect(findInFilesPending()).toEqual({ query: "wompi" });
    expect(currentViews().active).toEqual({ kind: "search" });
    clearFindInFiles();
    expect(findInFilesPending()).toBeNull();
  });

  it("does not leave a search request for an unrelated checkout when none is selected", () => {
    setWorkbenchStore("workspace", null);
    showSession();
    requestFindInFiles();
    expect(findInFilesPending()).toBeNull();
    expect(centerMode()).toBe("session");
  });
});

describe("closing editor tabs", () => {
  it("offers Code before opening any file in a workspace", () => {
    setWorkbenchStore("workspace", `empty-${workspace}`);
    expect(currentViews().open).toHaveLength(0);
    expect(codeOpen()).toBe(true);
    showCode();
    expect(centerMode()).toBe("code");
    closeCode();
    expect(codeOpen()).toBe(true);
    expect(centerMode()).toBe("code");
    setWorkbenchStore("workspace", null);
    expect(codeOpen()).toBe(false);
  });

  it("closes a confirmed clean buffer without a prompt", () => {
    setForgeStore("sessions", 0, "editor", { ...editor, dirty: false });
    close(currentViews().active);
    expect(runtimeStore.confirm).toBeNull();
    expect(currentViews().open).toHaveLength(0);
    expect(codeOpen()).toBe(true);
    expect(centerMode()).toBe("code");
  });

  it("does not assume a buffer is saved before its first state arrives", () => {
    setForgeStore("sessions", [sessionFixture({ id: "dirty", kind: "Editor" })]);
    close(currentViews().active);
    expect(runtimeStore.confirm).not.toBeNull();
    expect(currentViews().open).toHaveLength(1);
  });
  it("keeps the tab while confirmation is pending or cancelled", () => {
    close(currentViews().active);
    expect(runtimeStore.confirm).not.toBeNull();
    expect(currentViews().open).toHaveLength(1);
    setRuntimeStore("confirm", null);
    expect(currentViews().open).toHaveLength(1);
    close(currentViews().active);
    runtimeStore.confirm?.onConfirm();
    expect(currentViews().open).toHaveLength(0);
  });

  it.each([closeCode, closeOtherViews, closeViewsToRight])(
    "guards bulk closing through %s",
    (run) => {
      const draft = currentViews().active;
      openEditorTerminal("other", "other.ts");
      if (run === closeViewsToRight) focus(draft);
      run();
      expect(runtimeStore.confirm).not.toBeNull();
      expect(currentViews().open).toHaveLength(2);
    },
  );

  it("applies a delayed confirmation to the checkout it was opened for", () => {
    closeCode();
    const confirm = runtimeStore.confirm?.onConfirm;
    setWorkbenchStore("workspace", `elsewhere-${workspace}`);
    openEditorTerminal("other", "other.ts");
    confirm?.();
    expect(currentViews().open).toHaveLength(1);
    setWorkbenchStore("workspace", `close-test-${workspace}`);
    expect(currentViews().open).toHaveLength(0);
  });
});
