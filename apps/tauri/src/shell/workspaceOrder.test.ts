import { describe, expect, it } from "vitest";
import {
  defaultWorkspaceSort,
  orderFromWorkspaces,
  orderWorkspaces,
  parseWorkspaceOrder,
  type OrderableCheckout,
} from "./workspaceOrder";

const checkout = (id: string, extra: Partial<OrderableCheckout> = {}): OrderableCheckout => ({
  id,
  label: extra.label ?? id,
  worktree: extra.worktree ?? true,
});

describe("workspaceOrder", () => {
  it("leads with the repository checkout, then worktrees by name", () => {
    const workspaces = [
      checkout("z", { label: "zeta", worktree: true }),
      checkout("m", { label: "main", worktree: false }),
      checkout("a", { label: "alpha", worktree: true }),
    ];
    expect(defaultWorkspaceSort(workspaces).map((item) => item.id)).toEqual(["m", "a", "z"]);
    expect(orderWorkspaces(workspaces, []).map((item) => item.id)).toEqual(["m", "a", "z"]);
  });

  it("honours a stored order, including a worktree above primary", () => {
    const workspaces = [
      checkout("m", { label: "main", worktree: false }),
      checkout("t", { label: "test", worktree: true }),
      checkout("f", { label: "feat", worktree: true }),
    ];
    expect(orderWorkspaces(workspaces, ["t", "m", "f"]).map((item) => item.id)).toEqual([
      "t",
      "m",
      "f",
    ]);
  });

  it("appends new checkouts in the default sort and drops stale ids", () => {
    const workspaces = [
      checkout("m", { label: "main", worktree: false }),
      checkout("t", { label: "test", worktree: true }),
      checkout("n", { label: "new", worktree: true }),
    ];
    expect(orderFromWorkspaces(workspaces, ["t", "gone", "m"])).toEqual(["t", "m", "n"]);
  });

  it("reads a per-project map and ignores junk", () => {
    expect(parseWorkspaceOrder(undefined)).toEqual({});
    expect(parseWorkspaceOrder("{")).toEqual({});
    expect(parseWorkspaceOrder('["a"]')).toEqual({});
    expect(parseWorkspaceOrder('{"p":["w1",2,"w2"],"q":"nope"}')).toEqual({ p: ["w1", "w2"] });
  });
});
