import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createDirectoryController,
  DIRECTORY_TIMEOUT_MS,
  type DirectoryRequest,
  type DirectorySnapshot,
} from "./directoryController";
import type { FileEntry } from "./types";

const file = (path: string): FileEntry => ({ path, kind: "File", ignored: false });
const folder = (path: string): FileEntry => ({ path, kind: "Directory", ignored: false });

describe("lazy directory reconciliation", () => {
  const disposers: (() => void)[] = [];
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    disposers.splice(0).forEach((dispose) => dispose());
    vi.clearAllTimers();
    vi.useRealTimers();
  });
  function setup() {
    let snapshot!: DirectorySnapshot;
    const controller = createDirectoryController((value) => {
      snapshot = value;
    });
    const requests: DirectoryRequest[] = [];
    controller.configure(async (request) => {
      requests.push(request);
    });
    controller.focus("w");
    disposers.push(controller.dispose);
    const tick = () => vi.advanceTimersByTimeAsync(100);
    const reply = (request: DirectoryRequest, entries: FileEntry[], truncated = false) =>
      controller.accept({ ...request, entries, truncated });
    return {
      controller,
      requests,
      tick,
      reply,
      get state() {
        return snapshot;
      },
    };
  }
  it("loads root and empty directories once without inventing parent metadata", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    h.reply(h.requests[0], [folder("empty")]);
    h.controller.ensure("w", "empty");
    await h.tick();
    h.reply(h.requests[1], []);
    h.controller.ensure("w", "empty");
    await h.tick();
    expect(h.requests).toHaveLength(2);
    expect(h.state.directories.empty).toMatchObject({ status: "loaded", entries: [], error: null });
    expect(h.state.tree?.entries).toEqual([folder("empty")]);
  });
  it("rejects replies across A → B → A, request replacement and reconnect", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    const old = h.requests[0];
    h.controller.focus("b");
    h.controller.focus("w");
    h.controller.ensure("w");
    await h.tick();
    h.reply(old, [file("old")]);
    expect(h.state.tree?.entries).toEqual([]);
    h.controller.connection(false);
    h.controller.connection(true);
    await h.tick();
    h.reply(h.requests[1], [file("stale")]);
    h.reply(h.requests[2], [file("new")]);
    h.reply(old, [file("old")]);
    expect(h.state.tree?.entries).toEqual([file("new")]);
  });
  it("keeps one invalidation debt during a read and a fixed burst deadline", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    for (let i = 0; i < 10; i++) h.controller.changed("w", "a");
    h.reply(h.requests[0], [file("obsolete")]);
    expect(h.state.tree?.entries).toEqual([file("obsolete")]);
    expect(h.state.directories[""].stale).toBe(true);
    await vi.advanceTimersByTimeAsync(50);
    h.controller.changed("w", "b");
    await vi.advanceTimersByTimeAsync(50);
    expect(h.requests).toHaveLength(2);
    h.reply(h.requests[1], [file("a"), file("b")]);
    await h.tick();
    expect(h.requests).toHaveLength(2);
  });
  it("invalidates just the parent and leaves closed folders stale until opened", async () => {
    const h = setup();
    h.controller.setInterests("w", ["", "src", "docs"]);
    await h.tick();
    h.reply(h.requests[0], [folder("src"), folder("docs")]);
    h.reply(h.requests[1], [file("src/a")]);
    h.reply(h.requests[2], []);
    h.controller.setInterests("w", ["", "docs"]);
    h.controller.changed("w", "src/a");
    await h.tick();
    expect(h.requests).toHaveLength(3);
    expect(h.state.directories.src.stale).toBe(true);
    h.controller.ensure("w", "src");
    await h.tick();
    expect(h.requests.at(-1)?.path).toBe("src");
    expect(h.requests).toHaveLength(4);
  });
  it("reconciles a reopened folder even without an event while it was unwatched", async () => {
    const h = setup();
    h.controller.setInterests("w", ["src"]);
    await h.tick();
    h.reply(h.requests[0], [file("src/a")]);
    h.controller.setInterests("w", []);
    h.controller.setInterests("w", ["src"]);
    await h.tick();
    expect(h.requests).toHaveLength(2);
    h.reply(h.requests[1], [file("src/b")]);
    expect(h.state.tree?.entries).toEqual([file("src/b")]);
  });
  it("does not starve displayed observations under continuous writes", async () => {
    const h = setup();
    h.controller.ensure("w");
    for (let i = 0; i < 5; i++) {
      await h.tick();
      h.controller.changed("w", `file-${i}`);
      h.reply(h.requests[i], [file(`file-${i}`)]);
      expect(h.state.tree?.entries.some((entry) => entry.path === `file-${i}`)).toBe(true);
      expect(h.state.directories[""].stale).toBe(true);
    }
  });
  it("retires a parent's pre-mutation read before inserting a confirmed move", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    const old = h.requests[0];
    h.controller.operation({
      operation_id: "op",
      workspace: "w",
      kind: "rename",
      from: "old",
      to: "new",
      success: true,
      uncertain: false,
    });
    h.reply(old, [file("old")]);
    expect(h.state.tree?.entries).toEqual([file("new")]);
  });
  it("drops an in-flight child when its parent confirms deletion", async () => {
    const h = setup();
    h.controller.setInterests("w", ["", "src"]);
    await h.tick();
    h.reply(h.requests[0], []);
    h.reply(h.requests[1], [file("src/zombie")]);
    expect(h.state.directories.src).toBeUndefined();
    expect(h.state.tree?.entries).toEqual([]);
  });
  it("retires ancestor reads that predate confirmed nested creation", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    const old = h.requests[0];
    h.controller.operation({
      operation_id: "nested",
      workspace: "w",
      kind: "create",
      to: "new/deep/a.ts",
      success: true,
      uncertain: false,
    });
    h.reply(old, []);
    expect(h.state.tree?.entries.map((entry) => entry.path)).toContain("new/deep/a.ts");
    await h.tick();
    expect(h.requests.at(-1)?.path).toBe("");
  });
  it("loads uncovered folders beyond the native watch budget", async () => {
    const h = setup();
    const paths = Array.from({ length: 129 }, (_, i) => `d${i}`);
    h.controller.setInterests("w", paths);
    let answered = 0;
    while (answered < paths.length) {
      await h.tick();
      for (const request of h.requests.slice(answered)) {
        h.reply(request, [file(`${request.path}/a`)]);
        answered++;
      }
    }
    expect(h.state.directories.d128.entries).toEqual([file("d128/a")]);
  });
  it("evicts closed observations when entry capacity is needed by a new folder", async () => {
    const h = setup();
    for (let i = 0; i < 11; i++) {
      const path = `d${i}`;
      h.controller.setInterests("w", [path]);
      await h.tick();
      h.reply(
        h.requests.at(-1)!,
        Array.from({ length: 2000 }, (_, n) => file(`${path}/${n}`)),
      );
    }
    expect(h.state.directories.d10.entries).toHaveLength(2000);
    expect(h.state.tree!.entries.length).toBeLessThanOrEqual(20_000);
    h.controller.setInterests("w", ["d0"]);
    await h.tick();
    expect(h.requests.at(-1)?.path).toBe("d0");
  });
  it("retains listing identity through reads and identical answers", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    h.reply(h.requests[0], [file("a")]);
    const tree = h.state.tree;
    h.controller.invalidate("w");
    await h.tick();
    expect(h.state.tree).toBe(tree);
    h.reply(h.requests[1], [file("a")]);
    expect(h.state.tree).toBe(tree);
  });
  it("bounds read timeouts and retries, then permits an explicit reopen", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    await vi.advanceTimersByTimeAsync(3 * DIRECTORY_TIMEOUT_MS + 5000);
    expect(h.requests).toHaveLength(3);
    expect(h.state.directories[""]).toMatchObject({ status: "error", request_id: null });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.requests).toHaveLength(3);
    h.controller.ensure("w");
    await h.tick();
    expect(h.requests).toHaveLength(4);
  });
  it("keeps known and confirmed entries omitted from a partial listing", async () => {
    const h = setup();
    h.controller.ensure("w");
    await h.tick();
    h.reply(h.requests[0], [file("known")]);
    h.controller.operation({
      operation_id: "op",
      workspace: "w",
      kind: "create",
      to: "new",
      success: true,
      uncertain: false,
    });
    await h.tick();
    h.reply(h.requests[1], [file("partial")], true);
    expect(h.state.tree?.entries.map((entry) => entry.path)).toEqual(["known", "new", "partial"]);
    expect(h.state.directories[""].truncated).toBe(true);
    h.controller.invalidate("w");
    await h.tick();
    h.reply(h.requests[2], [file("new")]);
    expect(h.state.tree?.entries).toEqual([file("new")]);
    expect(h.state.tree?.truncated).toBe(false);
  });
  it("cancels moved descendants and reconciles both parents without a root reload", async () => {
    const h = setup();
    h.controller.setInterests("w", ["src", "dest", "src/dir"]);
    await h.tick();
    h.reply(h.requests[0], [folder("src/dir")]);
    h.reply(h.requests[1], []);
    const old = h.requests[2];
    h.controller.operation({
      operation_id: "op",
      workspace: "w",
      kind: "rename",
      from: "src/dir",
      to: "dest/dir",
      success: true,
      uncertain: false,
    });
    h.reply(old, [file("src/dir/zombie")]);
    await h.tick();
    expect(h.state.tree?.entries.map((entry) => entry.path)).toEqual(["dest/dir"]);
    expect(
      h.requests
        .slice(3)
        .map((request) => request.path)
        .sort(),
    ).toEqual(["dest", "src"]);
  });
  it("limits simultaneous reads", async () => {
    const h = setup();
    h.controller.setInterests(
      "w",
      Array.from({ length: 20 }, (_, i) => `dir${i}`),
    );
    await h.tick();
    expect(h.requests).toHaveLength(4);
    h.reply(h.requests[0], []);
    await h.tick();
    expect(h.requests).toHaveLength(5);
  });
  it("prunes a deleted ancestor even when only a deeper directory was cached", async () => {
    const h = setup();
    h.controller.setInterests("w", ["", "src/deep"]);
    await h.tick();
    h.reply(h.requests[0], []);
    h.reply(h.requests[1], [file("src/deep/zombie")]);
    expect(h.state.tree?.entries).toEqual([]);
    expect(h.state.directories["src/deep"]).toBeUndefined();
  });
});
