import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import type { Launchable, Workspace } from "../runtime/types";
import {
  createdWorkspace,
  defaultLaunch,
  launchOptions,
  NOTHING,
  parseLaunch,
  slugFromBranch,
} from "./worktreeLaunch";

function launchable(partial: Partial<Launchable> = {}): Launchable {
  return {
    kind: "agent",
    label: "Claude",
    detail: null,
    provider: "claude",
    profile: null,
    enabled: true,
    key: "agent:claude",
    supports_initial_prompt: true,
    ...partial,
  };
}

const shell = launchable({ kind: "shell", label: "Shell", provider: null, key: "shell" });

function workspace(partial: Partial<Workspace> & Pick<Workspace, "id">): Workspace {
  return {
    project_id: "p1",
    kind: "Worktree",
    path: "/wt",
    branch: "feat/x",
    display_name: null,
    managed_by_app: true,
    status: { dirty: false, ahead: null, behind: null, measured_at: null },
    ...partial,
  };
}

describe("slugFromBranch", () => {
  // The cases `git_service::worktree::slugify` is tested with, mirrored.
  it("maps slashes and specials to a dash", () => {
    expect(slugFromBranch("feature/auth")).toBe("feature-auth");
    expect(slugFromBranch("feat/JIRA-123_fix.v2")).toBe("feat-JIRA-123_fix.v2");
    expect(slugFromBranch("hello world!")).toBe("hello-world-");
  });

  it("collapses repeated dashes", () => {
    expect(slugFromBranch("a//b")).toBe("a-b");
    expect(slugFromBranch("a  b")).toBe("a-b");
  });

  it("caps at 64 characters", () => {
    expect(slugFromBranch("x".repeat(200))).toHaveLength(64);
  });

  it("never returns a path that escapes the worktree root", () => {
    expect(slugFromBranch("")).toBe("worktree");
    expect(slugFromBranch(".")).toBe("worktree");
    expect(slugFromBranch("..")).toBe("worktree");
  });
});

describe("launchOptions", () => {
  it("offers nothing first, then everything that could run here", () => {
    const options = launchOptions([shell, launchable()]);
    expect(options.map((option) => option.value)).toEqual([NOTHING, "shell", "agent:claude"]);
    expect(options[1].label).toBe("New Terminal");
    expect(options[2].label).toBe("New Claude");
  });

  it("lists an uninstalled agent, disabled, the way the launcher does", () => {
    const options = launchOptions([launchable({ enabled: false })]);
    expect(options[1].disabled).toBe(true);
  });
});

describe("parseLaunch", () => {
  it("reads a shell, an agent and nothing", () => {
    const list = [shell, launchable()];
    expect(parseLaunch("shell", list)).toEqual({ kind: "shell" });
    expect(parseLaunch("agent:claude", list)).toEqual({
      kind: "agent",
      provider: "claude",
      profile: null,
    });
    expect(parseLaunch(NOTHING, list)).toEqual({ kind: "none" });
  });

  it("refuses to launch something that is not installed or not there", () => {
    expect(parseLaunch("agent:claude", [launchable({ enabled: false })])).toEqual({ kind: "none" });
    expect(parseLaunch("agent:gone", [shell])).toEqual({ kind: "none" });
  });

  it("carries the profile, so a profiled launcher starts its own profile", () => {
    const profiled = launchable({ profile: "review", key: "profile:review" });
    expect(parseLaunch("profile:review", [profiled])).toEqual({
      kind: "agent",
      provider: "claude",
      profile: "review",
    });
  });
});

describe("defaultLaunch", () => {
  it("opens on a terminal, because creating and then hunting is the step this removes", () => {
    expect(defaultLaunch([launchable(), shell])).toBe("shell");
  });

  it("still names the shell when the list has not arrived", () => {
    expect(defaultLaunch([])).toBe("shell");
  });
});

describe("createdWorkspace", () => {
  const store = {
    ...emptySnapshot(),
    workspaces: [
      workspace({ id: "w1", branch: "main", kind: "Main" }),
      workspace({ id: "w2", branch: "feat/x" }),
      workspace({ id: "w3", project_id: "p2", branch: "feat/x" }),
    ],
  };

  it("finds the new checkout by the branch it was made for", () => {
    expect(createdWorkspace(store, "p1", "feat/x")?.id).toBe("w2");
  });

  it("does not cross projects, even on the same branch name", () => {
    expect(createdWorkspace(store, "p2", "feat/x")?.id).toBe("w3");
  });

  it("is null until the broadcast lands", () => {
    expect(createdWorkspace(store, "p1", "feat/unborn")).toBeNull();
  });
});
