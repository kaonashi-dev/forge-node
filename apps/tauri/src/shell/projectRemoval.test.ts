import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { ShellSnapshot, Workspace } from "../runtime/types";
import { describeProjectRemoval, policyIsRefused, projectFootprint } from "./projectRemoval";

/** Status is never `null` on the wire, and no test here reads it. */
const UNMEASURED = { dirty: false, ahead: null, behind: null, measured_at: null };

function workspace(partial: Partial<Workspace> & Pick<Workspace, "id" | "project_id">): Workspace {
  return {
    kind: "Main",
    path: "/r",
    branch: "main",
    display_name: null,
    managed_by_app: false,
    status: UNMEASURED,
    ...partial,
  };
}

function snapshot(): ShellSnapshot {
  return {
    ...emptySnapshot(),
    projects: [{ id: "p1", project_group_id: null, name: "forge", icon: null, root_path: "/r" }],
    workspaces: [
      workspace({ id: "w1", project_id: "p1" }),
      workspace({
        id: "w2",
        project_id: "p1",
        kind: "Worktree",
        branch: "feat",
        managed_by_app: true,
      }),
      workspace({ id: "other", project_id: "p2", managed_by_app: true }),
    ],
    sessions: [
      sessionFixture({ id: "s1", workspace_id: "w2", state: "Running" }),
      sessionFixture({ id: "s2", workspace_id: "w1", state: "Exited" }),
      sessionFixture({ id: "s3", workspace_id: "other", state: "Running" }),
    ],
  };
}

describe("projectFootprint", () => {
  it("counts only this project's checkouts, managed worktrees and live sessions", () => {
    expect(projectFootprint(snapshot(), "p1")).toEqual({
      checkouts: 2,
      managedWorktrees: 1,
      runningSessions: 1,
    });
  });

  it("is all zeroes for a project with nothing under it", () => {
    expect(projectFootprint(emptySnapshot(), "gone")).toEqual({
      checkouts: 0,
      managedWorktrees: 0,
      runningSessions: 0,
    });
  });
});

describe("describeProjectRemoval", () => {
  const footprint = { checkouts: 2, managedWorktrees: 1, runningSessions: 1 };

  it("promises the disk is untouched when everything is kept", () => {
    expect(describeProjectRemoval("keep_everything", footprint)).toEqual([
      "Forge Node stops tracking the project.",
      "Every checkout stays on disk, exactly as it is.",
      "No branch and no commit is deleted.",
    ]);
  });

  it("counts what the strongest policy kills and deletes", () => {
    expect(describeProjectRemoval("kill_sessions_and_worktrees", footprint)).toEqual([
      "Forge Node stops tracking the project.",
      "1 running session killed.",
      "1 worktree Forge Node created is deleted from disk.",
      "No branch and no commit is deleted.",
    ]);
  });

  it("says so rather than counting to zero", () => {
    expect(
      describeProjectRemoval("kill_sessions_and_worktrees", {
        checkouts: 1,
        managedWorktrees: 0,
        runningSessions: 0,
      }),
    ).toEqual([
      "Forge Node stops tracking the project.",
      "Nothing is running, so no session is killed.",
      "No worktree here was created by Forge Node, so none is deleted.",
      "No branch and no commit is deleted.",
    ]);
  });

  it("pluralises both counts", () => {
    expect(
      describeProjectRemoval("kill_sessions_and_worktrees", {
        checkouts: 4,
        managedWorktrees: 3,
        runningSessions: 2,
      }),
    ).toContain("3 worktrees Forge Node created are deleted from disk.");
  });
});

describe("policyIsRefused", () => {
  it("refuses to keep everything while a session is running", () => {
    expect(
      policyIsRefused("keep_everything", { checkouts: 1, managedWorktrees: 0, runningSessions: 1 }),
    ).toBe(true);
  });

  it("allows it once nothing is running", () => {
    expect(
      policyIsRefused("keep_everything", { checkouts: 1, managedWorktrees: 0, runningSessions: 0 }),
    ).toBe(false);
  });

  it("never refuses a policy that kills first", () => {
    const busy = { checkouts: 1, managedWorktrees: 1, runningSessions: 3 };
    expect(policyIsRefused("kill_sessions", busy)).toBe(false);
    expect(policyIsRefused("kill_sessions_and_worktrees", busy)).toBe(false);
  });
});
