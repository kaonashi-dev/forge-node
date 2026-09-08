import { describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { Session } from "../runtime/types";
import {
  dropFromGap,
  gapAtX,
  gapAtY,
  moveTabToGap,
  openSessions,
  orderFromSessions,
} from "./tabOrder";

const session = (id: string, created_at?: string): Session =>
  sessionFixture({ id, ...(created_at ? { created_at } : {}) });

describe("tabOrder", () => {
  it("moves a tab to every gap, including after the last tab", () => {
    const order = ["a", "b", "c"];
    expect(moveTabToGap(order, "c", 0)).toEqual(["c", "a", "b"]);
    expect(moveTabToGap(order, "a", 2)).toEqual(["b", "a", "c"]);
    expect(moveTabToGap(order, "a", 3)).toEqual(["b", "c", "a"]);
  });

  it("leaves invalid or same-position moves unchanged", () => {
    expect(moveTabToGap(["a", "b"], "missing", 0)).toEqual(["a", "b"]);
    expect(moveTabToGap(["a", "b"], "a", 1)).toEqual(["a", "b"]);
  });

  it("orders one checkout's tabs without reaching into another's", () => {
    // The strip is handed the checkout's sessions and the checkout's order;
    // an id belonging to a sibling worktree must not pull a tab in or push
    // one out.
    const checkout = [
      session("dev-2", "2026-01-02T00:00:00Z"),
      session("dev-1", "2026-01-01T00:00:00Z"),
    ];
    expect(openSessions(checkout, ["dev-2", "pay-9", "dev-1"]).map((item) => item.id)).toEqual([
      "dev-2",
      "dev-1",
    ]);
    expect(openSessions(checkout, []).map((item) => item.id)).toEqual(["dev-1", "dev-2"]);
  });

  it("applies the stored order and appends new sessions", () => {
    const sessions = [session("a"), session("b"), session("c")];
    expect(openSessions(sessions, ["c", "stale", "a"]).map((item) => item.id)).toEqual([
      "c",
      "a",
      "b",
    ]);
    expect(orderFromSessions(sessions, ["c", "stale", "a"])).toEqual(["c", "a", "b"]);
  });

  it("falls back to creation order when nothing is stored", () => {
    const sessions = [
      session("c", "2026-01-03T00:00:00Z"),
      session("a", "2026-01-01T00:00:00Z"),
      session("b", "2026-01-02T00:00:00Z"),
    ];
    expect(openSessions(sessions, []).map((item) => item.id)).toEqual(["a", "b", "c"]);
    expect(orderFromSessions(sessions, [])).toEqual(["a", "b", "c"]);
  });

  it("names the gap a pointer is in from tab boxes", () => {
    const tabs = [
      { left: 0, width: 100 },
      { left: 100, width: 100 },
      { left: 200, width: 100 },
    ];
    expect(gapAtX(tabs, -10)).toBe(0);
    expect(gapAtX(tabs, 49)).toBe(0);
    expect(gapAtX(tabs, 50)).toBe(1);
    expect(gapAtX(tabs, 150)).toBe(2);
    expect(gapAtX(tabs, 250)).toBe(3);
  });

  it("names the gap a pointer is in from stacked boxes", () => {
    const items = [
      { top: 0, height: 80 },
      { top: 80, height: 80 },
      { top: 160, height: 80 },
    ];
    expect(gapAtY(items, -4)).toBe(0);
    expect(gapAtY(items, 39)).toBe(0);
    expect(gapAtY(items, 40)).toBe(1);
    expect(gapAtY(items, 200)).toBe(3);
  });

  it("hides the drop mark when the pointer is still on the dragged item", () => {
    expect(dropFromGap(["a", "b", "c"], 1, "a")).toBeNull();
    expect(dropFromGap(["a", "b", "c"], 0, "b")).toEqual({ id: "a", after: false });
    expect(dropFromGap(["a", "b", "c"], 3, "a")).toEqual({ id: "c", after: true });
  });
});
