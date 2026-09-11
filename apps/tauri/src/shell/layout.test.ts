import { createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { applyShellSnapshot, emptySnapshot } from "../store/forgeStore";
import { SIDEBAR_WIDTH_KEY, SIDEBAR_RANGE, readWidth, seedFromAppState } from "./layout";

/** Land a snapshot the way `applyConnected` does, and let the effects run. */
async function snapshotWith(app_state: Record<string, string>): Promise<void> {
  applyShellSnapshot({ ...emptySnapshot(), app_state });
  await Promise.resolve();
}

describe("seedFromAppState", () => {
  it("waits for the snapshot, then seeds once", async () => {
    await snapshotWith({});
    const seen: number[] = [];

    const dispose = createRoot((dispose) => {
      // What a component does: read at creation, when `app_state` is still
      // empty, and register the correction.
      seen.push(readWidth(SIDEBAR_WIDTH_KEY, SIDEBAR_RANGE));
      seedFromAppState(() => seen.push(readWidth(SIDEBAR_WIDTH_KEY, SIDEBAR_RANGE)));
      return dispose;
    });

    // Nothing stored yet: the fallback, and no seed.
    expect(seen).toEqual([SIDEBAR_RANGE.fallback]);

    await snapshotWith({ [SIDEBAR_WIDTH_KEY]: "420" });
    expect(seen).toEqual([SIDEBAR_RANGE.fallback, 420]);

    // A later write must not yank a panel the person has since dragged.
    await snapshotWith({ [SIDEBAR_WIDTH_KEY]: "300" });
    expect(seen).toEqual([SIDEBAR_RANGE.fallback, 420]);

    dispose();
    await snapshotWith({});
  });

  it("never fires on a fresh install, where there is nothing to seed", async () => {
    await snapshotWith({});
    let seeded = 0;
    const dispose = createRoot((dispose) => {
      seedFromAppState(() => (seeded += 1));
      return dispose;
    });
    await Promise.resolve();
    expect(seeded).toBe(0);
    dispose();
  });
});
