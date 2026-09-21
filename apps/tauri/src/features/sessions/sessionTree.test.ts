import { describe, expect, it } from "vitest";
import { sessionFixture } from "../../contracts/sessions.fixture";
import type { Session } from "../../contracts/runtime";
import { orderSessionsTree, sessionDisplayTitle } from "./sessionTree";

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
  it("prefixes a non-generic role", () => {
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
