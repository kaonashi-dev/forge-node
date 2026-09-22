import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { pasteClipboard } from "./commands";
import { pasteEditorClipboard } from "../editor/cells/commands";
import { setConnectionStore } from "../../state/connection";

const commands: Record<string, unknown>[] = [];

beforeEach(() => {
  commands.length = 0;
  vi.stubGlobal("window", {});
  mockIPC((_name, args) => {
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
    const read = () =>
      new Promise<string>((done) => {
        resolve = done;
      });
    const paste =
      kind === "terminal" ? pasteClipboard(read) : pasteEditorClipboard("A", read, () => 1);
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
    const read = () =>
      new Promise<string>((done) => {
        resolve = done;
      });
    const paste =
      kind === "terminal" ? pasteClipboard(read) : pasteEditorClipboard("A", read, () => 1);
    setConnectionStore("connectionGeneration", 8);
    resolve("command\n");
    await paste;
    expect(commands).toEqual([]);
  },
);
