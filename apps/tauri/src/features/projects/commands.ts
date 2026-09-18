import type {
  ProjectRemovalPolicy,
  ShareCleanup,
  ShareRule,
  WorktreeIgnore,
} from "../../contracts/runtime";
import { pickDirectory, sendRuntimeCommand } from "../../runtime/host";
import { markApplying, setSharesStore } from "./sharesStore";
import { sendWorkbenchCommand } from "../../runtime/workbench";

/**
 * These are workbench reads for the same reason a diff is: the daemon answers
 * `DetectShareCandidates` with a `git status --ignored` plus a bounded walk,
 * and an apply can run `pnpm install`. None of that may share the thread that
 * carries keystrokes.
 */
export async function detectShareCandidates(project: string): Promise<void> {
  setSharesStore("scanning", true);
  await sendWorkbenchCommand({ type: "detect_share_candidates", project });
}

export async function previewShares(workspace: string): Promise<void> {
  await sendWorkbenchCommand({ type: "preview_shares", workspace });
}

export async function loadShareStatus(workspace: string): Promise<void> {
  await sendWorkbenchCommand({ type: "load_share_status", workspace });
}

/** Ack means *started*: the outcome arrives as `runtime:shares_applied`. */
export async function applyShares(workspace: string, only?: string[]): Promise<void> {
  markApplying(workspace);
  await sendWorkbenchCommand({ type: "apply_shares", workspace, only: only ?? null });
}

export async function adoptIntoShareStore(project: string, path: string): Promise<void> {
  await sendWorkbenchCommand({ type: "adopt_into_share_store", project, path });
}

export async function materializeFromShareStore(
  project: string,
  path: string,
  workspace?: string,
): Promise<void> {
  await sendWorkbenchCommand({
    type: "materialize_from_share_store",
    project,
    path,
    workspace: workspace ?? null,
  });
}

export async function createWorktree(
  project: string,
  branch: string,
  base: string | null = null,
  name: string | null = null,
): Promise<void> {
  await sendRuntimeCommand({ type: "create_worktree", project, branch, base, name });
}

export async function fetchRemote(project: string, remote: string | null = null): Promise<void> {
  await sendRuntimeCommand({ type: "fetch_remote", project, remote });
}

// --- Projects, groups and checkouts -----------------------------------------
//
// Every one of these acks and then broadcasts: the row that changed redraws
// from the snapshot the daemon sends, never from a return value here.

/**
 * Ask the platform for a directory, then register it.
 *
 * The picker is modal, so it lives on the host and answers on its own channel;
 * `null` means the user cancelled, which is not an error and gets no notice.
 */
export async function addProjectFromPicker(group: string | null = null): Promise<void> {
  const path = await pickDirectory("Add project");
  if (path === null) return;
  await sendRuntimeCommand({ type: "add_project", path, group });
}

export async function addProject(path: string, group: string | null = null): Promise<void> {
  await sendRuntimeCommand({ type: "add_project", path, group });
}

/** Re-detect a project's git root, branches and worktrees. */
export async function refreshProject(project: string): Promise<void> {
  await sendRuntimeCommand({ type: "refresh_project", project });
}

/** `null` moves it to General. */
export async function moveProject(project: string, group: string | null): Promise<void> {
  await sendRuntimeCommand({ type: "move_project", project, group });
}

/** `null` clears it, so the rail falls back to the initials of the name. */
export async function setProjectIcon(project: string, icon: string | null): Promise<void> {
  await sendRuntimeCommand({ type: "set_project_icon", project, icon });
}

export async function createProjectGroup(name: string): Promise<void> {
  await sendRuntimeCommand({ type: "create_project_group", name });
}

export async function renameProjectGroup(group: string, name: string): Promise<void> {
  await sendRuntimeCommand({ type: "rename_project_group", group, name });
}

/** Removes the grouping only; the project directories are untouched. */
export async function removeProjectGroup(group: string): Promise<void> {
  await sendRuntimeCommand({ type: "remove_project_group", group });
}

/**
 * Remove a project.
 *
 * No policy ever deletes a branch, and only `kill_sessions_and_worktrees`
 * touches the disk — and then only the worktrees Forge created itself. The
 * daemon refuses `keep_everything` while sessions are still running, which is
 * why the dialog asks for the policy up front rather than discovering it from
 * a refusal.
 */
export async function removeProject(project: string, policy: ProjectRemovalPolicy): Promise<void> {
  await sendRuntimeCommand({ type: "remove_project", project, policy });
}

/** `null` falls back to the branch name. */
export async function renameWorkspace(
  workspace: string,
  displayName: string | null,
): Promise<void> {
  await sendRuntimeCommand({ type: "rename_workspace", workspace, display_name: displayName });
}

/**
 * Remove a managed worktree; never deletes a branch.
 *
 * Without `force` the daemon refuses while the tree is dirty, a merge is in
 * progress, or sessions are running. The host reports that refusal through a
 * contextual event so the shell can ask before reissuing with `force`.
 */
export async function removeWorktree(workspace: string, force = false): Promise<void> {
  await sendRuntimeCommand({ type: "remove_worktree", workspace, force });
}

/**
 * Replace a project's whole rule set.
 *
 * The set, not a row: adding, reordering, enabling and re-pointing a strategy
 * are one edit of one list, so they are one request and one event. Removal is
 * separate — it is the only edit that can delete a file.
 */
export async function setProjectShares(project: string, rules: ShareRule[]): Promise<void> {
  await sendRuntimeCommand({ type: "set_project_shares", project, rules });
}

/** Drop one rule, saying what happens to the files it already wrote. */
export async function removeShareRule(
  project: string,
  rule: string,
  cleanup: ShareCleanup = "leave",
): Promise<void> {
  await sendRuntimeCommand({ type: "remove_share_rule", project, rule, cleanup });
}

/**
 * Replace a project's whole ignore set.
 *
 * The set, not a row, for the same reason as the share rules: adding and
 * removing are one edit of one list. The daemon rescans the project before it
 * acks, so the rail has already dropped what the new rules cover when this
 * returns.
 */
export async function setWorktreeIgnores(project: string, rules: WorktreeIgnore[]): Promise<void> {
  await sendRuntimeCommand({ type: "set_worktree_ignores", project, rules });
}
