import { describe, expect, it } from "vitest";
import { applyShellSnapshot, emptySnapshot } from "../store/forgeStore";
import { focusWorkspace, workbenchStore } from "../store/workbenchStore";
import type { Workspace } from "../runtime/types";
import { LAST_WORKSPACE_KEY } from "./layout";
import { restoreWorkspace } from "./sessionActions";

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
    expect(workbenchStore.workspace).toBe("dev-test");
  });

  it("leaves the fallback alone when the checkout is gone or was never stored", () => {
    focusWorkspace(null);
    connect("dev-removed");
    restoreWorkspace();
    expect(workbenchStore.workspace).toBeNull();

    connect(undefined);
    restoreWorkspace();
    expect(workbenchStore.workspace).toBeNull();
  });

  it("yields to a session the daemon put back on screen", () => {
    // `sessions.persist_history = true`: the runtime has already pointed the
    // window at the checkout of a live terminal, which beats a remembered one.
    focusWorkspace(null);
    connect("dev-test");
    focusWorkspace("dev-main");
    restoreWorkspace();
    expect(workbenchStore.workspace).toBe("dev-main");
    focusWorkspace(null);
  });
});
