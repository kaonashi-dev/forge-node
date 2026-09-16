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
  currentViews,
  focus,
  openEditorTerminal,
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
  setWorkbenchStore("workspace", `close-test-${++workspace}`);
  setRuntimeStore("confirm", null);
  setForgeStore("sessions", [
    sessionFixture({ id: "dirty", kind: "Editor", editor: { ...editor, dirty: true } }),
  ]);
  openEditorTerminal("dirty", "draft.ts");
});

describe("closing editor tabs", () => {
  it("closes a confirmed clean buffer without a prompt", () => {
    setForgeStore("sessions", 0, "editor", { ...editor, dirty: false });
    close(currentViews().active);
    expect(runtimeStore.confirm).toBeNull();
    expect(currentViews().open).toHaveLength(0);
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
