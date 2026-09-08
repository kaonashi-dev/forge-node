import { describe, expect, it } from "vitest";
import { runtimeStore, setRuntimeStore } from "../store/runtimeStore";
import { newWorktreeItem } from "./railMenuItems";

describe("newWorktreeItem", () => {
  it("offers New worktree on a checkout that has a project", () => {
    const item = newWorktreeItem({ id: "p1", name: "forge-node" });
    expect(item).toMatchObject({
      kind: "item",
      label: "New worktree…",
      disabled: false,
    });
  });

  it("stays listed, disabled, when the project is missing", () => {
    const item = newWorktreeItem(undefined);
    expect(item).toMatchObject({ kind: "item", label: "New worktree…", disabled: true });
  });

  it("opens the picker for the named project, not whichever session is active", () => {
    setRuntimeStore("branchPicker", null);
    const item = newWorktreeItem({ id: "p2", name: "other" });
    if (item.kind !== "item") throw new Error("expected an item");
    item.run();
    expect(runtimeStore.branchPicker).toEqual({ id: "p2", name: "other" });
    setRuntimeStore("branchPicker", null);
  });
});
