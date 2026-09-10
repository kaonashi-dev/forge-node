import { createComputed, createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { ShellSnapshot } from "../runtime/types";
import { applyRailTree } from "./railTree";
import { buildTree, type GroupNode } from "./tree";

const checkout = {
  id: "w",
  project_id: "p",
  kind: "Main" as const,
  path: "/r",
  branch: "main",
  display_name: null,
  managed_by_app: true,
  status: { dirty: false, ahead: null, behind: null, measured_at: null },
};

function agent(id: string, title: string) {
  return sessionFixture({
    id,
    workspace_id: "w",
    kind: "Agent",
    agent_provider_id: "claude",
    title: { user: null, terminal: title },
  });
}

const snapshot = (...sessions: ReturnType<typeof agent>[]): ShellSnapshot => ({
  ...emptySnapshot(),
  projects: [{ id: "p", project_group_id: null, name: "forge", icon: null, root_path: "/r" }],
  workspaces: [checkout],
  sessions,
});

describe("applyRailTree", () => {
  it("keeps the identity of a card whose OSC title moved", () => {
    const [tree, setTree] = createStore<GroupNode[]>([]);
    applyRailTree(setTree, buildTree(snapshot(agent("s", "Read a.ts"))));
    const card = tree[0].projects[0].workspaces[0];
    const row = card.sessions[0];

    applyRailTree(setTree, buildTree(snapshot(agent("s", "Edit a.ts"))));

    expect(tree[0].projects[0].workspaces[0]).toBe(card);
    expect(tree[0].projects[0].workspaces[0].sessions[0]).toBe(row);
  });

  it("keeps each session row when a sibling's title moves", () => {
    const [tree, setTree] = createStore<GroupNode[]>([]);
    applyRailTree(setTree, buildTree(snapshot(agent("a", "one"), agent("b", "two"))));
    const [first, second] = tree[0].projects[0].workspaces[0].sessions;

    applyRailTree(setTree, buildTree(snapshot(agent("a", "one"), agent("b", "changed"))));

    const rows = tree[0].projects[0].workspaces[0].sessions;
    expect(rows[0]).toBe(first);
    expect(rows[1]).toBe(second);
    expect(rows[1].id).toBe("b");
  });

  it("does not give a new session the existing row's identity", () => {
    const [tree, setTree] = createStore<GroupNode[]>([]);
    applyRailTree(setTree, buildTree(snapshot(agent("a", "one"))));
    const existing = tree[0].projects[0].workspaces[0].sessions[0];

    applyRailTree(setTree, buildTree(snapshot(agent("b", "new"), agent("a", "one"))));

    const rows = tree[0].projects[0].workspaces[0].sessions;
    expect(rows.find((row) => row.id === "a")).toBe(existing);
    expect(rows.find((row) => row.id === "b")).not.toBe(existing);
  });

  it("notifies nobody when the rail did not change", () => {
    const [tree, setTree] = createStore<GroupNode[]>([]);
    applyRailTree(setTree, buildTree(snapshot(agent("s", "one"))));
    let runs = 0;
    const dispose = createRoot((dispose) => {
      createComputed(() => {
        tree[0].projects[0].workspaces[0].sessions[0].label;
        runs += 1;
      });
      return dispose;
    });
    expect(runs).toBe(1);

    applyRailTree(setTree, buildTree(snapshot(agent("s", "one"))));
    expect(runs).toBe(1);
    dispose();
  });
});
