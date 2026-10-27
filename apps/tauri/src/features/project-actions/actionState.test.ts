import { describe, expect, it } from "vitest";
import { sessionFixture } from "../../contracts/sessions.fixture";
import type { ActionRun, ProjectAction, Session } from "../../contracts/runtime";
import {
  browserUrl,
  chipOrder,
  chipState,
  indexActionSessions,
  isActive,
  urlBadge,
} from "./actionState";

function task(
  id: string,
  readiness: ActionRun["readiness"],
  extra: Partial<Session> = {},
): Session {
  return sessionFixture({
    id,
    kind: "Task",
    workspace_id: "ws",
    action: { action_id: "dev", label: "dev", port: 5173, readiness },
    ...extra,
  });
}

function action(id: string, position: number, pinned: boolean): ProjectAction {
  return {
    id,
    project_id: "p",
    label: id,
    command: `bun run ${id}`,
    kind: "custom",
    single_instance: true,
    pinned,
    source: { kind: "manual" },
    position,
    created_at: "2026-01-01T00:00:00Z",
  };
}

describe("action chips", () => {
  it("is idle without a run and keys by workspace and action", () => {
    const index = indexActionSessions([task("a", { state: "unprobed" })]);
    expect(chipState(index, "other-ws", "dev")).toEqual({ phase: "idle" });
    expect(chipState(index, "ws", "lint")).toEqual({ phase: "idle" });
    expect(chipState(index, "ws", "dev")).toEqual({ phase: "running", sessionId: "a" });
  });

  it("prefers the live run over a newer exited one", () => {
    const index = indexActionSessions([
      task(
        "old-live",
        { state: "ready", url: "http://localhost:5173/" },
        { created_at: "2026-01-01T00:00:00Z" },
      ),
      task(
        "new-exited",
        { state: "unprobed" },
        {
          created_at: "2026-01-02T00:00:00Z",
          state: { Exited: { code: 1, signal: null } },
        },
      ),
    ]);
    const state = chipState(index, "ws", "dev");
    expect(state).toEqual({ phase: "ready", sessionId: "old-live", url: "http://localhost:5173/" });
    expect(isActive(state)).toBe(true);
  });

  it("shows the last exit code once nothing is live", () => {
    const index = indexActionSessions([
      task(
        "first",
        { state: "unprobed" },
        {
          created_at: "2026-01-01T00:00:00Z",
          state: { Exited: { code: 0, signal: null } },
        },
      ),
      task(
        "second",
        { state: "unprobed" },
        {
          created_at: "2026-01-02T00:00:00Z",
          state: { Exited: { code: 2, signal: null } },
        },
      ),
    ]);
    const state = chipState(index, "ws", "dev");
    expect(state).toEqual({ phase: "exited", sessionId: "second", code: 2 });
    expect(isActive(state)).toBe(false);
  });

  it("maps readiness, a dependency wait and a failure", () => {
    const one = (session: Session) => chipState(indexActionSessions([session]), "ws", "dev");
    expect(one(task("w", { state: "waiting", on: ["db"] }, { state: "Starting" })).phase).toBe(
      "waiting",
    );
    expect(one(task("t", { state: "timed_out" })).phase).toBe("timed_out");
    expect(one(task("p", { state: "port_in_use", port: 5173 }))).toEqual({
      phase: "port_in_use",
      sessionId: "p",
      port: 5173,
    });
    expect(
      one(
        task(
          "f",
          { state: "waiting", on: [] },
          { state: { Failed: { reason: "db exited with 1" } } },
        ),
      ),
    ).toEqual({
      phase: "failed",
      sessionId: "f",
      reason: "db exited with 1",
    });
  });

  it("ignores untagged sessions", () => {
    const untagged = sessionFixture({ id: "plain", kind: "Task", workspace_id: "ws" });
    expect(indexActionSessions([untagged]).size).toBe(0);
  });

  it("orders pinned chips first, each group by position", () => {
    const { pinned, rest } = chipOrder([
      action("c", 2, false),
      action("b", 1, true),
      action("a", 0, false),
    ]);
    expect(pinned.map((a) => a.id)).toEqual(["b"]);
    expect(rest.map((a) => a.id)).toEqual(["a", "c"]);
  });

  it("shortens a URL to its port", () => {
    expect(urlBadge("http://localhost:5174/", 5173)).toBe(":5174");
    expect(urlBadge("https://app.test/", null)).toBe("app.test");
    expect(urlBadge(null, 3000)).toBe(":3000");
    expect(urlBadge(null, null)).toBeNull();
  });

  it("opens the printed URL, else the port once the server is up", () => {
    expect(browserUrl(task("a", { state: "ready", url: "http://127.0.0.1:5174/" }))).toBe(
      "http://127.0.0.1:5174/",
    );
    expect(browserUrl(task("b", { state: "unprobed" }))).toBe("http://localhost:5173");
    expect(browserUrl(task("c", { state: "timed_out" }))).toBe("http://localhost:5173");
    expect(browserUrl(task("d", { state: "probing" }))).toBeNull();
    expect(browserUrl(task("e", { state: "port_in_use", port: 5173 }))).toBeNull();
    expect(
      browserUrl(
        task("f", { state: "unprobed" }, { state: { Exited: { code: 0, signal: null } } }),
      ),
    ).toBeNull();
  });
});
