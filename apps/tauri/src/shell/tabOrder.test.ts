import { describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { Session } from "../runtime/types";
import {
  clampGapToKind,
  dropFromGap,
  gapAtX,
  gapAtY,
  moveTabToGap,
  openSessions,
  orderFromSessions,
  stripIndex,
  stripItems,
} from "./tabOrder";

const session = (id: string, created_at?: string): Session =>
  sessionFixture({ id, ...(created_at ? { created_at } : {}) });

const agent = (id: string, created_at?: string): Session =>
  sessionFixture({
    id,
    kind: "Agent",
    agent_provider_id: "claude",
    ...(created_at ? { created_at } : {}),
  });

const editor = (id: string, created_at?: string): Session =>
  sessionFixture({
    id,
    kind: "Editor",
    ...(created_at ? { created_at } : {}),
  });

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

  it("groups shells before agents, keeping relative order inside each group", () => {
    const sessions = [
      session("t1", "2026-01-01T00:00:00Z"),
      agent("a1", "2026-01-02T00:00:00Z"),
      session("t2", "2026-01-03T00:00:00Z"),
      agent("a2", "2026-01-04T00:00:00Z"),
    ];
    expect(openSessions(sessions, []).map((item) => item.id)).toEqual(["t1", "t2", "a1", "a2"]);
    expect(openSessions(sessions, ["a1", "t2", "t1", "a2"]).map((item) => item.id)).toEqual([
      "t2",
      "t1",
      "a1",
      "a2",
    ]);
  });

  it("places a new shell after the existing shells, before agents", () => {
    // Code, terminal 1, three agents, then another terminal → that terminal
    // becomes third, not last.
    const sessions = [
      session("t1", "2026-01-01T00:00:00Z"),
      agent("a1", "2026-01-02T00:00:00Z"),
      agent("a2", "2026-01-03T00:00:00Z"),
      agent("a3", "2026-01-04T00:00:00Z"),
      session("t2", "2026-01-05T00:00:00Z"),
    ];
    expect(openSessions(sessions, ["t1", "a1", "a2", "a3"]).map((item) => item.id)).toEqual([
      "t1",
      "t2",
      "a1",
      "a2",
      "a3",
    ]);
  });

  it("does not count an editor as a shell", () => {
    // An editor should never reach the strip, but if one is handed in it must
    // not be numbered or ordered as a terminal: it sorts with the non-shells,
    // so it cannot sit between two terminals.
    const sessions = [
      session("t1", "2026-01-01T00:00:00Z"),
      editor("e1", "2026-01-02T00:00:00Z"),
      session("t2", "2026-01-03T00:00:00Z"),
    ];
    expect(openSessions(sessions, []).map((item) => item.id)).toEqual(["t1", "t2", "e1"]);
  });

  it("places a new agent at the end", () => {
    const sessions = [
      session("t1", "2026-01-01T00:00:00Z"),
      agent("a1", "2026-01-02T00:00:00Z"),
      agent("a2", "2026-01-03T00:00:00Z"),
    ];
    expect(openSessions(sessions, ["t1", "a1"]).map((item) => item.id)).toEqual(["t1", "a1", "a2"]);
  });

  it("puts Code first on the strip so Option+1 lands on it", () => {
    const sessions = [session("t1"), agent("a1")];
    expect(stripItems(sessions, true)).toEqual([
      { kind: "code" },
      { kind: "session", id: "t1" },
      { kind: "session", id: "a1" },
    ]);
    expect(stripItems(sessions, false)).toEqual([
      { kind: "session", id: "t1" },
      { kind: "session", id: "a1" },
    ]);
  });

  it("names the current strip tab from Code vs the active session", () => {
    const items = stripItems([session("t1"), agent("a1")], true);
    expect(stripIndex(items, true, "t1")).toBe(0);
    expect(stripIndex(items, false, "t1")).toBe(1);
    expect(stripIndex(items, false, "a1")).toBe(2);
    expect(stripIndex(items, false, null)).toBe(-1);
  });

  it("keeps a shell drag inside the shell group", () => {
    const sessions = [session("t1"), session("t2"), agent("a1"), agent("a2")];
    expect(clampGapToKind(sessions, "t1", 3)).toBe(2);
    expect(clampGapToKind(sessions, "t2", 0)).toBe(0);
    expect(clampGapToKind(sessions, "a1", 0)).toBe(2);
    expect(clampGapToKind(sessions, "a2", 4)).toBe(4);
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
