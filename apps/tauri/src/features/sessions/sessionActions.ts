// Session gestures shared by the overlay, the session menu, the command palette
// and the `registerAction` bodies in `AppShell`.
//
// Functions rather than a component's closures, so a palette entry cannot
// quietly do something slightly different from the button beside it.

import { forgeStore } from "../../state/forgeStore";
import { beginSessionLaunch, connectionStore } from "../../state/connection";
import { requestHandoff, requestRunSpec, requestSendContext, requestSpawnChild } from "./dialogs";
import { setSplitOpen, splitOpen } from "../git/sessionChangesStore";
import {
  centerMode,
  currentViews,
  openCompare,
  openReview,
  showSession,
} from "../../navigation/viewsStore";
import {
  centerSplit,
  closeSplit,
  focusSplitPane,
  openCodeSplit,
} from "../../navigation/centerSplitStore";
import { canSplitCenter, visibleSplit } from "../../navigation/centerSplit";
import {
  newAgent,
  newShell,
  runSpec,
  restartSession,
  selectSession,
  splitShell,
  detachSplit,
} from "./commands";
import { reopenTerminalEditor } from "../editor/cells/commands";
import { sessionTabLabel } from "./attention";
import { focusTerminal } from "../terminal/focus";
import { hasExited } from "./exited";
import { type ExternalAgentSession, type Session, type Workspace } from "../../contracts/runtime";
import { LAST_WORKSPACE_KEY, SESSION_SPLIT_OPEN_KEY, readFlag } from "../../state/preferences";
import { activeWorkspaceId, sessionsInWorkspace, storedWorkspaceId } from "./sessionScope";
import { loadBranchCompare } from "../git/commands";
import { startReview } from "../review/commands";
import { setReviewStore } from "../review/state";
import { loadExternalTranscript, loadSessionTranscript } from "./commands";
import { activeWorkspace, focusWorkspace } from "../../state/workspace";

export function activeSession(): Session | null {
  return forgeStore.sessions.find((item) => item.id === connectionStore.activeSession) ?? null;
}

/**
 * The checkout the window is in: the scope of the tab strip, the launchers and
 * the sidebar views.
 *
 * Here rather than in `AppShell` because the `+` menu inside the strip needs
 * it too, and a menu that starts a terminal in a different checkout than the
 * strip it hangs off is the bug this scoping exists to prevent.
 */
export function currentWorkspace(): string | null {
  return activeWorkspaceId(
    forgeStore.sessions,
    connectionStore.activeSession,
    activeWorkspace(),
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
 * launchers and the sidebar views are all scoped to `activeWorkspace()`,
 * so a selection that only told the daemon would leave the window pointed at
 * the checkout the user just left — and the reactive fallback in `AppShell`
 * cannot close the gap on its own: re-selecting the session that is already
 * active is a no-op in the daemon, so nothing changes for an effect to observe,
 * and the click would do nothing at all.
 *
 * The caret goes too: asking for a terminal is asking to type in it.
 */
export function focusSession(session: string): void {
  const row = forgeStore.sessions.find((item) => item.id === session);
  // An editor is not a terminal tab: it lives in the Code strip, so focusing it
  // opens the file there rather than raising the main terminal.
  if (row?.kind === "Editor") {
    if (row.workspace_id) focusWorkspace(row.workspace_id);
    void reopenTerminalEditor(row.id).catch(() => undefined);
    return;
  }
  const split = visibleSplit(centerSplit(), false, centerMode(), currentViews().active);
  if (split === "session" && centerSplit().extra === session) {
    focusSplitPane("extra");
    if (row?.workspace_id) focusWorkspace(row.workspace_id);
    focusTerminal(session);
    return;
  }
  // Opening an exited session means running it again; the host follows the
  // restart onto its new terminal.
  if (row && hasExited(row)) void restartSession(session).catch(() => undefined);
  // Before `showSession`: raising the centre column wakes the focus-ring
  // effect, and the pending selection is what stops it recording the terminal
  // this call is leaving.
  else void selectSession(session).catch(() => undefined);
  if (split === "code") {
    // The file stays; this tab is the terminal column beside it.
    focusSplitPane("extra");
  } else if (split === "session") {
    focusSplitPane("primary");
  } else {
    showSession();
  }
  if (row?.workspace_id) focusWorkspace(row.workspace_id);
  focusTerminal(centerSplit().extra === session ? session : "main");
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
 * conflict resolver — calls `features/sessions/commands` directly, because that
 * surface is where its output is meant to be read. Those need no launch guard:
 * nothing raises the centre column ahead of the daemon's answer, so the focus
 * ring first hears about the session when it is genuinely the active one.
 */
export function launchShell(workspace: string | null = null): Promise<void> {
  const cancel = beginSessionLaunch();
  showSession();
  return newShell(workspace).catch((error: unknown) => {
    cancel();
    throw error;
  });
}

export function launchAgent(...args: Parameters<typeof newAgent>): Promise<void> {
  const cancel = beginSessionLaunch();
  showSession();
  return newAgent(...args).catch((error: unknown) => {
    cancel();
    throw error;
  });
}

export function launchSpec(...args: Parameters<typeof runSpec>): Promise<void> {
  const cancel = beginSessionLaunch();
  showSession();
  return runSpec(...args).catch((error: unknown) => {
    cancel();
    throw error;
  });
}

const SPEC_FILE = /\.(md|markdown|mdx|txt)$/i;

/** Open "Run Spec…" on this checkout, with the Markdown file on screen as the spec. */
export function startRunSpec(): void {
  const workspace = currentWorkspace();
  if (!workspace) return;
  const view = currentViews().active;
  const path = "path" in view && SPEC_FILE.test(view.path) ? view.path : "";
  requestRunSpec({ workspace, spec: path });
}

/** Whether the split is showing for the session on screen. */
export function sessionChangesOpen(): boolean {
  const session = activeSession();
  return splitOpen(session?.id ?? null, readFlag(SESSION_SPLIT_OPEN_KEY, false));
}

/**
 * Cmd+D: a terminal beside this terminal, or beside the open file.
 *
 * A second press is a no-op — there is one extra column, not a tree of
 * them. Diff, search and pull-request views are not a split surface.
 */
export function splitPane(): void {
  if (centerSplit().kind !== "closed") return;
  if (!canSplitCenter(centerMode(), currentViews().active)) return;
  if (centerMode() === "code") {
    openCodeSplit();
    if (!connectionStore.activeSession) {
      void newShell(currentWorkspace()).catch(() => undefined);
    }
    return;
  }
  void splitShell(currentWorkspace()).catch(() => undefined);
}

export function joinPanes(): void {
  const extra = centerSplit().extra;
  closeSplit();
  if (extra) void detachSplit(extra).catch(() => undefined);
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

/** Open the spawn-child dialog for the session on screen. */
export function startSpawnChild(): void {
  const session = activeSession();
  const checkout = activeCheckout();
  if (!session || !checkout) return;
  requestSpawnChild({
    parent: session.id,
    title: sessionTabLabel(session, sessionsInWorkspace(forgeStore.sessions, checkout.id)),
  });
}

/** Open the send-context dialog for the session on screen. */
export function startSendContext(): void {
  const session = activeSession();
  const checkout = activeCheckout();
  if (!session || !checkout) return;
  requestSendContext({
    source: session.id,
    title: sessionTabLabel(session, sessionsInWorkspace(forgeStore.sessions, checkout.id)),
  });
}

/**
 * Handoff for a discovered transcript.
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

/** Review the checkout the window is pointed at, which with every terminal
 *  of a worktree closed is not the active session's. */
export function reviewCurrentWorkspace(): void {
  const checkout = currentCheckout();
  if (checkout) void startReview(checkout.id).catch(() => undefined);
}

/** Open the review preset picker for the same checkout. */
export function reviewWithPreset(): void {
  const checkout = currentCheckout();
  if (checkout) setReviewStore("picker", checkout.id);
}

/** Open this checkout's PR preview: its branch against the default branch. */
export function openPullRequestPreview(): void {
  const checkout = currentCheckout();
  if (!checkout) return;
  // Pointed first, for the same reason as the review: the answer is dropped
  // unless it belongs to the focused workspace.
  focusWorkspace(checkout.id);
  openCompare(checkout.id);
  void loadBranchCompare(checkout.id).catch(() => undefined);
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
  if (activeWorkspace()) return;
  const stored = storedWorkspaceId(forgeStore.app_state[LAST_WORKSPACE_KEY], forgeStore.workspaces);
  if (stored) focusWorkspace(stored);
}
