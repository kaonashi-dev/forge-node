import type { ProjectRemovalPolicy, ShellSnapshot } from "../runtime/types";
import { sessionIsActive } from "../runtime/types";

/** What a project is holding, counted for the removal dialog. */
export type ProjectFootprint = {
  /** Every checkout of the project, main included. */
  checkouts: number;
  /** Worktrees Forge created itself — the only ones any policy may delete. */
  managedWorktrees: number;
  /** Sessions still running in any of them. */
  runningSessions: number;
};

/**
 * Count what removing `project` would touch.
 *
 * Derived from the snapshot rather than asked of the daemon: `RemoveProject`
 * acks and broadcasts like every other mutation, so there is no round trip to
 * hang a count on, and the rail already holds the same rows the dialog is
 * describing.
 */
export function projectFootprint(store: ShellSnapshot, project: string): ProjectFootprint {
  const workspaces = store.workspaces.filter((item) => item.project_id === project);
  const ids = new Set(workspaces.map((item) => item.id));
  return {
    checkouts: workspaces.length,
    managedWorktrees: workspaces.filter((item) => item.managed_by_app).length,
    runningSessions: store.sessions.filter(
      (item) => ids.has(item.workspace_id) && sessionIsActive(item.state),
    ).length,
  };
}

/**
 * The consequence of one policy, in the words the dialog shows.
 *
 * Written against the footprint rather than as fixed copy: "kill 3 running
 * sessions" and "nothing is running" are the same choice, and a person
 * deciding whether to take it needs the number, not the verb.
 */
export function describeProjectRemoval(
  policy: ProjectRemovalPolicy,
  footprint: ProjectFootprint,
): string[] {
  const lines = ["Forge Node stops tracking the project."];
  const running = footprint.runningSessions;

  switch (policy) {
    case "keep_everything":
      lines.push("Every checkout stays on disk, exactly as it is.");
      break;
    case "kill_sessions":
      lines.push(
        running === 0
          ? "Nothing is running, so no session is killed."
          : `${plural(running, "running session")} killed.`,
      );
      lines.push("Every checkout stays on disk.");
      break;
    case "kill_sessions_and_worktrees":
      lines.push(
        running === 0
          ? "Nothing is running, so no session is killed."
          : `${plural(running, "running session")} killed.`,
      );
      lines.push(
        footprint.managedWorktrees === 0
          ? "No worktree here was created by Forge Node, so none is deleted."
          : `${plural(footprint.managedWorktrees, "worktree")} Forge Node created ${
              footprint.managedWorktrees === 1 ? "is" : "are"
            } deleted from disk.`,
      );
      break;
  }

  // Last, and on every policy: it is the question people actually have.
  lines.push("No branch and no commit is deleted.");
  return lines;
}

/**
 * Whether the daemon would refuse this policy outright.
 *
 * `KeepEverything` is rejected while anything is running, so the dialog
 * disables it rather than offering a button that comes back as a notice.
 */
export function policyIsRefused(
  policy: ProjectRemovalPolicy,
  footprint: ProjectFootprint,
): boolean {
  return policy === "keep_everything" && footprint.runningSessions > 0;
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}
