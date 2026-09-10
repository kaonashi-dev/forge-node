import { describe, expect, it } from "vitest";
import {
  initialIndex,
  liveIdsByActivity,
  mruIds,
  stepIndex,
  switcherIds,
  touchMru,
} from "./tabMru";

describe("tabMru", () => {
  it("moves the focused id to the front without duplicating it", () => {
    expect(touchMru(["a", "b", "c"], "b")).toEqual(["b", "a", "c"]);
    expect(touchMru(["a", "b"], "a")).toEqual(["a", "b"]);
    expect(touchMru([], "x")).toEqual(["x"]);
  });

  it("lists the active tab first, then the ring, then strip leftovers", () => {
    expect(mruIds(["c", "a", "b"], ["a", "b", "c", "d"], "b")).toEqual(["b", "c", "a", "d"]);
    expect(mruIds(["z"], ["a", "b"], null)).toEqual(["a", "b"]);
    expect(mruIds(["b", "a"], ["a", "b"], "a")).toEqual(["a", "b"]);
  });

  it("drops history ids that are no longer open", () => {
    expect(mruIds(["gone", "b"], ["a", "b"], "a")).toEqual(["a", "b"]);
  });

  it("appends up to three recent sessions from other checkouts", () => {
    expect(
      switcherIds(
        ["local-a", "other-1", "other-2", "other-3", "other-4"],
        ["local-a", "local-b"],
        "local-a",
        ["local-a", "local-b", "other-1", "other-2", "other-3", "other-4"],
      ),
    ).toEqual({
      ids: ["local-a", "local-b", "other-1", "other-2", "other-3"],
      foreignAt: 2,
    });
  });

  it("fills foreign rows from live order when history has none", () => {
    expect(switcherIds([], ["a"], "a", ["a", "x", "y", "z"], 2)).toEqual({
      ids: ["a", "x", "y"],
      foreignAt: 1,
    });
  });

  it("orders live ids by activity, newest first", () => {
    expect(
      liveIdsByActivity([
        { id: "old", terminal_id: "t1", created_at: "2026-01-01T00:00:00Z" },
        {
          id: "new",
          terminal_id: "t2",
          last_activity_at: "2026-06-01T00:00:00Z",
          created_at: "2026-01-02T00:00:00Z",
        },
        { id: "dead", terminal_id: null, created_at: "2026-07-01T00:00:00Z" },
      ]),
    ).toEqual(["new", "old"]);
  });

  it("starts one step away from the current tab", () => {
    expect(initialIndex(1, 1)).toBe(0);
    expect(initialIndex(4, 1)).toBe(1);
    expect(initialIndex(4, -1)).toBe(3);
  });

  it("wraps the cursor in both directions", () => {
    expect(stepIndex(0, 3, -1)).toBe(2);
    expect(stepIndex(2, 3, 1)).toBe(0);
    expect(stepIndex(1, 3, 1)).toBe(2);
  });
});
