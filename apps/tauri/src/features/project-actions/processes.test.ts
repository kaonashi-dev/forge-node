import { describe, expect, it } from "vitest";
import { sessionFixture } from "../../contracts/sessions.fixture";
import type { Project, Session, Workspace } from "../../contracts/runtime";
import { groupProcesses, liveCount } from "./processes";

function workspace(id: string, project: string, branch: string): Workspace {
  return {
    id,
    project_id: project,
    kind: "Main",
    path: `/repo/${id}`,
    branch,
    display_name: null,
    managed_by_app: false,
    status: { dirty: false } as Workspace["status"],
  };
}

function project(id: string, name: string): Project {
  return { id, project_group_id: null, name, icon: null, root_path: `/repo/${id}` };
}

function run(id: string, ws: string, label: string, created: string, live: boolean): Session {
  return sessionFixture({
    id,
    kind: "Task",
    workspace_id: ws,
    created_at: created,
    state: live ? "Running" : { Exited: { code: 1, signal: null } },
    action: { action_id: label, label, port: null, readiness: { state: "unprobed" } },
  });
}

describe("processes view", () => {
  const workspaces = [workspace("w1", "api", "main"), workspace("w2", "web", "feature/x")];
  const projects = [project("api", "api"), project("web", "web")];

  it("groups by project with live runs first, then recent exits", () => {
    const groups = groupProcesses(
      [
        run("a", "w1", "test", "2026-01-03T00:00:00Z", false),
        run("b", "w2", "dev", "2026-01-01T00:00:00Z", true),
        run("c", "w1", "lint", "2026-01-02T00:00:00Z", false),
        sessionFixture({ id: "shell", workspace_id: "w1" }),
      ],
      workspaces,
      projects,
    );
    expect(groups.map((g) => g.projectName)).toEqual(["web", "api"]);
    expect(groups[0]!.rows[0]).toMatchObject({
      label: "dev",
      workspaceLabel: "feature/x",
      live: true,
    });
    expect(groups[1]!.rows.map((row) => row.label)).toEqual(["test", "lint"]);
    expect(liveCount(groups)).toBe(1);
  });

  it("renders from the store alone and skips orphaned workspaces", () => {
    expect(
      groupProcesses([run("x", "gone", "dev", "2026-01-01T00:00:00Z", true)], workspaces, projects),
    ).toEqual([]);
  });
});
