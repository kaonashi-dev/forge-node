import { createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { workbenchStore } from "./workbenchStore";
import {
  closeOthers,
  closeToRight,
  closeView,
  emptyViews,
  focusView,
  hasCode,
  openView,
  sameView,
  stepView,
  FEATURE_COMPOSE_VIEW,
  PR_COMPOSE_VIEW,
  TERMINAL_VIEW,
  type ParkedViews,
  type WorkbenchView,
} from "../workbench/views";

/**
 * Centre views, remembered per checkout (parked centre views).
 *
 * Kept here rather than in the component so leaving a worktree and coming back
 * finds the same tabs: the views are about the checkout, not about the pane
 * that happened to be mounted.
 */
const [store, setStore] = createStore<{ byWorkspace: Record<string, ParkedViews> }>({
  byWorkspace: {},
});

export const viewsStore = store;

/**
 * Which of the window's own tabs is up: a session's terminal, or Code.
 *
 * Window-level rather than per-checkout, because it is about what the person
 * is looking at right now and not about the checkout: switching worktrees
 * while reading a file should land on that worktree's files, not throw the
 * reader back into a terminal.
 */
const [mode, setMode] = createSignal<CenterMode>("session");

export type CenterMode = "session" | "code";

/** What the centre column is showing. */
export const centerMode = mode;

/** Show the Code tab — only meaningful once something is open in it. */
export function showCode(): void {
  setMode("code");
}

/** Back to the active session's terminal. */
export function showSession(): void {
  setMode("session");
}

/** Does the Code tab exist right now? */
export function codeOpen(): boolean {
  return hasCode(currentViews());
}

/** The views of the checkout the workbench is pointed at. */
export function currentViews(): ParkedViews {
  const workspace = workbenchStore.workspace;
  return (workspace && store.byWorkspace[workspace]) || emptyViews();
}

function update(change: (views: ParkedViews) => ParkedViews): void {
  const workspace = workbenchStore.workspace;
  if (!workspace) return;
  setStore("byWorkspace", workspace, change(currentViews()));
}

/**
 * Open a view and raise the Code tab.
 *
 * Opening is always a request to *look* at the thing: leaving the terminal up
 * with a tab quietly appearing beside it is how a click on a file reads as
 * having done nothing.
 */
function open(view: WorkbenchView): void {
  update((views) => openView(views, view));
  if (workbenchStore.workspace) setMode("code");
}

export function openDiff(): void {
  open({ kind: "diff" });
}

export function openEditor(path: string): void {
  open({ kind: "editor", path });
}

/**
 * Open a file *at* a line: a diff hunk, a review note, a definition.
 *
 * The line is a request that stands until the editor honours it, not part of
 * the view — `viewKey` is the path alone, so a second jump into a file already
 * open moves the caret in the tab that is there rather than opening a rival
 * one, and a parked tab reopened later is not stuck on an old line.
 */
export function openEditorAt(path: string, line: number): void {
  openEditor(path);
  setPendingLine({ path, line });
  revealInTree(path);
}

export function openPrDetail(key: string): void {
  open({ kind: "pr_detail", key });
}

/** Open, or come back to, the agent review of one pull request (§16.9). */
export function openPrReview(key: string): void {
  open({ kind: "pr_review", key });
}

export function openPrCompose(): void {
  open(PR_COMPOSE_VIEW);
}

/** Open, or re-focus, this checkout's review tab (§16.7). */
export function openReview(workspace: string): void {
  open({ kind: "review", workspace });
}

export function openFeatureView(id: number): void {
  open({ kind: "feature", id });
}

export function openFeatureCompose(): void {
  open(FEATURE_COMPOSE_VIEW);
}

export function focus(view: WorkbenchView): void {
  update((views) => focusView(views, view));
  setMode("code");
}

/**
 * Close one view, and fall back to the session when it was the last.
 *
 * An empty Code tab is not a place to be: the tab itself disappears with the
 * last view in it, so the window has to have somewhere to put the person.
 */
export function close(view: WorkbenchView): void {
  remember(view);
  update((views) => closeView(views, view));
  if (!hasCode(currentViews())) setMode("session");
}

/** Close every view in the Code tab and go back to the session. */
export function closeCode(): void {
  for (const view of currentViews().open) remember(view);
  update(() => emptyViews());
  setMode("session");
}

export function showTerminal(): void {
  update((views) => focusView(views, TERMINAL_VIEW));
  setMode("session");
}

/**
 * Views closed in this workspace, most recent last (§4.2 U10).
 *
 * Per checkout, like the strip itself, and capped: "reopen the last thing I
 * closed" is a few steps of undo, not a session history, and an unbounded list
 * of paths for a checkout nobody has visited in an hour is memory spent on
 * nothing.
 */
const CLOSED_DEPTH = 16;
const [closedByWorkspace, setClosedByWorkspace] = createStore<Record<string, WorkbenchView[]>>({});

function remember(view: WorkbenchView): void {
  const workspace = workbenchStore.workspace;
  // The terminal is never closed, and re-opening a diff is a refresh, not a
  // restoration — only a file has a place to come back to.
  if (!workspace || view.kind === "terminal") return;
  const kept = (closedByWorkspace[workspace] ?? []).filter((item) => !sameView(item, view));
  setClosedByWorkspace(workspace, [...kept, view].slice(-CLOSED_DEPTH));
}

/** Bring back the last view closed in this checkout. */
export function reopenClosed(): void {
  const workspace = workbenchStore.workspace;
  if (!workspace) return;
  const stack = closedByWorkspace[workspace] ?? [];
  const view = stack.at(-1);
  if (!view) return;
  setClosedByWorkspace(workspace, stack.slice(0, -1));
  open(view);
}

/** Close every view but the active one. */
export function closeOtherViews(): void {
  const views = currentViews();
  for (const view of views.open) if (!sameView(view, views.active)) remember(view);
  update((current) => closeOthers(current, current.active));
  if (!hasCode(currentViews())) setMode("session");
}

/** Close everything after the active view in the strip. */
export function closeViewsToRight(): void {
  const views = currentViews();
  const index = views.open.findIndex((item) => sameView(item, views.active));
  if (index >= 0) for (const view of views.open.slice(index + 1)) remember(view);
  update((current) => closeToRight(current, current.active));
}

/** Move one step along the Code strip, wrapping. */
export function stepCodeView(delta: number): void {
  const next = stepView(currentViews(), delta);
  if (next) focus(next);
}

/**
 * A7: the directory the file tree should show and select.
 *
 * A signal rather than a call into the panel, because the panel may not be
 * mounted: the rail can be collapsed, or the inspector on another tab. The
 * request stands until the tree picks it up, and the tree clears it.
 */
const [pendingReveal, setPendingReveal] = createSignal<string | null>(null);

export const treeReveal = pendingReveal;

export function revealInTree(path: string): void {
  setPendingReveal(path);
}

export function clearTreeReveal(): void {
  setPendingReveal(null);
}

/**
 * The line the editor should put the caret on, and for which file.
 *
 * The same shape as `treeReveal` and for the same reason: the editor for that
 * path may not be mounted yet — opening it is what mounts it — and the read
 * that gives it a document has not landed either. The request stands until the
 * editor takes it, and the editor clears it.
 */
const [pendingLine, setPendingLine] = createSignal<{ path: string; line: number } | null>(null);

export const editorReveal = pendingLine;

export function clearEditorReveal(): void {
  setPendingLine(null);
}
