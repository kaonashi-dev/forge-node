import { describe, expect, it } from "vitest";
import { applyShellSnapshot, emptySnapshot } from "../../state/forgeStore";
import type { Workspace } from "../../contracts/runtime";
import { LAST_WORKSPACE_KEY } from "../../state/preferences";
import { restoreWorkspace } from "./sessionActions";
import { activeWorkspace, focusWorkspace } from "../../state/workspace";

const workspace = (id: string): Workspace => ({
  id,
  project_id: "dev",
  kind: "GitWorktree",
  path: `/tmp/${id}`,
  branch: "main",
  display_name: null,
  managed_by_app: false,
  status: { dirty: false, head: null, ahead: null, behind: null, measured_at: null },
});

function connect(stored: string | undefined): void {
  applyShellSnapshot({
    ...emptySnapshot(),
    workspaces: [workspace("dev-main"), workspace("dev-test")],
    app_state: stored === undefined ? {} : { [LAST_WORKSPACE_KEY]: stored },
  });
}

describe("restoreWorkspace", () => {
  it("opens the window on the checkout it was left in", () => {
    focusWorkspace(null);
    connect("dev-test");
    restoreWorkspace();
    expect(activeWorkspace()).toBe("dev-test");
  });

  it("leaves the fallback alone when the checkout is gone or was never stored", () => {
    focusWorkspace(null);
    connect("dev-removed");
    restoreWorkspace();
    expect(activeWorkspace()).toBeNull();

    connect(undefined);
    restoreWorkspace();
    expect(activeWorkspace()).toBeNull();
  });

  it("yields to a session the daemon put back on screen", () => {
    // `sessions.persist_history = true`: the runtime has already pointed the
    // window at the checkout of a live terminal, which beats a remembered one.
    focusWorkspace(null);
    connect("dev-test");
    focusWorkspace("dev-main");
    restoreWorkspace();
    expect(activeWorkspace()).toBe("dev-main");
    focusWorkspace(null);
  });
});
