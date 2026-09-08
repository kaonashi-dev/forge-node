// The three session gestures, in one place so the overlay, the session menu
// and the command palette all run the same code (§16.7, §16.8).
//
// Functions rather than a component's closures: two of the three are also
// `registerAction` bodies in `AppShell`, and a palette entry that quietly did
// something slightly different from the button beside it is a bug nobody
// reports.

import { forgeStore } from "../store/forgeStore";
import { requestHandoff, runtimeStore } from "../store/runtimeStore";
import { setSplitOpen, splitOpen } from "../store/sessionChangesStore";
import { openReview, showSession } from "../store/viewsStore";
import { focusWorkspace, workbenchStore } from "../store/workbenchStore";
import { selectSession } from "../runtime/api";
import { draftWithJuva, loadSessionTranscript, loadWorkspaceReview } from "../workbench/api";
import { sessionTabLabel } from "../runtime/attention";
import type { Session, Workspace } from "../runtime/types";
import { SESSION_SPLIT_OPEN_KEY, readFlag } from "./layout";
import { activeWorkspaceId, sessionsInWorkspace } from "./sessionScope";

export function activeSession(): Session | null {
  return forgeStore.sessions.find((item) => item.id === runtimeStore.activeSession) ?? null;
}

/**
 * The checkout the window is in: the scope of the tab strip, the launchers and
 * the inspector.
 *
 * Here rather than in `AppShell` because the `+` menu inside the strip needs
 * it too, and a menu that starts a terminal in a different checkout than the
 * strip it hangs off is the bug this scoping exists to prevent.
 */
export function currentWorkspace(): string | null {
  return activeWorkspaceId(
    forgeStore.sessions,
    runtimeStore.activeSession,
    workbenchStore.workspace,
    forgeStore.workspaces[0]?.id ?? null,
  );
}

/** The checkout of the session on screen; `null` when there is none. */
export function activeCheckout(): Workspace | null {
  const session = activeSession();
  if (!session) return null;
  return forgeStore.workspaces.find((item) => item.id === session.workspace_id) ?? null;
}

/** The checkout the window is pointed at, as a row. */
export function currentCheckout(): Workspace | null {
  const id = currentWorkspace();
  return forgeStore.workspaces.find((item) => item.id === id) ?? null;
}

/**
 * Go to a session: the window's checkout, the centre column and the daemon.
 *
 * One function because the three have to move together. The strip, the
 * launchers and the inspector are all scoped to `workbenchStore.workspace`, so
 * a selection that only told the daemon would leave the window pointed at the
 * checkout the user just left — and the reactive fallback in `RightPanel`
 * cannot close the gap on its own: re-selecting the session that is already
 * active is a no-op in the daemon, so nothing changes for an effect to observe,
 * and the click would do nothing at all.
 */
export function focusSession(session: string): void {
  showSession();
  const workspace = forgeStore.sessions.find((item) => item.id === session)?.workspace_id;
  if (workspace) focusWorkspace(workspace);
  void selectSession(session).catch(() => undefined);
}

/** Whether the split is showing for the session on screen. */
export function sessionChangesOpen(): boolean {
  const session = activeSession();
  return splitOpen(session?.id ?? null, readFlag(SESSION_SPLIT_OPEN_KEY, false));
}

export function toggleSessionChanges(): void {
  const session = activeSession();
  if (!session) return;
  setSplitOpen(session.id, !sessionChangesOpen());
}

/**
 * Open the handoff dialog and start reading the capture.
 *
 * The dialog opens first and fills in: the capture is a round trip, and a
 * spinner with no dialog around it reads as a click that did nothing.
 */
export function startHandoff(): void {
  const session = activeSession();
  const checkout = activeCheckout();
  if (!session || !checkout) return;
  requestHandoff({
    session: session.id,
    // Numbered within its checkout, exactly as the tab strip numbers it: a
    // dialog that calls the session "Terminal 3" while its tab says
    // "Terminal 1" is naming a session the user cannot find.
    title: sessionTabLabel(session, sessionsInWorkspace(forgeStore.sessions, checkout.id)),
    workspace: checkout.id,
    workingDirectory: checkout.path,
    branch: checkout.branch,
    sourceAgent: session.agent_provider_id,
  });
  void loadSessionTranscript(session.id).catch(() => undefined);
}

/**
 * Open, or regenerate, this checkout's review tab.
 *
 * The diff and the prose are two requests on purpose: the summary opens a
 * socket and travels the ack-then-event path, so the diff lands first and the
 * tab is useful before — and without — the prose.
 */
export function openCheckoutReview(): void {
  // The checkout the window is pointed at, not the active session's: with the
  // rail on a worktree whose terminals are all closed, those two differ, and
  // "review this checkout" means the one on screen.
  const checkout = currentCheckout();
  if (!checkout) return;
  // The workbench has to be pointed at this checkout before the answer lands,
  // or the event guard drops it as belonging to another one.
  focusWorkspace(checkout.id);
  openReview(checkout.id);
  void loadWorkspaceReview(checkout.id).catch(() => undefined);
  void draftWithJuva(checkout.id, "ChangeReview").catch(() => undefined);
}
