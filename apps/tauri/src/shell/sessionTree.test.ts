import { describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { Session } from "../runtime/types";
import {
  harnessFeatureAgents,
  harnessIsolationNote,
  orderSessionsTree,
  sessionDisplayTitle,
} from "./sessionTree";

function session(id: string, extra: Partial<Session> = {}): Session {
  return sessionFixture({
    id,
    root_session_id: extra.parent_session_id ? "root" : id,
    created_at: `2026-01-01T00:00:0${id.replace(/\D/g, "") || "0"}Z`,
    ...extra,
  });
}

describe("orderSessionsTree", () => {
  it("puts each root's descendants under it, depth-first", () => {
    const rows = orderSessionsTree([
      session("c1", { parent_session_id: "root" }),
      session("root"),
      session("c2", { parent_session_id: "root" }),
      session("g1", { parent_session_id: "c1" }),
    ]);
    expect(rows.map((row) => [row.session.id, row.depth])).toEqual([
      ["root", 0],
      ["c1", 1],
      ["g1", 2],
      ["c2", 1],
    ]);
  });

  it("sorts siblings by creation, not by arrival order", () => {
    const rows = orderSessionsTree([
      session("root"),
      sessionFixture({ id: "late", parent_session_id: "root", created_at: "2026-02-01T00:00:00Z" }),
      sessionFixture({
        id: "early",
        parent_session_id: "root",
        created_at: "2026-01-01T00:00:00Z",
      }),
    ]);
    expect(rows.map((row) => row.session.id)).toEqual(["root", "early", "late"]);
  });

  // A child whose parent lives in another workspace still has to be reachable:
  // dropping it would hide a running agent.
  it("keeps an orphan flat at the end rather than dropping it", () => {
    const rows = orderSessionsTree([
      session("root"),
      session("orphan", { parent_session_id: "elsewhere" }),
    ]);
    expect(rows.map((row) => [row.session.id, row.depth])).toEqual([
      ["root", 0],
      ["orphan", 0],
    ]);
  });

  it("is empty for an empty list", () => {
    expect(orderSessionsTree([])).toEqual([]);
  });
});

describe("sessionDisplayTitle", () => {
  it("prefixes a harness role", () => {
    const row = sessionFixture({ role: "Orchestrator", title: { user: "spec", terminal: null } });
    expect(sessionDisplayTitle(row)).toBe("Orchestrator · spec");
  });

  it("leaves a generic session unprefixed", () => {
    const row = sessionFixture({ role: "Generic", title: { user: "zsh", terminal: null } });
    expect(sessionDisplayTitle(row)).toBe("zsh");
  });

  it("calls a custom role an agent", () => {
    const row = sessionFixture({ role: { Custom: "juva" }, title: { user: "x", terminal: null } });
    expect(sessionDisplayTitle(row)).toBe("Agent · x");
  });
});

describe("harnessFeatureAgents", () => {
  const sessions = [
    session("orch", { role: "Orchestrator" }),
    session("kid", { parent_session_id: "orch", role: "Executor" }),
    session("unrelated"),
  ];

  it("is the orchestrator plus its direct children", () => {
    expect(harnessFeatureAgents(sessions, "orch").map((row) => row.session.id)).toEqual([
      "orch",
      "kid",
    ]);
  });

  it("is empty when the feature has no orchestrator yet", () => {
    expect(harnessFeatureAgents(sessions, null)).toEqual([]);
  });

  // The orchestrator may have exited and been reaped while the feature row
  // still names it; an empty list is the honest answer.
  it("is empty when the named orchestrator is gone", () => {
    expect(harnessFeatureAgents(sessions, "vanished")).toEqual([]);
  });
});

describe("harnessIsolationNote", () => {
  const workspaces = [
    { id: "main", kind: "Main", branch: "main", display_name: null },
    { id: "tree", kind: "GitWorktree", branch: "feat", display_name: "feat-tree" },
  ];

  it("names the worktree the executor is implementing in", () => {
    const sessions = [
      session("orch", { role: "Orchestrator" }),
      session("exec", { parent_session_id: "orch", role: "Executor", workspace_id: "tree" }),
    ];
    expect(harnessIsolationNote(sessions, workspaces, "orch")).toBe(
      "Implementing in worktree · feat-tree",
    );
  });

  it("names the branch when the executor is in the main checkout", () => {
    const sessions = [
      session("orch", { role: "Orchestrator" }),
      session("exec", { parent_session_id: "orch", role: "Executor", workspace_id: "main" }),
    ];
    expect(harnessIsolationNote(sessions, workspaces, "orch")).toBe("Implementing in main");
  });

  it("is silent when nothing is implementing", () => {
    const sessions = [session("orch", { role: "Orchestrator" })];
    expect(harnessIsolationNote(sessions, workspaces, "orch")).toBeNull();
  });
});
