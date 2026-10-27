import { describe, expect, it } from "vitest";
import { sessionFixture } from "../../contracts/sessions.fixture";
import type { ProjectAction } from "../../contracts/runtime";
import { actionMenuItems } from "./actionMenu";
import { indexActionSessions } from "./actionState";

function action(id: string, position: number): ProjectAction {
  return {
    id,
    project_id: "p",
    label: id,
    command: `bun run ${id}`,
    kind: "custom",
    single_instance: true,
    pinned: false,
    source: { kind: "manual" },
    position,
    created_at: "2026-01-01T00:00:00Z",
  };
}

const labels = (items: ReturnType<typeof actionMenuItems>) =>
  items.map((item) => (item.kind === "item" ? item.label : item.kind));

describe("the run menu", () => {
  it("runs idle actions and shows, opens or stops the live one in this checkout", () => {
    const index = indexActionSessions([
      sessionFixture({
        id: "s",
        kind: "Task",
        workspace_id: "ws",
        action: {
          action_id: "dev",
          label: "dev",
          port: 5173,
          readiness: { state: "ready", url: "http://localhost:5173/" },
        },
      }),
    ]);
    const items = actionMenuItems(
      "ws",
      { id: "p", name: "api" },
      [action("test", 1), action("dev", 0)],
      index,
    );
    expect(labels(items)).toEqual([
      "Show dev",
      "Open http://localhost:5173/",
      "Stop dev",
      "Run test",
      "rule",
      "Edit actions…",
    ]);
    const elsewhere = actionMenuItems("other", { id: "p", name: "api" }, [action("dev", 0)], index);
    expect(labels(elsewhere)).toEqual(["Run dev", "rule", "Edit actions…"]);
  });

  it("offers setup when the project has no actions", () => {
    expect(labels(actionMenuItems("ws", { id: "p", name: "api" }, [], new Map()))).toEqual([
      "Set up actions…",
    ]);
  });
});
