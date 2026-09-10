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
import { newAgent, newShell, selectSession } from "../runtime/api";
import {
  draftWithJuva,
  loadExternalTranscript,
  loadSessionTranscript,
  loadWorkspaceReview,
} from "../workbench/api";
import { sessionTabLabel } from "../runtime/attention";
import { focusTerminal } from "../terminal/focus";
import type { ExternalAgentSession, Session, Workspace } from "../runtime/types";
import { LAST_WORKSPACE_KEY, SESSION_SPLIT_OPEN_KEY, readFlag } from "./layout";
import { activeWorkspaceId, sessionsInWorkspace, storedWorkspaceId } from "./sessionScope";
import { recordTabFocus } from "./tabSwitcher";

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
 *
 * The caret goes too: asking for a terminal is asking to type in it.
 */
export function focusSession(session: string): void {
  showSession();
  // The focus ring Ctrl+Tab walks. Every user-driven focus goes through here,
  // which is what makes "the tab you just left" mean anything.
  recordTabFocus(session);
  const workspace = forgeStore.sessions.find((item) => item.id === session)?.workspace_id;
  if (workspace) focusWorkspace(workspace);
  void selectSession(session).catch(() => undefined);
  focusTerminal();
}

/**
 * Start a terminal, or an agent, and go to it.
 *
 * The centre column is raised before the ask rather than after it: the command
 * channel is one-way, so the new session's id never comes back, and a window
 * left on the Code tab goes on showing a file while the agent it just started
 * prints its first prompt behind it.
 *
 * A launch that belongs to another surface — a PR review, a compose draft, a
 * conflict resolver — calls `runtime/api` directly, because that surface is
 * where its output is meant to be read.
 */
export function launchShell(workspace: string | null = null): Promise<void> {
  showSession();
  return newShell(workspace);
}

export function launchAgent(...args: Parameters<typeof newAgent>): Promise<void> {
  showSession();
  return newAgent(...args);
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
    kind: "session",
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
 * The same dialog for a run found on disk (§13.5).
 *
 * The sibling of `startHandoff`, which reads a live terminal. Here the capture
 * is the transcript file, so the daemon reads it rather than the VT engine —
 * but the dialog, the prompt and the launch are the same.
 *
 * A run recorded in a directory Forge maps to a project but not to a workspace
 * row has no checkout to start in, so nothing happens: `handoffBlocked` is what
 * greys the gesture before it is reached.
 */
export function startExternalHandoff(session: ExternalAgentSession): void {
  const checkout = forgeStore.workspaces.find((item) => item.id === session.workspace_id);
  if (!checkout) return;
  requestHandoff({
    kind: "external",
    session: session.session_id,
    title: session.title,
    workspace: checkout.id,
    workingDirectory: checkout.path,
    branch: session.branch ?? checkout.branch,
    sourceAgent: session.provider,
    profile: session.profile_id,
  });
  void loadExternalTranscript(session.session_id, session.provider, session.profile_id).catch(
    () => undefined,
  );
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

/**
 * Open the window on the checkout it was left in. Called once, on the snapshot.
 *
 * After the snapshot because the stored id has to be checked against the
 * worktrees that still exist, and both arrive together. It yields to a live
 * session: with `sessions.persist_history` on, the daemon can put a terminal
 * back on screen before this runs, and the checkout that terminal is in is a
 * better answer than the one a previous launch was reading.
 */
export function restoreWorkspace(): void {
  if (workbenchStore.workspace) return;
  const stored = storedWorkspaceId(forgeStore.app_state[LAST_WORKSPACE_KEY], forgeStore.workspaces);
  if (stored) focusWorkspace(stored);
}
