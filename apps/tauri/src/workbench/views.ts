// Which centre views a checkout has open .
//
// Keyed by workspace because that is what the views are *about*: leaving a
// worktree and coming back should find the diff and the file still there, and
// carrying them across would put one checkout's patch under another's branch
// name. The terminal is not in here — it is the floor the strip sits on and
// exists for every checkout.

export type WorkbenchView =
  | { kind: "terminal" }
  | { kind: "diff" }
  | { kind: "editor"; path: string }
  | { kind: "pr_detail"; key: string }
  | { kind: "pr_compose" }
  /**
   * One pull request being reviewed by an agent (§16.9).
   *
   * Keyed on the same string as the detail tab, so a second Review on the same
   * pull request comes back to the run already in flight rather than starting
   * a rival one — the reason the tab exists at all is to be somewhere to come
   * back to while the agent works.
   */
  | { kind: "pr_review"; key: string }
  /**
   * One checkout's changes since its sessions began, with a Juva summary.
   *
   * Keyed on the *workspace* and not on a session: it is deliberately about
   * every session that wrote this checkout, so pressing Review from a second
   * terminal regenerates the same tab rather than opening a rival one.
   */
  | { kind: "review"; workspace: string }
  /**
   * The draft a feature starts as: a spec with no number yet.
   *
   * Its own kind rather than a `feature` with `id: null`, because everything
   * the feature tab does — the gate, the steps, the documents — is keyed on a
   * number the daemon has not minted yet.
   */
  | { kind: "feature_compose" }
  /**
   * One harness feature: its gate, its steps, its agents and its documents.
   *
   * Identified by the feature *number*, which is what `features.json` keys on.
   * That number is only unique inside a repository — but so is this strip,
   * which is parked per checkout, so two projects can each have a `#3` open
   * without either finding the other's.
   */
  | { kind: "feature"; id: number };

export const PR_COMPOSE_VIEW: WorkbenchView = { kind: "pr_compose" };
export const FEATURE_COMPOSE_VIEW: WorkbenchView = { kind: "feature_compose" };

export type ParkedViews = {
  /** Open views, in the order they were opened, terminal excluded. */
  open: WorkbenchView[];
  /** What is on screen. */
  active: WorkbenchView;
};

export const TERMINAL_VIEW: WorkbenchView = { kind: "terminal" };

export function emptyViews(): ParkedViews {
  return { open: [], active: TERMINAL_VIEW };
}

/** A stable identity for a view, for keys and comparisons. */
export function viewKey(view: WorkbenchView): string {
  if (view.kind === "editor") return `editor:${view.path}`;
  if (view.kind === "pr_detail") return `pr:${view.key}`;
  if (view.kind === "pr_review") return `pr-review:${view.key}`;
  if (view.kind === "review") return `review:${view.workspace}`;
  if (view.kind === "pr_compose") return "pr_compose";
  if (view.kind === "feature_compose") return "feature_compose";
  if (view.kind === "feature") return `feature:${view.id}`;
  return view.kind;
}

export function sameView(a: WorkbenchView, b: WorkbenchView): boolean {
  return viewKey(a) === viewKey(b);
}

export function viewLabel(view: WorkbenchView): string {
  switch (view.kind) {
    case "terminal":
      return "Terminal";
    case "diff":
      return "Diff";
    case "editor":
      return view.path.slice(view.path.lastIndexOf("/") + 1);
    case "pr_detail":
      return "Pull Request";
    case "pr_review":
      return "PR Review";
    case "review":
      return "Review";
    case "pr_compose":
      return "Open PR";
    case "feature_compose":
      return "New Feature";
    case "feature":
      return `Feature #${view.id}`;
  }
}

/**
 * The whole of what a tab is about, for its tooltip.
 *
 * The tab itself shows the basename, because a strip of full paths is a strip
 * of identical prefixes — but a basename alone cannot tell two `mod.rs` apart,
 * so the path has to stay reachable.
 */
export function viewTitle(view: WorkbenchView): string {
  if (view.kind === "editor") return view.path;
  // Two review tabs are both called "PR Review"; the key is the only thing
  // that tells the reader which pull request each one is about.
  if (view.kind === "pr_review") return `Review ${view.key}`;
  return viewLabel(view);
}

/**
 * Open a view, or focus it when it is already open.
 *
 * Re-opening never duplicates and never reorders: a strip that reshuffles
 * itself as you revisit tabs is one you have to read before every click.
 */
export function openView(views: ParkedViews, view: WorkbenchView): ParkedViews {
  const already = views.open.some((item) => sameView(item, view));
  return {
    open: already ? views.open : [...views.open, view],
    active: view,
  };
}

/**
 * Close a view and choose what takes its place.
 *
 * The neighbour to the left, or the terminal when nothing is left — never the
 * one to the right, because closing a run of tabs left-to-right would then
 * walk the selection through every tab it is about to close.
 */
export function closeView(views: ParkedViews, view: WorkbenchView): ParkedViews {
  const index = views.open.findIndex((item) => sameView(item, view));
  if (index < 0) return views;
  const open = views.open.filter((item) => !sameView(item, view));
  if (!sameView(views.active, view)) return { open, active: views.active };
  const next = open[index - 1] ?? open[index] ?? TERMINAL_VIEW;
  return { open, active: next };
}

export function focusView(views: ParkedViews, view: WorkbenchView): ParkedViews {
  const known = view.kind === "terminal" || views.open.some((item) => sameView(item, view));
  return known ? { ...views, active: view } : views;
}

/**
 * The tabs inside the Code tab.
 *
 * The terminal is *not* one of them. It has its own tab in the window strip
 * next to the sessions, so putting it here too would give the same pane two
 * places to be selected from and no way to tell which one is current.
 */
export function strip(views: ParkedViews): WorkbenchView[] {
  return views.open;
}

/** Whether the Code tab has anything to show. */
export function hasCode(views: ParkedViews): boolean {
  return views.open.length > 0;
}

/**
 * Close everything but one view (§4.2 U10, `close_other_views`).
 *
 * The kept view becomes active whatever was active before: closing the others
 * from a tab's own menu is a statement about *that* tab.
 */
export function closeOthers(views: ParkedViews, keep: WorkbenchView): ParkedViews {
  const kept = views.open.filter((item) => sameView(item, keep));
  return { open: kept, active: kept[0] ?? TERMINAL_VIEW };
}

/** Close everything after `from` in the strip; `from` itself stays. */
export function closeToRight(views: ParkedViews, from: WorkbenchView): ParkedViews {
  const index = views.open.findIndex((item) => sameView(item, from));
  if (index < 0) return views;
  const open = views.open.slice(0, index + 1);
  const active = open.some((item) => sameView(item, views.active))
    ? views.active
    : (open[index] ?? TERMINAL_VIEW);
  return { open, active };
}

/**
 * The view one step along the strip, wrapping.
 *
 * `null` when there is nothing to move to — an empty strip, or one view, where
 * "next" and "current" are the same thing and moving would be a no-op that
 * looked like a broken shortcut.
 */
export function stepView(views: ParkedViews, delta: number): WorkbenchView | null {
  if (views.open.length < 2) return null;
  const index = views.open.findIndex((item) => sameView(item, views.active));
  // Not open yet — the terminal is active — so a step lands on an end.
  if (index < 0) return delta > 0 ? views.open[0] : views.open[views.open.length - 1];
  const count = views.open.length;
  return views.open[(((index + delta) % count) + count) % count];
}
