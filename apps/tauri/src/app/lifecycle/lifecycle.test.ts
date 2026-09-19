import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { openEditorTerminal, viewsStore } from "../../navigation/viewsStore";
import {
  directories,
  directoryError,
  directoryTree,
} from "../../features/files/directories/directoryState";
import { pathOperations } from "../../features/files/operations/operations";
import { focusWorkspace } from "../../state/workspace";
import {
  createPath,
  ensureDirectory,
  renamePath,
  searchFiles,
} from "../../features/files/commands";
import { filesStore, setFilesStore } from "../../features/files/state";

describe("correlated workbench events", () => {
  let dispose: (() => void) | undefined;
  const commands: Record<string, unknown>[] = [];
  beforeEach(async () => {
    vi.useFakeTimers();
    const document = { addEventListener: vi.fn(), body: { addEventListener: vi.fn() } };
    vi.stubGlobal("window", { document, setInterval, clearInterval, crypto });
    vi.stubGlobal("document", document);
    mockIPC(
      (name, args) => {
        if (name === "send_workbench_command")
          commands.push((args as { command: Record<string, unknown> }).command);
      },
      { shouldMockEvents: true },
    );
    const { startAppRuntime } = await import("./index");
    dispose = await startAppRuntime();
    focusWorkspace("w");
    directories.connection(true);
    commands.length = 0;
  });
  afterEach(() => {
    dispose?.();
    pathOperations.disconnect();
    directories.connection(false);
    focusWorkspace(null);
    clearMocks();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });
  it("uses the host directory event names and rejects stale failures", async () => {
    ensureDirectory("w");
    await vi.advanceTimersByTimeAsync(100);
    const request = commands[0];
    await emit("workbench:directory", {
      ...request,
      entries: [{ path: ".agents", kind: "Directory", ignored: false }],
      truncated: false,
    });
    await emit("workbench:directory_failed", { ...request, message: "Late failure" });
    expect(directoryTree()?.entries[0].path).toBe(".agents");
    expect(directoryError()).toBeNull();
  });
  it("an unrelated read failure cannot settle a mutation", async () => {
    const promise = createPath("w", ".agents", true);
    const done = vi.fn();
    void promise.then(done);
    await Promise.resolve();
    const command = commands[0];
    await emit("workbench:file_failed", { workspace: "w", error: "Read denied" });
    await Promise.resolve();
    expect(done).not.toHaveBeenCalled();
    await emit("workbench:path_result", {
      operation_id: command.operation_id,
      workspace: "w",
      kind: "create",
      from: null,
      to: ".agents",
      success: true,
      error: null,
      uncertain: false,
    });
    await expect(promise).resolves.toMatchObject({ success: true });
    expect(directoryTree()?.entries).toEqual([
      { path: ".agents", kind: "Directory", ignored: false },
    ]);
  });
  it("disconnect rejects a pending operation as uncertain", async () => {
    const promise = createPath("w", "new", false);
    const rejected = expect(promise).rejects.toMatchObject({ uncertain: true });
    await Promise.resolve();
    await emit("runtime:disconnected", { reason: "lost" });
    await rejected;
    expect(commands.filter((command) => command.type === "create_path")).toHaveLength(1);
  });
  it("keeps confirmed creation separate from a failed parent refresh", async () => {
    ensureDirectory("w");
    const promise = createPath("w", "new.ts", false);
    await Promise.resolve();
    const command = commands.find((item) => item.type === "create_path")!;
    await emit("workbench:path_result", {
      operation_id: command.operation_id,
      workspace: "w",
      kind: "create",
      from: null,
      to: "new.ts",
      success: true,
      error: null,
      uncertain: false,
    });
    await expect(promise).resolves.toMatchObject({ success: true });
    await vi.advanceTimersByTimeAsync(100);
    const read = commands.filter((item) => item.type === "load_file_directory").at(-1)!;
    await emit("workbench:directory_failed", { ...read, message: "Permission denied" });
    expect(directoryError()).toBe("Permission denied");
    expect(directoryTree()?.entries.some((entry) => entry.path === "new.ts")).toBe(true);
  });
  it("name search cannot clear find-in-files staleness", async () => {
    setFilesStore({ treeVersion: 0, searchReadVersion: 0, searchStale: false });
    await searchFiles("w", "needle", "content");
    expect(filesStore.searchReadVersion).toBe(0);
    await emit("workbench:file_changed", ["w", "src/a.ts"]);
    expect(filesStore.searchStale).toBe(true);
    expect(filesStore.treeVersion).toBe(1);
    await searchFiles("w", "a.ts", "name");
    expect(filesStore.searchReadVersion).toBe(0);
    await emit("workbench:search", [
      "w",
      { workspace_id: "w", query: "needle", matches: [], truncated: false },
    ]);
    expect(filesStore.searchStale).toBe(true);
  });
  it("retargets the existing editor session and index on a confirmed rename result", async () => {
    openEditorTerminal("moving-editor", "src/a.ts", "w");
    setFilesStore("tree", {
      workspace_id: "w",
      entries: [
        { path: "src/a.ts", kind: "File", ignored: false },
        { path: "src-extra/a.ts", kind: "File", ignored: false },
      ],
      truncated: false,
    });
    const promise = renamePath("w", "src", "lib");
    await Promise.resolve();
    const command = commands.find((item) => item.type === "rename_path")!;
    await emit("workbench:path_result", {
      operation_id: command.operation_id,
      workspace: "w",
      kind: "rename",
      from: "src",
      to: "lib",
      success: true,
      error: null,
      uncertain: false,
    });
    await promise;
    expect(viewsStore.byWorkspace.w.open).toContainEqual({
      kind: "editor-terminal",
      session: "moving-editor",
      path: "lib/a.ts",
    });
    expect(filesStore.tree?.entries.map((entry) => entry.path)).toEqual([
      "lib/a.ts",
      "src-extra/a.ts",
    ]);
  });
});
