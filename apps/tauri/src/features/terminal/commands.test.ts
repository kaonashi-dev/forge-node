import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { pasteClipboard } from "./commands";
import { pasteEditorClipboard } from "../editor/cells/commands";
import { setConnectionStore } from "../../state/connection";

const commands: Record<string, unknown>[] = [];

beforeEach(() => {
  commands.length = 0;
  vi.stubGlobal("window", {});
  mockIPC((name, args) => {
    // No host pasteboard reader on this platform: the answer that sends the
    // editor paste down the WebView read these tests cover.
    if (name === "paste_editor_clipboard") return false;
    commands.push((args as { command: Record<string, unknown> }).command);
  });
  setConnectionStore({ activeSession: "A", activeTerminal: "terminal-A", connectionGeneration: 7 });
});

afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
  setConnectionStore({ activeSession: null, activeTerminal: null, connectionGeneration: 0 });
});

it.each(["terminal", "editor"] as const)(
  "%s clipboard reads retain their original destination",
  async (kind) => {
    let resolve!: (text: string) => void;
    const input = new Promise<string>((done) => {
      resolve = done;
    });
    // Resolved up front: the editor route starts the read after the host's
    // answer, so the test cannot depend on when `read` is called.
    const paste =
      kind === "terminal"
        ? pasteClipboard(() => input)
        : pasteEditorClipboard(
            "A",
            () => input,
            () => 1,
          );
    setConnectionStore({ activeSession: "B", activeTerminal: "terminal-B" });
    resolve("first\nsecond\n");
    await paste;
    expect(commands).toHaveLength(1);
    expect(commands[0]).toMatchObject({ session_id: "A", text: "first\nsecond\n" });
    if (kind === "terminal") {
      expect(commands[0]).toMatchObject({ terminal_id: "terminal-A", connection_generation: 7 });
    }
  },
);

it.each(["terminal", "editor"] as const)(
  "%s clipboard reads are cancelled across reconnection",
  async (kind) => {
    let resolve!: (text: string) => void;
    const input = new Promise<string>((done) => {
      resolve = done;
    });
    const paste =
      kind === "terminal"
        ? pasteClipboard(() => input)
        : pasteEditorClipboard(
            "A",
            () => input,
            () => 1,
          );
    setConnectionStore("connectionGeneration", 8);
    resolve("command\n");
    await paste;
    expect(commands).toEqual([]);
  },
);
