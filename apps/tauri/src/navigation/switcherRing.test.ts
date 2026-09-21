import { beforeEach, describe, expect, it } from "vitest";
import { setConnectionStore } from "../state/connection";
import { setActiveWorkspace } from "../state/workspace";
import { activeSwitcherKey, switcherTargets } from "./switcherRing";
import { openEditorTerminal, showSession, openDiff } from "./viewsStore";

let workspace = "";

beforeEach(() => {
  workspace = `ring-${Math.random().toString(36).slice(2)}`;
  setActiveWorkspace(workspace);
  setConnectionStore("activeSession", "t1");
  showSession();
});

describe("switcher ring", () => {
  it("offers the Code tab while nothing is parked in it, as the strip does", () => {
    expect(switcherTargets(["t1"])).toEqual([
      { kind: "code", workspace },
      { kind: "session", id: "t1" },
    ]);
    expect(activeSwitcherKey()).toBe("session:t1");
  });

  it("expands Code into one row per open view once there are any", () => {
    openEditorTerminal("e1", "src/main.ts");
    openDiff();

    expect(switcherTargets(["t1"])).toEqual([
      {
        kind: "view",
        workspace,
        view: { kind: "editor-terminal", session: "e1", path: "src/main.ts" },
      },
      { kind: "view", workspace, view: { kind: "diff" } },
      { kind: "session", id: "t1" },
    ]);
    // Opening raised Code, so the diff is the pane a step has to move away from.
    expect(activeSwitcherKey()).toBe(`view:${workspace}:diff`);
  });

  it("names the terminal once the centre column goes back to it", () => {
    openEditorTerminal("e1", "src/main.ts");
    showSession();

    expect(activeSwitcherKey()).toBe("session:t1");
  });

  it("has no Code row without a checkout to park views in", () => {
    setActiveWorkspace(null);

    expect(switcherTargets(["t1"])).toEqual([{ kind: "session", id: "t1" }]);
    expect(activeSwitcherKey()).toBe("session:t1");
  });
});
