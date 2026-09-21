import { describe, expect, it } from "vitest";
import { activeTargetKey, localTargets, targetKey } from "./tabTargets";
import { TERMINAL_VIEW } from "./views";

describe("tabTargets", () => {
  it("keys a view by its checkout, so two checkouts' diffs are two rows", () => {
    const view = { kind: "diff" } as const;
    expect(targetKey({ kind: "view", workspace: "one", view })).not.toBe(
      targetKey({ kind: "view", workspace: "two", view }),
    );
    expect(targetKey({ kind: "session", id: "t1" })).toBe("session:t1");
  });

  it("keeps one Code row while nothing is parked in it, as the strip does", () => {
    expect(localTargets("w1", [], ["t1"])).toEqual([
      { kind: "code", workspace: "w1" },
      { kind: "session", id: "t1" },
    ]);
    expect(activeTargetKey(true, "w1", [], TERMINAL_VIEW, "t1")).toBe("code:w1");
  });

  it("lists the Code views ahead of the terminals, as the strip does", () => {
    expect(
      localTargets("w1", [{ kind: "editor-terminal", session: "e1", path: "src/main.ts" }], ["t1"]),
    ).toEqual([
      {
        kind: "view",
        workspace: "w1",
        view: { kind: "editor-terminal", session: "e1", path: "src/main.ts" },
      },
      { kind: "session", id: "t1" },
    ]);
  });

  it("offers no view rows without a checkout to park them in", () => {
    expect(localTargets(null, [{ kind: "diff" }], ["t1"])).toEqual([{ kind: "session", id: "t1" }]);
  });

  it("names the pane on screen, terminal or Code view", () => {
    const open = [{ kind: "search" } as const];
    expect(activeTargetKey(true, "w1", open, { kind: "search" }, "t1")).toBe("view:w1:search");
    // The Code tab showing the terminal view is the terminal: one pane, one row.
    expect(activeTargetKey(true, "w1", open, TERMINAL_VIEW, "t1")).toBe("session:t1");
    expect(activeTargetKey(false, "w1", open, { kind: "search" }, "t1")).toBe("session:t1");
    expect(activeTargetKey(false, "w1", [], TERMINAL_VIEW, null)).toBeNull();
  });
});
