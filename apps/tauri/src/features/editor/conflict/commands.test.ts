import { afterEach, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { loadEditorConflict } from "./commands";
import { clearEditorConflict, editorConflictFor } from "./editorConflictStore";
afterEach(() => {
  clearEditorConflict("editor-A");
  clearMocks();
  vi.unstubAllGlobals();
});

it("requests conflict contents and releases loading after enqueue rejection", async () => {
  vi.stubGlobal("window", {});
  const send = vi.fn().mockRejectedValueOnce(new Error("queue full"));
  mockIPC(send);
  await expect(loadEditorConflict("editor-A")).rejects.toThrow("queue full");
  expect(send).toHaveBeenCalledWith("send_workbench_command", {
    command: { type: "load_editor_conflict", session: "editor-A" },
  });
  expect(editorConflictFor("editor-A").loading).toBe(false);
  expect(editorConflictFor("editor-A").error).toContain("queue full");
  send.mockResolvedValueOnce(undefined);
  await loadEditorConflict("editor-A");
  expect(editorConflictFor("editor-A").loading).toBe(true);
  expect(editorConflictFor("editor-A").error).toBeNull();
  clearEditorConflict("editor-A");
});
