import type { Project, Session, Workspace } from "../../contracts/runtime";
import { isLive, sessionPhase, type ChipState } from "./actionState";

function basename(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

export type ProcessRow = {
  session: Session;
  label: string;
  workspaceLabel: string;
  state: ChipState;
  live: boolean;
};

export type ProcessGroup = { projectId: string; projectName: string; rows: ProcessRow[] };

/**
 * Every action run still in the store, grouped by project. Live runs sort
 * first, then the most recent exits; a group with a live run sorts before one
 * without. Derived when the store changes, never per frame.
 */
export function groupProcesses(
  sessions: readonly Session[],
  workspaces: readonly Workspace[],
  projects: readonly Project[],
): ProcessGroup[] {
  const workspaceById = new Map(workspaces.map((workspace) => [workspace.id, workspace]));
  const projectById = new Map(projects.map((project) => [project.id, project]));
  const groups = new Map<string, ProcessGroup>();
  for (const session of sessions) {
    if (!session.action) continue;
    const workspace = workspaceById.get(session.workspace_id);
    if (!workspace) continue;
    const projectId = workspace.project_id;
    let group = groups.get(projectId);
    if (!group) {
      group = {
        projectId,
        projectName: projectById.get(projectId)?.name ?? basename(workspace.path),
        rows: [],
      };
      groups.set(projectId, group);
    }
    group.rows.push({
      session,
      label: session.action.label,
      workspaceLabel: workspace.display_name ?? workspace.branch ?? basename(workspace.path),
      state: sessionPhase(session),
      live: isLive(session),
    });
  }
  const byRecency = (a: ProcessRow, b: ProcessRow): number =>
    Number(b.live) - Number(a.live) || b.session.created_at.localeCompare(a.session.created_at);
  const ordered = [...groups.values()];
  for (const group of ordered) group.rows.sort(byRecency);
  const hasLive = (group: ProcessGroup): boolean => group.rows.some((row) => row.live);
  ordered.sort(
    (a, b) => Number(hasLive(b)) - Number(hasLive(a)) || a.projectName.localeCompare(b.projectName),
  );
  return ordered;
}

export function liveCount(groups: readonly ProcessGroup[]): number {
  return groups.reduce((sum, group) => sum + group.rows.filter((row) => row.live).length, 0);
}
