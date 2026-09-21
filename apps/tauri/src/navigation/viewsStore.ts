import { createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { activeWorkspace } from "../state/workspace";
import {
  closeView,
  emptyViews,
  focusView,
  openView,
  sameView,
  retargetView,
  retargetViewPath,
  retargetViews,
  stepView,
  PR_COMPOSE_VIEW,
  TERMINAL_VIEW,
  type ParkedViews,
  type WorkbenchView,
} from "./views";

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

export function showCode(): void {
  setMode("code");
}

/** Back to the active session's terminal. */
export function showSession(): void {
  setMode("session");
}

export function codeOpen(): boolean {
  return activeWorkspace() !== null;
}

/** The views of the checkout the workbench is pointed at. */
export function currentViews(): ParkedViews {
  const workspace = activeWorkspace();
  return (workspace && store.byWorkspace[workspace]) || emptyViews();
}

function update(change: (views: ParkedViews) => ParkedViews): void {
  const workspace = activeWorkspace();
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
export function open(view: WorkbenchView): void {
  update((views) => openView(views, view));
  if (activeWorkspace()) setMode("code");
}

export function openDiff(): void {
  open({ kind: "diff" });
}

export function openEditorTerminal(
  session: string,
  path: string,
  workspace = activeWorkspace(),
): void {
  if (!workspace) return;
  setStore(
    "byWorkspace",
    workspace,
    openView(store.byWorkspace[workspace] ?? emptyViews(), {
      kind: "editor-terminal",
      session,
      path,
    }),
  );
  if (workspace === activeWorkspace()) setMode("code");
}

export function openProjectSearch(): void {
  open({ kind: "search" });
}

export function openPrDetail(key: string): void {
  open({ kind: "pr_detail", key });
}

export function openPrReview(key: string): void {
  open({ kind: "pr_review", key });
}

export function openPrCompose(): void {
  open(PR_COMPOSE_VIEW);
}

export function openReview(workspace: string): void {
  open({ kind: "review", workspace });
}

export function focus(view: WorkbenchView): void {
  update((views) => focusView(views, view));
  setMode("code");
}

export function removeViews(workspace: string, targets: WorkbenchView[]): void {
  for (const view of targets) if (activeWorkspace() === workspace) remember(view);
  setStore("byWorkspace", workspace, (views) => targets.reduce(closeView, views));
}

export function showTerminal(): void {
  update((views) => focusView(views, TERMINAL_VIEW));
  setMode("session");
}

/**
 * Closed views in this workspace, most recent last.
 *
 * Per checkout, like the strip itself, and capped: "reopen the last thing I
 * closed" is a few steps of undo, not a session history, and an unbounded list
 * of paths for a checkout nobody has visited in an hour is memory spent on
 * nothing.
 */
const CLOSED_DEPTH = 16;
const [closedByWorkspace, setClosedByWorkspace] = createStore<Record<string, WorkbenchView[]>>({});

function remember(view: WorkbenchView): void {
  const workspace = activeWorkspace();
  // The terminal is never closed, and re-opening a diff is a refresh, not a
  // restoration — only a file has a place to come back to.
  if (!workspace || view.kind === "terminal" || view.kind === "editor-terminal") return;
  const kept = (closedByWorkspace[workspace] ?? []).filter((item) => !sameView(item, view));
  setClosedByWorkspace(workspace, [...kept, view].slice(-CLOSED_DEPTH));
}

/** Bring back the last view closed in this checkout. */
export function reopenClosed(): void {
  const workspace = activeWorkspace();
  if (!workspace) return;
  const stack = closedByWorkspace[workspace] ?? [];
  const view = stack.at(-1);
  if (!view) return;
  setClosedByWorkspace(workspace, stack.slice(0, -1));
  open(view);
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
 * mounted: the sidebar can be collapsed, or on another view. The request
 * stands until the tree picks it up, and the tree clears it.
 */
const [pendingReveal, setPendingReveal] = createSignal<{ workspace: string; path: string } | null>(
  null,
);

export const treeReveal = () => {
  const reveal = pendingReveal();
  return reveal?.workspace === activeWorkspace() ? reveal.path : null;
};

export function revealInTree(path: string): void {
  const workspace = activeWorkspace();
  if (workspace) setPendingReveal({ workspace, path });
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

// The lazy Search tab consumes the focus request after its input mounts.
const [pendingFindInFiles, setPendingFindInFiles] = createSignal<{ query: string | null } | null>(
  null,
);

export const findInFilesPending = pendingFindInFiles;

export function requestFindInFiles(query: string | null = null): void {
  if (!activeWorkspace()) return;
  setPendingFindInFiles({ query });
  openProjectSearch();
}

export function clearFindInFiles(): void {
  setPendingFindInFiles(null);
}

export function retargetWorkspaceViews(workspace: string, from: string, to: string): void {
  const views = store.byWorkspace[workspace];
  if (views) setStore("byWorkspace", workspace, retargetViews(views, from, to));
  const closed = closedByWorkspace[workspace];
  if (closed)
    setClosedByWorkspace(
      workspace,
      closed.map((view) => retargetView(view, from, to)),
    );
  setPendingReveal((reveal) =>
    reveal?.workspace === workspace
      ? { ...reveal, path: retargetViewPath(reveal.path, from, to) }
      : reveal,
  );
  if (workspace !== activeWorkspace()) return;
  setPendingLine((reveal) =>
    reveal === null ? null : { ...reveal, path: retargetViewPath(reveal.path, from, to) },
  );
}

export function retargetEditorViewPaths(paths: ReadonlyMap<string, string>): void {
  const sync = (view: WorkbenchView): WorkbenchView => {
    if (view.kind !== "editor-terminal") return view;
    const path = paths.get(view.session);
    return path && path !== view.path ? { ...view, path } : view;
  };
  for (const [workspace, views] of Object.entries(store.byWorkspace)) {
    const open = views.open.map(sync);
    const active = sync(views.active);
    if (active !== views.active || open.some((view, index) => view !== views.open[index])) {
      setStore("byWorkspace", workspace, { open, active });
    }
  }
}
