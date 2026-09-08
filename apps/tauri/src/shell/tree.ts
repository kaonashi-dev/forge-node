// The rail's four levels, derived from the snapshot.
//
// Group → project → workspace → session. Built as plain data so the ordering
// and the "does anything under here want me" rollup can be tested without a
// DOM, which is where the rules that matter live: a folded row must never be
// able to hide a question (§16.3).

import type { PullRequest, ReviewDecision, Session, ShellSnapshot } from "../runtime/types";
import { sessionIsActive } from "../runtime/types";
import { orderSessionsTree, sessionDisplayTitle } from "./sessionTree";
import { orderWorkspaces, type WorkspaceOrderMap } from "./workspaceOrder";

export type SessionNode = {
  session: Session;
  /** The label the row shows, with its harness role when it has one. */
  label: string;
  /**
   * How deep under its parent this session sits; `0` is a root.
   *
   * The harness starts an orchestrator and hangs its steps off it. Side by
   * side those rows read as five unrelated agents rather than one piece of
   * work, which is the question the rail exists to answer.
   */
  depth: number;
  wantsYou: boolean;
  unread: boolean;
};

/** What the rail shows of an open pull request; the panel owns the rest. */
export type WorkspacePullRequest = {
  number: number;
  draft: boolean;
  decision: ReviewDecision | null;
  title: string;
};

export type WorkspaceNode = {
  id: string;
  label: string;
  path: string;
  branch: string | null;
  /** A worktree, as opposed to the repository's own checkout. */
  worktree: boolean;
  dirty: boolean;
  ahead: number | null;
  behind: number | null;
  /** `null` means "not measured", which is not the same as clean. */
  measured: boolean;
  pullRequest: WorkspacePullRequest | null;
  sessions: SessionNode[];
  wantsYou: boolean;
};

export type ProjectNode = {
  id: string;
  name: string;
  icon: string | null;
  workspaces: WorkspaceNode[];
  wantsYou: boolean;
};

export type GroupNode = {
  /** `null` is the implicit group of projects that belong to none. */
  id: string | null;
  name: string | null;
  projects: ProjectNode[];
  wantsYou: boolean;
};

export function buildTree(
  store: ShellSnapshot,
  workspaceOrder: WorkspaceOrderMap = {},
): GroupNode[] {
  const groups: GroupNode[] = store.project_groups.map((group) => ({
    id: group.id,
    name: group.name,
    projects: [],
    wantsYou: false,
  }));
  // Projects in no group still need a home, and it goes last: a named group is
  // something the user made, and it should not be pushed below the leftovers.
  const loose: GroupNode = { id: null, name: null, projects: [], wantsYou: false };

  for (const project of store.projects) {
    const workspaces = store.workspaces
      .filter((workspace) => workspace.project_id === project.id)
      .map((workspace) => {
        // Ordered as a tree rather than flat, then flattened for the rail:
        // the indent is the only thing that says a row is a step of the row
        // above it.
        const sessions = orderSessionsTree(
          store.sessions.filter((session) => session.workspace_id === workspace.id),
        ).map(({ session, depth }) => ({
          session,
          label: sessionDisplayTitle(session),
          depth,
          wantsYou: store.session_attention[session.id]?.wants_you ?? false,
          unread: store.session_attention[session.id]?.unread ?? false,
        }));
        return {
          id: workspace.id,
          label: workspace.display_name ?? workspace.branch ?? basename(workspace.path),
          path: workspace.path,
          branch: workspace.branch,
          worktree: workspace.kind === "GitWorktree",
          dirty: workspace.status.dirty,
          ahead: workspace.status.ahead,
          behind: workspace.status.behind,
          measured: workspace.status.measured_at !== null,
          pullRequest: pullRequestFor(store, project.id, workspace.branch),
          sessions,
          wantsYou: sessions.some((node) => node.wantsYou),
        } satisfies WorkspaceNode;
      });
    const ordered = orderWorkspaces(workspaces, workspaceOrder[project.id] ?? []);

    const node: ProjectNode = {
      id: project.id,
      name: project.name,
      icon: project.icon,
      workspaces: ordered,
      wantsYou: ordered.some((workspace) => workspace.wantsYou),
    };
    const group = groups.find((item) => item.id === project.project_group_id) ?? loose;
    group.projects.push(node);
  }

  for (const group of groups) group.wantsYou = group.projects.some((p) => p.wantsYou);
  loose.wantsYou = loose.projects.some((project) => project.wantsYou);

  const out = groups;
  if (loose.projects.length > 0) out.push(loose);
  return out;
}

/**
 * Every session asking for the user, the one that has waited longest first.
 *
 * Longest-waiting leads because that is the one at risk of being forgotten; a
 * rail sorted by recency would keep burying it.
 *
 * Shells are deliberately excluded upstream: a `\a` from a shell is usually a
 * completion beep, and a rail that lights up because zsh could not complete a
 * filename teaches the user to ignore the one mark that means *come here*.
 */
export function waiting(store: ShellSnapshot): Session[] {
  return store.sessions
    .filter((session) => store.session_attention[session.id]?.wants_you)
    .sort((a, b) =>
      (a.last_activity_at ?? a.created_at).localeCompare(b.last_activity_at ?? b.created_at),
    );
}

/** Sessions with a terminal, in creation order: the tab strip's contents. */
export function openSessions(store: ShellSnapshot): Session[] {
  return store.sessions.filter((session) => session.terminal_id !== null);
}

export function liveSessions(store: ShellSnapshot): Session[] {
  return store.sessions.filter((session) => sessionIsActive(session.state));
}

/**
 * The open pull request for a branch, from the last remote read.
 *
 * Matched on head ref *and* project: two repositories in the same window can
 * both have a `main`, and a cache that answers for the wrong one puts a PR
 * badge on a checkout that has none.
 */
function pullRequestFor(
  store: ShellSnapshot,
  projectId: string,
  branch: string | null,
): WorkspacePullRequest | null {
  if (!branch) return null;
  const found: PullRequest | undefined = store.pull_requests.pull_requests.find(
    (pr) => pr.head_ref === branch && pr.project_id === projectId,
  );
  if (!found) return null;
  return {
    number: found.number,
    draft: found.is_draft,
    decision: found.review_decision,
    title: found.title,
  };
}

function basename(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

/* ------------------------------------------------------- keyboard model --- */

/** What a rail row is, for the keyboard (§4.1 U4). */
export type RailRowKind = "group" | "project" | "workspace" | "session";

export type RailRow = {
  /** Stable across renders, and unique across the four levels. */
  id: string;
  kind: RailRowKind;
  /** Nesting depth, `0` at the top, for `aria-level`. */
  depth: number;
  /** `null` for a top-level row. */
  parent: string | null;
  /** `false` for a session, which has nothing under it. */
  expandable: boolean;
  expanded: boolean;
  /** The identifier the row acts on: a session id, a workspace id, a name. */
  target: string;
};

/**
 * The rail as one flat list, in the order it is painted.
 *
 * The rail renders as four nested `For`s, which is the right shape for the
 * markup and the wrong shape for `↑`/`↓`: "the row after this one" crosses
 * every one of those nestings. Flattening once, here, is what lets the
 * keyboard model be four lines of index arithmetic — and lets it be tested
 * without mounting anything.
 *
 * A collapsed row's children are absent rather than marked, because a row that
 * cannot be seen must not be reachable by an arrow key.
 */
export function railRows(groups: GroupNode[], collapsed: ReadonlySet<string>): RailRow[] {
  const rows: RailRow[] = [];
  const open = (id: string) => !collapsed.has(id);

  for (const group of groups) {
    // The implicit group of ungrouped projects has no row of its own: there
    // is nothing to fold and nothing to name.
    const groupId = group.id === null || group.name === null ? null : `group:${group.id}`;
    if (groupId !== null) {
      rows.push({
        id: groupId,
        kind: "group",
        depth: 0,
        parent: null,
        expandable: true,
        expanded: open(groupId),
        target: group.id ?? "",
      });
      if (!open(groupId)) continue;
    }
    const groupDepth = groupId === null ? 0 : 1;

    for (const project of group.projects) {
      const projectId = `project:${project.id}`;
      rows.push({
        id: projectId,
        kind: "project",
        depth: groupDepth,
        parent: groupId,
        expandable: true,
        expanded: open(projectId),
        target: project.id,
      });
      if (!open(projectId)) continue;

      for (const workspace of project.workspaces) {
        const workspaceId = `workspace:${workspace.id}`;
        rows.push({
          id: workspaceId,
          kind: "workspace",
          depth: groupDepth + 1,
          parent: projectId,
          expandable: true,
          expanded: open(workspaceId),
          target: workspace.id,
        });
        if (!open(workspaceId)) continue;

        for (const session of workspace.sessions) {
          rows.push({
            id: `session:${session.session.id}`,
            kind: "session",
            depth: groupDepth + 2 + session.depth,
            parent: workspaceId,
            expandable: false,
            expanded: false,
            target: session.session.id,
          });
        }
      }
    }
  }
  return rows;
}

/**
 * Where `←` goes: fold this row, or move to its parent when it is already
 * folded (or cannot fold at all). The behaviour every tree widget has, and the
 * reason a session row is not a dead end for the left arrow.
 */
export function railCollapseTarget(
  row: RailRow | undefined,
): { fold: string } | { select: string } | null {
  if (!row) return null;
  if (row.expandable && row.expanded) return { fold: row.id };
  return row.parent === null ? null : { select: row.parent };
}

/** Where `→` goes: unfold, or move to the first child of an open row. */
export function railExpandTarget(
  row: RailRow | undefined,
  rows: RailRow[],
  index: number,
): { unfold: string } | { select: string } | null {
  if (!row || !row.expandable) return null;
  if (!row.expanded) return { unfold: row.id };
  const child = rows[index + 1];
  return child && child.parent === row.id ? { select: child.id } : null;
}
