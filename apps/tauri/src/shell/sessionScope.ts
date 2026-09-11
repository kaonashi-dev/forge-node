// What checkout the window is in, and what belongs to it.
//
// The rail is four levels deep (group → project → workspace → session) but the
// window only ever tracked two selections: the active session and the checkout
// the workbench points at. The tab strip sat above both and showed every open
// session in the daemon, so walking from one checkout to another left the
// previous one's terminals in the strip — tabs for a branch that is no longer
// on screen, one click away from typing into the wrong worktree.
//
// The checkout is the unit, not the project: two worktrees of the same
// repository are two working directories with two branches and two sets of
// uncommitted changes, and a terminal in one of them is no more relevant to
// the other than a terminal in a different repository would be. It is also the
// unit everything else in the window already uses — `viewsStore` parks the
// Code tab per checkout, `workbenchStore` clears its answers per checkout —
// so the strip was the last surface with a scope of its own.
//
// Pure functions over the snapshot, so the rules are testable without a DOM;
// `AppShell` is the only place that reads the live stores.

import type { Session, Workspace } from "../runtime/types";

/**
 * Tab order, per checkout.
 *
 * A single flat list is what the window used to persist, which meant dragging
 * a tab in one worktree rewrote the order every other worktree read from.
 */
export type TabOrderMap = Record<string, string[]>;

/**
 * The checkout the window is in.
 *
 * The workbench checkout leads, because it is the one selection every gesture
 * writes: `focusSession` points it at the session's checkout before it tells
 * the daemon anything, the rail's cards point it at themselves, and
 * `AppShell` points it at the active session whenever the daemon moves that
 * on its own. Deriving it from the active session instead would make a
 * worktree with no sessions unreachable — picking it in the rail could not
 * change an answer that reads the session it does not have.
 *
 * The active session is the fallback for the frames before any of that has
 * run — a window that has just connected — and the first checkout for a window
 * with no session at all, so `⌘T` on a fresh launch has somewhere to land.
 */
export function activeWorkspaceId(
  sessions: readonly Session[],
  activeSession: string | null,
  workbenchWorkspace: string | null,
  firstWorkspace: string | null,
): string | null {
  if (workbenchWorkspace) return workbenchWorkspace;
  const session = activeSession ? sessions.find((item) => item.id === activeSession) : undefined;
  return session?.workspace_id ?? firstWorkspace;
}

/**
 * The open sessions of one checkout: everything the strip may show.
 *
 * A null checkout means the window has none, which is not the same as "no
 * filter" — answering with every session there is how the bug this module
 * exists for got in.
 */
export function sessionsInWorkspace(
  sessions: readonly Session[],
  workspace: string | null,
): Session[] {
  if (!workspace) return [];
  return sessions.filter(
    (session) => session.terminal_id !== null && session.workspace_id === workspace,
  );
}

/** The project a checkout belongs to; what the branch picker and menus name. */
export function projectOfWorkspace(
  workspaces: readonly Workspace[],
  workspace: string | null | undefined,
): string | null {
  if (!workspace) return null;
  return workspaces.find((item) => item.id === workspace)?.project_id ?? null;
}

/**
 * The remembered checkout, if it is still one.
 *
 * Validated against the snapshot rather than trusted: a worktree can be
 * removed between two launches, and a window that opened pointed at a row
 * that is gone shows a tab strip and a rail scoped to nothing at all. An
 * unknown id falls back to the same first-checkout rule a fresh install gets.
 */
export function storedWorkspaceId(
  stored: string | undefined,
  workspaces: readonly Workspace[],
): string | null {
  if (!stored) return null;
  return workspaces.some((item) => item.id === stored) ? stored : null;
}

/**
 * Read the persisted tab order.
 *
 * A stored value from before the order was split per checkout is a flat array
 * whose ids cannot be attributed to one after the fact. Dropping it costs one
 * checkout's worth of manual ordering and leaves the strip in creation order,
 * which is what an unordered checkout shows anyway; keeping it would apply one
 * checkout's order to all of them, which is the bug again.
 */
export function parseTabOrder(raw: string | undefined): TabOrderMap {
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: TabOrderMap = {};
    for (const [workspace, order] of Object.entries(parsed as Record<string, unknown>)) {
      if (Array.isArray(order)) {
        out[workspace] = order.filter((id): id is string => typeof id === "string");
      }
    }
    return out;
  } catch {
    return {};
  }
}
