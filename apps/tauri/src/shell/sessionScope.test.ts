import { describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { Session, Workspace } from "../runtime/types";
import {
  activeWorkspaceId,
  parseTabOrder,
  projectOfWorkspace,
  sessionsInWorkspace,
  storedWorkspaceId,
} from "./sessionScope";

const workspace = (id: string, project_id: string): Workspace => ({
  id,
  project_id,
  kind: "GitWorktree",
  path: `/tmp/${id}`,
  branch: "main",
  display_name: null,
  managed_by_app: false,
  status: { dirty: false, head: null, ahead: null, behind: null, measured_at: null },
});

// Two worktrees of the *same* project, which is the shape the bug needs: a
// filter by project would let these two share a strip and still look correct.
const workspaces = [
  workspace("dev-main", "dev"),
  workspace("dev-test", "dev"),
  workspace("pay-feat", "pay"),
];

const sessions: Session[] = [
  sessionFixture({ id: "s1", workspace_id: "dev-main" }),
  sessionFixture({ id: "s2", workspace_id: "dev-main" }),
  sessionFixture({ id: "s3", workspace_id: "dev-test" }),
  sessionFixture({ id: "s4", workspace_id: "pay-feat" }),
];

describe("sessionScope", () => {
  it("keeps one worktree's open sessions out of another's strip", () => {
    expect(sessionsInWorkspace(sessions, "dev-main").map((s) => s.id)).toEqual(["s1", "s2"]);
    expect(sessionsInWorkspace(sessions, "dev-test").map((s) => s.id)).toEqual(["s3"]);
    expect(sessionsInWorkspace(sessions, "pay-feat").map((s) => s.id)).toEqual(["s4"]);
  });

  it("separates two worktrees of the same project", () => {
    const main = sessionsInWorkspace(sessions, "dev-main").map((s) => s.id);
    const test = sessionsInWorkspace(sessions, "dev-test").map((s) => s.id);
    expect(main.some((id) => test.includes(id))).toBe(false);
  });

  it("leaves out sessions with no terminal", () => {
    const closed = [
      ...sessions,
      sessionFixture({ id: "s5", workspace_id: "dev-main", terminal_id: null }),
    ];
    expect(sessionsInWorkspace(closed, "dev-main").map((s) => s.id)).toEqual(["s1", "s2"]);
  });

  it("shows nothing rather than everything when there is no checkout", () => {
    expect(sessionsInWorkspace(sessions, null)).toEqual([]);
  });

  it("follows the workbench checkout before the active session", () => {
    // The rail pointing at an empty worktree while a session elsewhere is
    // still the active one: picking the worktree has to win, or it could
    // never be selected at all.
    expect(activeWorkspaceId(sessions, "s1", "dev-test", "dev-main")).toBe("dev-test");
  });

  it("falls back to the active session, then to the first checkout", () => {
    expect(activeWorkspaceId(sessions, "s4", null, "dev-main")).toBe("pay-feat");
    expect(activeWorkspaceId(sessions, null, null, "dev-main")).toBe("dev-main");
    expect(activeWorkspaceId(sessions, "missing", null, "dev-main")).toBe("dev-main");
    expect(activeWorkspaceId(sessions, null, null, null)).toBeNull();
  });

  it("names the project a checkout belongs to", () => {
    expect(projectOfWorkspace(workspaces, "dev-test")).toBe("dev");
    expect(projectOfWorkspace(workspaces, "pay-feat")).toBe("pay");
    expect(projectOfWorkspace(workspaces, "missing")).toBeNull();
    expect(projectOfWorkspace(workspaces, null)).toBeNull();
  });

  it("reads a per-checkout tab order and drops anything that is not one", () => {
    expect(parseTabOrder(JSON.stringify({ "dev-main": ["s2", "s1"], "dev-test": ["s3"] }))).toEqual(
      { "dev-main": ["s2", "s1"], "dev-test": ["s3"] },
    );
    expect(parseTabOrder(JSON.stringify({ "dev-main": ["s2", 7, null] }))).toEqual({
      "dev-main": ["s2"],
    });
    expect(parseTabOrder(JSON.stringify({ "dev-main": "s2" }))).toEqual({});
  });

  it("discards the flat order stored before the split, rather than applying it everywhere", () => {
    expect(parseTabOrder(JSON.stringify(["s3", "s1"]))).toEqual({});
    expect(parseTabOrder(undefined)).toEqual({});
    expect(parseTabOrder("not json")).toEqual({});
  });

  it("reopens on the remembered checkout", () => {
    expect(storedWorkspaceId("dev-test", workspaces)).toBe("dev-test");
  });

  it("drops a remembered checkout that is no longer one", () => {
    // A worktree removed between two launches: the window has to fall back to
    // the first-checkout rule rather than point at a row that is gone.
    expect(storedWorkspaceId("dev-gone", workspaces)).toBeNull();
    expect(storedWorkspaceId(undefined, workspaces)).toBeNull();
    expect(storedWorkspaceId("", workspaces)).toBeNull();
    expect(storedWorkspaceId("dev-test", [])).toBeNull();
  });
});
