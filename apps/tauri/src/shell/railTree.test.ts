import { createComputed, createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { ShellSnapshot } from "../runtime/types";
import { applyRailTree } from "./railTree";
import { buildTree, type GroupNode } from "./tree";

const snapshot = (title: string): ShellSnapshot => ({
  ...emptySnapshot(),
  projects: [{ id: "p", project_group_id: null, name: "forge", icon: null, root_path: "/r" }],
  workspaces: [
    {
      id: "w",
      project_id: "p",
      kind: "Main",
      path: "/r",
      branch: "main",
      display_name: null,
      managed_by_app: true,
      status: { dirty: false, ahead: null, behind: null, measured_at: null },
    },
  ],
  sessions: [
    sessionFixture({
      id: "s",
      workspace_id: "w",
      kind: "Agent",
      agent_provider_id: "claude",
      title: { user: null, terminal: title },
    }),
  ],
});

describe("applyRailTree", () => {
  it("keeps the identity of a card whose OSC title moved", () => {
    const [tree, setTree] = createStore<GroupNode[]>([]);
    applyRailTree(setTree, buildTree(snapshot("Read a.ts")));
    const card = tree[0].projects[0].workspaces[0];
    const row = card.sessions[0];

    applyRailTree(setTree, buildTree(snapshot("Edit a.ts")));

    expect(tree[0].projects[0].workspaces[0]).toBe(card);
    expect(tree[0].projects[0].workspaces[0].sessions[0]).toBe(row);
  });

  it("notifies nobody when the rail did not change", () => {
    const [tree, setTree] = createStore<GroupNode[]>([]);
    applyRailTree(setTree, buildTree(snapshot("one")));
    let runs = 0;
    const dispose = createRoot((dispose) => {
      createComputed(() => {
        tree[0].projects[0].workspaces[0].sessions[0].label;
        runs += 1;
      });
      return dispose;
    });
    expect(runs).toBe(1);

    // The same rail again: `buildTree` allocates new wrappers either way, so
    // this is what proves the reconcile compares contents and not identity.
    applyRailTree(setTree, buildTree(snapshot("one")));
    expect(runs).toBe(1);
    dispose();
  });
});
