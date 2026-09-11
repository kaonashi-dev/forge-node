import { describe, expect, it } from "vitest";
import { FLOOR_MS, QUIET_MS, shouldSync } from "./gitSync";

const clock = (over: Partial<Parameters<typeof shouldSync>[0]> = {}) => ({
  now: 100_000,
  lastFrameAt: 100_000 - QUIET_MS,
  lastSyncAt: 100_000 - FLOOR_MS,
  dirty: true,
  ...over,
});

describe("shouldSync", () => {
  it("reads once the terminal has been quiet", () => {
    expect(shouldSync(clock())).toBe(true);
  });

  it("has nothing to ask about when no frame arrived", () => {
    expect(shouldSync(clock({ dirty: false }))).toBe(false);
  });

  // Mid-burst is the wrong moment: the command has not finished, so the
  // branch it may be changing is not the one git would report.
  it("waits for the burst to finish", () => {
    expect(shouldSync(clock({ lastFrameAt: 100_000 - 10 }))).toBe(false);
  });

  it("refuses to ask again inside the floor", () => {
    // A watcher printing on every save alternates output and quiet forever.
    expect(shouldSync(clock({ lastSyncAt: 100_000 - 100 }))).toBe(false);
  });

  it("asks again once the floor has passed", () => {
    expect(shouldSync(clock({ lastSyncAt: 100_000 - FLOOR_MS - 1 }))).toBe(true);
  });
});
