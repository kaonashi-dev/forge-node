import { expect, it, vi } from "vitest";
import { createEditorAutosaveSync } from "./editorAutosave";
import { sessionFixture } from "./sessions.fixture";

it("updates live editors on preference changes and reconnect, not caret changes", () => {
  const send = vi.fn().mockResolvedValue(undefined);
  const sync = createEditorAutosaveSync(send);
  const editor = sessionFixture({ id: "editor", kind: "Editor", state: "Running" });
  const sessions = [editor, sessionFixture({ kind: "Shell" })];
  sync(sessions, false);
  sync(sessions, false);
  expect(send).toHaveBeenCalledTimes(1);
  sync(sessions, true);
  expect(send).toHaveBeenLastCalledWith("editor", true);
  sync(sessions, false);
  expect(send).toHaveBeenLastCalledWith("editor", false);
  sync(sessions, false, true);
  expect(send).toHaveBeenCalledTimes(4);
  sync([], false);
  sync(sessions, false);
  expect(send).toHaveBeenCalledTimes(5);
});
