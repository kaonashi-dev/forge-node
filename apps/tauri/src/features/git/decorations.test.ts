import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { reconcile } from "solid-js/store";
import { fileChangesChannel } from "../../runtime/bus";
import { ensureDiff, refreshDiff, startDecorationEffects } from "./decorations";
import { focusWorkspace } from "../../state/workspace";
import { resetFilesAnswers } from "../files/state";
import { gitStore, resetGitAnswers } from "./state";
import { loading, setLoading } from "../../state/loading";

function resetWorkbenchAnswers(): void {
  resetGitAnswers();
  resetFilesAnswers();
  setLoading(reconcile({}));
}

describe("Git decoration refresh", () => {
  let reads: string[];
  let clock = 100_000;
  let failRead = false;
  let stopDecorations: () => void;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(clock);
    clock += 60_000;
    vi.stubGlobal("window", {});
    mockIPC((command, payload) => {
      if (command !== "send_workbench_command") return;
      const request = payload as { command: { type: string; workspace: string } };
      if (request.command.type === "load_diff") {
        reads.push(request.command.workspace);
        if (failRead) throw new Error("diff unavailable");
      }
    });
    failRead = false;
    reads = [];
    focusWorkspace("w1");
    resetWorkbenchAnswers();
    stopDecorations = startDecorationEffects();
  });

  afterEach(() => {
    focusWorkspace(null);
    resetWorkbenchAnswers();
    stopDecorations();
    vi.clearAllTimers();
    vi.useRealTimers();
    clearMocks();
    vi.unstubAllGlobals();
  });

  const changed = (workspace = "w1") => fileChangesChannel.publish([workspace, "src/main.rs"]);

  it("eventually refreshes a change inside the cooldown without needing another event", async () => {
    changed();
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1"]);
    setLoading("diff", false);

    await vi.advanceTimersByTimeAsync(1_000);
    changed();
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1"]);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(reads).toEqual(["w1", "w1"]);
  });

  it("keeps one refresh owed when a file changes during a slow read", async () => {
    ensureDiff();
    changed();
    changed();
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toEqual(["w1"]);

    setLoading("diff", false);
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1", "w1"]);
    setLoading("diff", false);
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toHaveLength(2);
  });

  it("does not postpone the scheduled read indefinitely under continuous writes", async () => {
    changed();
    await vi.advanceTimersByTimeAsync(150);
    setLoading("diff", false);
    for (let second = 0; second < 10; second++) {
      changed();
      await vi.advanceTimersByTimeAsync(1_000);
    }
    expect(reads).toEqual(["w1", "w1"]);
  });

  it("lets an explicit refresh bypass the cooldown and cover the queued external change", async () => {
    changed();
    await vi.advanceTimersByTimeAsync(150);
    setLoading("diff", false);
    changed();
    refreshDiff();
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1", "w1"]);
    setLoading("diff", false);
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toHaveLength(2);
  });

  it("does not carry a cooldown into another checkout", async () => {
    changed();
    await vi.advanceTimersByTimeAsync(150);
    setLoading("diff", false);
    changed();
    focusWorkspace("w2");
    changed("w1");
    changed("w2");
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1", "w2"]);
    setLoading("diff", false);
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toHaveLength(2);
  });

  it("discards a pending explicit refresh when leaving its checkout", async () => {
    refreshDiff();
    focusWorkspace("w2");
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toEqual([]);
  });

  it("cancels a queued read on disposal without needing a workspace change", async () => {
    changed();
    stopDecorations();
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toEqual([]);
    expect(loading.diff).toBeFalsy();

    stopDecorations = startDecorationEffects();
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toEqual([]);
    changed();
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1"]);
  });

  it("discards refresh debt held behind an active read on disposal", async () => {
    ensureDiff();
    changed();
    refreshDiff();
    stopDecorations();
    setLoading("diff", false);
    stopDecorations = startDecorationEffects();
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toEqual(["w1"]);
  });

  it("coalesces explicit and external changes during a read without starting another one", async () => {
    ensureDiff();
    refreshDiff();
    changed();
    refreshDiff();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(reads).toEqual(["w1"]);
    setLoading("diff", false);
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1", "w1"]);
    setLoading("diff", false);
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toHaveLength(2);
  });

  it("stops on a failed read and lets a later change retry", async () => {
    failRead = true;
    ensureDiff();
    await vi.advanceTimersByTimeAsync(0);
    expect(loading.diff).toBe(false);
    expect(gitStore.diffError).toBe("diff unavailable");
    ensureDiff();
    await vi.advanceTimersByTimeAsync(15_000);
    expect(reads).toEqual(["w1"]);
    failRead = false;
    changed();
    await vi.advanceTimersByTimeAsync(150);
    expect(reads).toEqual(["w1", "w1"]);
  });

  it("ignores a late send failure after leaving and reopening the same checkout", async () => {
    let rejectPrevious!: (error: Error) => void;
    const previous = new Promise<void>((_resolve, reject) => {
      rejectPrevious = reject;
    });
    let sent = 0;
    mockIPC(() => (++sent === 1 ? previous : undefined));
    ensureDiff();
    focusWorkspace("w2");
    focusWorkspace("w1");
    ensureDiff();
    rejectPrevious(new Error("old connection closed"));
    await vi.advanceTimersByTimeAsync(0);
    expect(loading.diff).toBe(true);
    expect(gitStore.diffError).toBeNull();
  });
});
