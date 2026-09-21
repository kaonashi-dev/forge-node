/**
 * What the Ctrl+Tab ring walks: one row per pane the window can raise.
 *
 * The ring mirrors the strip, with the Code tab expanded. What a reader wants
 * back is `main.ts`, not the container `main.ts` happens to be parked in — but
 * an empty Code tab is still a tab the strip offers, so it keeps one row.
 */

import { viewKey, type WorkbenchView } from "./views";

export type SwitchTarget =
  | { kind: "session"; id: string }
  | { kind: "view"; workspace: string; view: WorkbenchView }
  | { kind: "code"; workspace: string };

export function sessionKey(id: string): string {
  return `session:${id}`;
}

export function codeKey(workspace: string): string {
  return `code:${workspace}`;
}

/** Ring identity. Views carry their checkout: every checkout has a `diff`. */
export function targetKey(target: SwitchTarget): string {
  switch (target.kind) {
    case "session":
      return sessionKey(target.id);
    case "code":
      return codeKey(target.workspace);
    case "view":
      return `view:${target.workspace}:${viewKey(target.view)}`;
  }
}

/** This checkout's panes, in strip order: Code first, then the terminals. */
export function localTargets(
  workspace: string | null,
  views: readonly WorkbenchView[],
  sessionIds: readonly string[],
): SwitchTarget[] {
  const targets: SwitchTarget[] = [];
  if (workspace) {
    if (views.length === 0) targets.push({ kind: "code", workspace });
    else for (const view of views) targets.push({ kind: "view", workspace, view });
  }
  for (const id of sessionIds) targets.push({ kind: "session", id });
  return targets;
}

/**
 * The pane on screen.
 *
 * The Code tab holding the terminal view *is* the terminal: that view is the
 * floor the Code strip sits on, and the session tab is where it gets selected
 * from, so the ring must not offer two rows for one pane.
 */
export function activeTargetKey(
  onCode: boolean,
  workspace: string | null,
  views: readonly WorkbenchView[],
  activeView: WorkbenchView,
  activeSession: string | null,
): string | null {
  if (onCode && workspace) {
    if (views.length === 0) return codeKey(workspace);
    if (activeView.kind !== "terminal") {
      return targetKey({ kind: "view", workspace, view: activeView });
    }
  }
  return activeSession ? sessionKey(activeSession) : null;
}
