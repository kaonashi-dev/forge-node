import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createOperationTracker,
  OPERATION_TIMEOUT_MS,
  PathOperationError,
  type PathOperationResult,
} from "./operations";

describe("path operation confirmation", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
  });
  function setup() {
    const tracker = createOperationTracker();
    const sent: string[] = [];
    const send = vi.fn(async (id: string) => {
      sent.push(id);
    });
    const descriptor = { workspace: "w", kind: "rename" as const, from: "src", to: "lib" };
    const successes: PathOperationResult[] = [];
    tracker.subscribe((result) => successes.push(result));
    const result = (over: Partial<PathOperationResult> = {}): PathOperationResult => ({
      ...descriptor,
      operation_id: sent[0],
      success: true,
      uncertain: false,
      ...over,
    });
    return { tracker, sent, send, descriptor, successes, result };
  }
  it("waits for its own explicit result and deduplicates double Enter", async () => {
    const h = setup();
    const promise = h.tracker.run(h.descriptor, h.send);
    expect(h.tracker.run(h.descriptor, h.send)).toBe(promise);
    await Promise.resolve();
    expect(h.send).toHaveBeenCalledTimes(1);
    h.tracker.settle(h.result({ workspace: "other" }));
    h.tracker.settle(h.result({ from: "other" }));
    expect(h.successes).toEqual([]);
    h.tracker.settle(h.result());
    await expect(promise).resolves.toMatchObject({ success: true });
    h.tracker.settle(h.result());
    expect(h.successes).toHaveLength(1);
  });
  it("rejects queue failure definitively without waiting for a listing", async () => {
    const h = setup();
    const promise = h.tracker.run(h.descriptor, async () => {
      throw new Error("Queue full");
    });
    await expect(promise).rejects.toMatchObject({ uncertain: false, message: "Queue full" });
    expect(h.successes).toEqual([]);
  });
  it("settles timeout as uncertain and accepts a late confirmation exactly once", async () => {
    const h = setup();
    const promise = h.tracker.run(h.descriptor, h.send);
    const rejected = expect(promise).rejects.toMatchObject({ uncertain: true });
    await vi.advanceTimersByTimeAsync(OPERATION_TIMEOUT_MS);
    await rejected;
    const reads = vi.fn();
    h.tracker.reconcile(reads, vi.fn());
    expect(reads).toHaveBeenCalledWith(expect.objectContaining(h.descriptor));
    expect(h.send).toHaveBeenCalledTimes(1);
    h.tracker.settle(h.result());
    h.tracker.settle(h.result());
    expect(h.successes).toHaveLength(1);
  });
  it("disconnect never retries writes and late failures do not retarget", async () => {
    const h = setup();
    const promise = h.tracker.run(h.descriptor, h.send);
    const rejected = expect(promise).rejects.toBeInstanceOf(PathOperationError);
    await Promise.resolve();
    h.tracker.disconnect();
    await rejected;
    h.tracker.settle(h.result({ success: false, error: "Refused" }));
    h.tracker.reconcile(vi.fn(), vi.fn());
    expect(h.successes).toEqual([]);
    expect(h.send).toHaveBeenCalledTimes(1);
  });
  it("successful write remains complete when a later directory read fails", async () => {
    const h = setup();
    const promise = h.tracker.run(h.descriptor, h.send);
    await Promise.resolve();
    h.tracker.settle(h.result());
    h.tracker.disconnect();
    await expect(promise).resolves.toMatchObject({ success: true, uncertain: false });
  });
  it("does not send a command after a disconnect before enqueue", async () => {
    const h = setup();
    const promise = h.tracker.run(h.descriptor, h.send);
    const rejected = expect(promise).rejects.toMatchObject({ uncertain: false });
    h.tracker.disconnect();
    await rejected;
    expect(h.send).not.toHaveBeenCalled();
  });
});
