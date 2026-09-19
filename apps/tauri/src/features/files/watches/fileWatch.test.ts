import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { setConnectionStore } from "../../../state/connection";
import * as watch from "./fileWatch";

describe("file watch reconciliation", () => {
  let generation = 0;
  const cleanup: (() => void)[] = [];

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("window", {});
    mockIPC((_command, args) => {
      generation = (args as { command: { generation: number } }).command.generation;
    });
    setConnectionStore("connection", {
      kind: "connected",
      instanceId: "daemon",
      version: "test",
      editorSurface: "dom",
    });
    watch.reconnectFileWatches();
  });

  afterEach(() => {
    for (const dispose of cleanup.splice(0)) dispose();
    watch.disconnectFileWatches();
    vi.clearAllTimers();
    vi.useRealTimers();
    clearMocks();
    vi.unstubAllGlobals();
  });

  async function harness() {
    const changes: string[] = [];
    let release: (() => void) | undefined;
    cleanup.push(() => release?.());
    async function folders(paths: string[]) {
      release?.();
      release = watch.watchFiles("workspace", paths, (path) => changes.push(path));
      await vi.advanceTimersByTimeAsync(100);
      watch.fileWatchReady("workspace", generation);
    }
    return { ...watch, changes, folders };
  }

  it("reconciles changes made while a folder was closed after its watch is armed", async () => {
    const h = await harness();
    await h.folders([""]);
    expect(h.changes).toEqual([""]);
    h.changes.length = 0;

    await h.folders(["", "src"]);
    expect(h.changes).toEqual(["src"]);
    h.changes.length = 0;

    await h.folders([""]);
    expect(h.changes).toEqual([]);
    await h.folders(["", "src"]);
    expect(h.changes).toEqual(["src"]);
  });

  it("does not invalidate folders whose watch stayed active", async () => {
    const h = await harness();
    await h.folders(["", "src", "docs"]);
    h.changes.length = 0;
    await h.folders(["", "src"]);
    await h.folders(["src", ""]);
    expect(h.changes).toEqual([]);
  });

  it("reconciles the whole surface after reconnecting", async () => {
    const h = await harness();
    await h.folders(["", "src"]);
    h.changes.length = 0;
    h.reconnectFileWatches();
    await vi.advanceTimersByTimeAsync(100);
    h.fileWatchReady("workspace", generation);
    expect(h.changes).toEqual([""]);
  });
  it("ignores stale acknowledgements and failures across replacements", async () => {
    const changes: string[] = [];
    cleanup.push(watch.watchFiles("workspace", [""], (path) => changes.push(path)));
    await vi.advanceTimersByTimeAsync(100);
    const old = generation;
    cleanup.push(watch.watchFiles("workspace", ["", "src"], () => {}));
    await vi.advanceTimersByTimeAsync(100);
    watch.fileWatchReady("workspace", old);
    watch.failFileWatch("workspace", "old failure", old);
    expect(changes).toEqual([]);
    expect(watch.fileWatchError()).toBeNull();
    watch.fileWatchReady("workspace", generation);
    expect(changes).toEqual([""]);
  });

  it("bounds missing-ACK retries and rearms on an explicit reconnect", async () => {
    cleanup.push(watch.watchFiles("workspace", [""], () => {}));
    await vi.advanceTimersByTimeAsync(100);
    const first = generation;
    await vi.advanceTimersByTimeAsync(4 * watch.WATCH_ACK_TIMEOUT_MS + 5000);
    expect(generation).toBe(first + 3);
    expect(watch.fileWatchError()).toContain("timed out");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(generation).toBe(first + 3);
    watch.reconnectFileWatches();
    await vi.advanceTimersByTimeAsync(100);
    watch.fileWatchReady("workspace", generation);
    expect(watch.fileWatchError()).toBeNull();
  });

  it("keeps partial coverage warnings after successful ACKs", async () => {
    cleanup.push(
      watch.watchFiles(
        "workspace",
        Array.from({ length: 140 }, (_, i) => `d${i}`),
        () => {},
      ),
    );
    await vi.advanceTimersByTimeAsync(100);
    watch.fileWatchReady("workspace", generation);
    expect(watch.fileWatchError()).toContain("128");
  });
  it("reports coverage changes even when the installed set is unchanged", async () => {
    const h = await harness();
    const paths = Array.from({ length: 128 }, (_, i) => `a${i}`);
    await h.folders(paths);
    const installed = generation;
    await h.folders([...paths, "z"]);
    expect(generation).toBe(installed);
    expect(watch.fileWatchError()).toContain("128");
    await h.folders(paths);
    expect(watch.fileWatchError()).toBeNull();
  });
  it("reconciles an interrupted watch even when its last acknowledged set returns", async () => {
    const changes: string[] = [];
    let release = watch.watchFiles("workspace", ["", "a"], (path) => changes.push(path));
    cleanup.push(() => release());
    await vi.advanceTimersByTimeAsync(100);
    watch.fileWatchReady("workspace", generation);
    changes.length = 0;
    release();
    release = watch.watchFiles("workspace", ["", "b"], (path) => changes.push(path));
    await vi.advanceTimersByTimeAsync(100);
    const superseded = generation;
    release();
    release = watch.watchFiles("workspace", ["", "a"], (path) => changes.push(path));
    await vi.advanceTimersByTimeAsync(100);
    watch.fileWatchReady("workspace", superseded);
    watch.fileWatchReady("workspace", generation);
    expect(changes).toEqual([""]);
  });

  it("an explicit refresh rearms watches after automatic retries are exhausted", async () => {
    cleanup.push(watch.watchFiles("workspace", [""], () => {}));
    await vi.advanceTimersByTimeAsync(4 * watch.WATCH_ACK_TIMEOUT_MS + 5000);
    const exhausted = generation;
    watch.rearmFileWatches("workspace", "");
    await vi.advanceTimersByTimeAsync(100);
    expect(generation).toBeGreaterThan(exhausted);
    watch.fileWatchReady("workspace", generation);
    expect(watch.fileWatchError()).toBeNull();
  });

  it("rearms a deleted and recreated watched directory with unchanged interests", async () => {
    const h = await harness();
    await h.folders(["", "src"]);
    const old = generation;
    h.changes.length = 0;
    watch.rearmFileWatches("workspace", "src");
    await vi.advanceTimersByTimeAsync(100);
    expect(generation).toBeGreaterThan(old);
    watch.fileWatchReady("workspace", old);
    expect(h.changes).toEqual([]);
    watch.fileWatchReady("workspace", generation);
    expect(h.changes).toEqual([""]);
  });
});
