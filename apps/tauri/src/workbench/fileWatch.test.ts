import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { setRuntimeStore } from "../store/runtimeStore";
import * as watch from "./fileWatch";

describe("file watch reconciliation", () => {
  const cleanup: (() => void)[] = [];

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("window", {});
    mockIPC(() => undefined);
    setRuntimeStore("connection", {
      kind: "connected",
      instanceId: "daemon",
      version: "test",
      editorSurface: "dom",
    });
    watch.reconnectFileWatches();
  });

  afterEach(() => {
    for (const dispose of cleanup.splice(0)) dispose();
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
      watch.fileWatchReady("workspace");
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
    h.fileWatchReady("workspace");
    expect(h.changes).toEqual([""]);
  });
});
