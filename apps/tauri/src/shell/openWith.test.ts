import { describe, expect, it } from "vitest";
import { OPEN_WITH_KEY, openTargetPath, openTargets, preferredTarget } from "./openWith";

const snapshot = {
  sessions: [
    { id: "s1", workspace_id: "w1" },
    { id: "s2", workspace_id: "gone" },
  ],
  workspaces: [
    { id: "w1", path: "/repo/main" },
    { id: "w2", path: "/worktrees/feature" },
  ],
} as unknown as Parameters<typeof openTargetPath>[0];

describe("openTargetPath", () => {
  it("follows the active session's checkout", () => {
    expect(openTargetPath(snapshot, "s1", "w2")).toBe("/repo/main");
  });

  it("falls back to the focused workspace when no session is active", () => {
    expect(openTargetPath(snapshot, null, "w2")).toBe("/worktrees/feature");
  });

  it("has nothing to open when neither names a checkout", () => {
    expect(openTargetPath(snapshot, null, null)).toBeNull();
  });

  // A retained worktree can outlive its directory in the snapshot's session
  // list, so a session pointing at a workspace that is gone is not a path.
  it("has nothing to open when the workspace is missing", () => {
    expect(openTargetPath(snapshot, "s2", null)).toBeNull();
  });
});

describe("preferredTarget", () => {
  it("runs the remembered target", () => {
    expect(preferredTarget({ [OPEN_WITH_KEY]: "cursor" }).id).toBe("cursor");
  });

  it("falls back to the first target on an empty or unknown preference", () => {
    expect(preferredTarget({}).id).toBe(openTargets()[0].id);
    expect(preferredTarget({ [OPEN_WITH_KEY]: "emacs" }).id).toBe(openTargets()[0].id);
  });
});
