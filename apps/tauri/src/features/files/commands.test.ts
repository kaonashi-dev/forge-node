import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { reconcile } from "solid-js/store";
import {
  acceptFileIndex,
  failFileIndex,
  warmFileTree,
  createPath,
  deletePath,
  renamePath,
  ensureDirectory,
  searchFiles,
} from "./commands";
import { directories, directoryTree } from "./directories/directoryState";
import { OPERATION_TIMEOUT_MS, pathOperations } from "./operations/operations";
import { startFileMutationEffects } from "./operations/effects";
import { focusWorkspace, onWorkspaceChange } from "../../state/workspace";
import { filesStore, invalidateFileIndex, resetFilesAnswers, setFilesStore } from "./state";
import { loading, setLoading } from "../../state/loading";
import { navigationFileIndex } from "./index/fileIndex";

describe("workbench path transport", () => {
  const commands: Record<string, unknown>[] = [];
  let stopPathMutations: () => void;
  let stopWorkspaceFocus: () => void;
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("window", {});
    mockIPC((name, args) => {
      if (name === "send_workbench_command")
        commands.push((args as { command: Record<string, unknown> }).command);
    });
    commands.length = 0;
    focusWorkspace("w");
    directories.focus("w");
    resetFilesAnswers();
    setLoading(reconcile({}));
    directories.connection(true);
    stopWorkspaceFocus = onWorkspaceChange((workspace) => {
      directories.focus(workspace);
      resetFilesAnswers();
      setLoading(reconcile({}));
    });
    stopPathMutations = startFileMutationEffects();
  });
  afterEach(() => {
    stopPathMutations();
    stopWorkspaceFocus();
    pathOperations.disconnect();
    directories.connection(false);
    directories.focus(null);
    focusWorkspace(null);
    clearMocks();
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });
  it.each(["create", "rename", "delete"] as const)(
    "%s carries identity and resolves on explicit result without root-index reload",
    async (kind) => {
      const promise =
        kind === "create"
          ? createPath("w", "a", false)
          : kind === "rename"
            ? renamePath("w", "a", "b")
            : deletePath("w", "a");
      await Promise.resolve();
      const command = commands[0];
      expect(command.type).toBe(`${kind}_path`);
      expect(typeof command.operation_id).toBe("string");
      pathOperations.settle({
        operation_id: command.operation_id as string,
        workspace: "w",
        kind,
        ...(kind === "create"
          ? { to: "a" }
          : kind === "rename"
            ? { from: "a", to: "b" }
            : { from: "a" }),
        success: true,
        uncertain: false,
      });
      await expect(promise).resolves.toMatchObject({ success: true });
      await vi.advanceTimersByTimeAsync(100);
      expect(commands.some((item) => item.type === "load_file_tree")).toBe(false);
      expect(filesStore.treeStale).toBe(true);
    },
  );
  it("root reads carry generation and request identity without replacing the global index", async () => {
    setFilesStore("tree", {
      workspace_id: "w",
      entries: [{ path: "nested/a", kind: "File", ignored: false }],
      truncated: false,
    });
    ensureDirectory("w");
    await vi.advanceTimersByTimeAsync(100);
    const command = commands[0];
    expect(command).toMatchObject({
      type: "load_file_directory",
      path: "",
      generation: expect.any(Number),
      request_id: expect.any(String),
    });
    directories.accept({
      workspace: "w",
      path: "",
      request_id: command.request_id as string,
      generation: command.generation as number,
      entries: [],
      truncated: false,
    });
    expect(directoryTree()?.entries).toEqual([]);
    expect(filesStore.tree?.entries[0].path).toBe("nested/a");
  });
  it("retains a moved folder's type when a watcher removes its source before confirmation", async () => {
    ensureDirectory("w");
    await vi.advanceTimersByTimeAsync(100);
    const answer = (command: Record<string, unknown>, present: boolean, truncated = false) =>
      directories.accept({
        workspace: "w",
        path: "",
        request_id: command.request_id as string,
        generation: command.generation as number,
        entries: present ? [{ path: "source", kind: "Directory", ignored: false }] : [],
        truncated,
      });
    answer(commands.at(-1)!, true);
    const promise = renamePath("w", "source", "target");
    await Promise.resolve();
    const mutation = commands.at(-1)!;
    directories.changed("w", "source");
    await vi.advanceTimersByTimeAsync(100);
    answer(commands.at(-1)!, false);
    pathOperations.settle({
      operation_id: mutation.operation_id as string,
      workspace: "w",
      kind: "rename",
      from: "source",
      to: "target",
      success: true,
      uncertain: false,
    });
    await promise;
    await vi.advanceTimersByTimeAsync(100);
    answer(commands.at(-1)!, false, true);
    expect(directoryTree()?.entries).toEqual([
      { path: "target", kind: "Directory", ignored: false },
    ]);
  });
  it("invalid paths cannot reach the host", async () => {
    await expect(createPath("w", "../outside", false)).rejects.toThrow();
    await expect(renamePath("w", "a", "a/b")).rejects.toThrow();
    await expect(deletePath("w", "")).rejects.toThrow();
    expect(commands).toEqual([]);
  });
  it("retains search results and one refresh debt when the index changes during a read", async () => {
    setFilesStore("search", {
      workspace_id: "w",
      query: "needle",
      matches: [],
      truncated: false,
    });
    const search = filesStore.search;
    warmFileTree("w");
    const first = commands.at(-1)!;
    invalidateFileIndex("w");
    expect(filesStore.search).toBe(search);
    acceptFileIndex({
      workspace: "w",
      request_id: first.request_id as string,
      tree: { workspace_id: "w", entries: [], truncated: false },
    });
    expect(commands.filter((command) => command.type === "load_file_tree")).toHaveLength(2);
    failFileIndex({ workspace: "w", request_id: first.request_id as string, error: "old" });
    expect(filesStore.treeError).toBeNull();
    const last = commands.at(-1)!;
    acceptFileIndex({
      workspace: "w",
      request_id: last.request_id as string,
      tree: { workspace_id: "w", entries: [], truncated: false },
    });
    expect(filesStore.treeStale).toBe(false);
  });
  it("rejects an index answer after leaving and returning to its workspace", () => {
    warmFileTree("w");
    const first = commands.at(-1)!;
    focusWorkspace("b");
    focusWorkspace("w");
    warmFileTree("w");
    acceptFileIndex({
      workspace: "w",
      request_id: first.request_id as string,
      tree: {
        workspace_id: "w",
        entries: [{ path: "obsolete", kind: "File", ignored: false }],
        truncated: false,
      },
    });
    expect(filesStore.tree).toBeNull();
    expect(loading.tree).toBe(true);
  });
  it("reconciles a timed-out mutation without replaying it on a live connection", async () => {
    const promise = createPath("w", "new/a", false);
    const rejected = expect(promise).rejects.toMatchObject({ uncertain: true });
    await vi.advanceTimersByTimeAsync(OPERATION_TIMEOUT_MS + 100);
    await rejected;
    expect(commands.filter((command) => command.type === "create_path")).toHaveLength(1);
    expect(
      commands.some((command) => command.type === "load_file_directory" && command.path === "new"),
    ).toBe(true);
  });
  it("only content search stamps find-in-files freshness", async () => {
    setFilesStore({ treeVersion: 4, searchReadVersion: 1 });
    await searchFiles("w", "foo", "name");
    expect(filesStore.searchReadVersion).toBe(1);
    await searchFiles("w", "foo", "definition");
    expect(filesStore.searchReadVersion).toBe(1);
    await searchFiles("w", "foo", "content");
    expect(filesStore.searchReadVersion).toBe(4);
  });
  it("stale directory observations cannot restore a deleted file to a fresh index", async () => {
    ensureDirectory("w", "src");
    await vi.advanceTimersByTimeAsync(100);
    const request = commands.at(-1)!;
    directories.accept({
      workspace: "w",
      path: "src",
      request_id: request.request_id as string,
      generation: request.generation as number,
      entries: [{ path: "src/deleted", kind: "File", ignored: false }],
      truncated: false,
    });
    expect(navigationFileIndex()?.entries).toHaveLength(1);
    directories.setInterests("w", []);
    directories.invalidate("w");
    setFilesStore("tree", { workspace_id: "w", entries: [], truncated: false });
    expect(navigationFileIndex()?.entries).toEqual([]);
  });
});

it("keeps confirmed directories visible when a partial parent answer omits them", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("window", {});
  const commands: Record<string, unknown>[] = [];
  mockIPC((name, args) => {
    if (name === "send_workbench_command")
      commands.push((args as { command: Record<string, unknown> }).command);
  });
  focusWorkspace("partial");
  directories.focus("partial");
  directories.connection(true);
  const stopPathMutations = startFileMutationEffects();
  ensureDirectory("partial");
  await vi.advanceTimersByTimeAsync(100);
  const initial = commands[0];
  directories.accept({
    workspace: "partial",
    path: "",
    request_id: initial.request_id as string,
    generation: initial.generation as number,
    entries: [],
    truncated: false,
  });
  const promise = createPath("partial", ".agents", true);
  await Promise.resolve();
  const mutation = commands.at(-1)!;
  pathOperations.settle({
    operation_id: mutation.operation_id as string,
    workspace: "partial",
    kind: "create",
    to: ".agents",
    success: true,
    uncertain: false,
  });
  await promise;
  await vi.advanceTimersByTimeAsync(100);
  const refresh = commands.at(-1)!;
  directories.accept({
    workspace: "partial",
    path: "",
    request_id: refresh.request_id as string,
    generation: refresh.generation as number,
    entries: [],
    truncated: true,
  });
  expect(directoryTree()?.entries).toEqual([
    { path: ".agents", kind: "Directory", ignored: false },
  ]);
  stopPathMutations();
  directories.connection(false);
  directories.focus(null);
  focusWorkspace(null);
  clearMocks();
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
