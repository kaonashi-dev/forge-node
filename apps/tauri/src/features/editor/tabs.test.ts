import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorState } from "../../contracts/runtime";
import { sessionFixture } from "../../contracts/sessions.fixture";
import {
  centerMode,
  codeOpen,
  currentViews,
  focus,
  openEditorTerminal,
  showCode,
  showSession,
} from "../../navigation/viewsStore";
import { setForgeStore } from "../../state/forgeStore";
import { dialogsStore, setDialogsStore } from "../../state/dialogs";
import { setActiveWorkspace } from "../../state/workspace";
import { close, closeCode, closeOtherViews, closeViewsToRight } from "./tabs";

vi.mock("./cells/commands", () => ({
  closeEditor: vi.fn().mockResolvedValue(undefined),
}));

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
  setDialogsStore("confirm", null);
  setForgeStore("sessions", [
    sessionFixture({ id: "dirty", kind: "Editor", editor: { ...editor, dirty: true } }),
  ]);
  openEditorTerminal("dirty", "draft.ts");
});

describe("closing editor tabs", () => {
  it("offers Code before opening any file in a workspace", () => {
    setActiveWorkspace(`empty-${workspace}`);
    expect(currentViews().open).toHaveLength(0);
    expect(codeOpen()).toBe(true);
    showCode();
    expect(centerMode()).toBe("code");
    closeCode();
    expect(codeOpen()).toBe(true);
    expect(centerMode()).toBe("code");
    setActiveWorkspace(null);
    expect(codeOpen()).toBe(false);
  });

  it("closes a confirmed clean buffer without a prompt", () => {
    setForgeStore("sessions", 0, "editor", { ...editor, dirty: false });
    close(currentViews().active);
    expect(dialogsStore.confirm).toBeNull();
    expect(currentViews().open).toHaveLength(0);
    expect(codeOpen()).toBe(true);
    expect(centerMode()).toBe("code");
  });

  it("does not assume a buffer is saved before its first state arrives", () => {
    setForgeStore("sessions", [sessionFixture({ id: "dirty", kind: "Editor" })]);
    close(currentViews().active);
    expect(dialogsStore.confirm).not.toBeNull();
    expect(currentViews().open).toHaveLength(1);
  });
  it("keeps the tab while confirmation is pending or cancelled", () => {
    close(currentViews().active);
    expect(dialogsStore.confirm).not.toBeNull();
    expect(currentViews().open).toHaveLength(1);
    setDialogsStore("confirm", null);
    expect(currentViews().open).toHaveLength(1);
    close(currentViews().active);
    dialogsStore.confirm?.onConfirm();
    expect(currentViews().open).toHaveLength(0);
  });

  it.each([closeCode, closeOtherViews, closeViewsToRight])(
    "guards bulk closing through %s",
    (run) => {
      const draft = currentViews().active;
      openEditorTerminal("other", "other.ts");
      if (run === closeViewsToRight) focus(draft);
      run();
      expect(dialogsStore.confirm).not.toBeNull();
      expect(currentViews().open).toHaveLength(2);
    },
  );

  it("applies a delayed confirmation to the checkout it was opened for", () => {
    closeCode();
    const confirm = dialogsStore.confirm?.onConfirm;
    setActiveWorkspace(`elsewhere-${workspace}`);
    openEditorTerminal("other", "other.ts");
    confirm?.();
    expect(currentViews().open).toHaveLength(1);
    setActiveWorkspace(`close-test-${workspace}`);
    expect(currentViews().open).toHaveLength(0);
  });
});
