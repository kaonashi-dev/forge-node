import { createSignal } from "solid-js";
import {
  SIDEBAR_OPEN_KEY,
  SIDEBAR_VIEW_KEY,
  readChoice,
  readFlag,
  writeChoice,
  writeFlag,
} from "../shell/layout";

/** Stored verbatim under `ui.sidebar.view`: renaming one sends everyone back to Projects. */
export const SIDEBAR_VIEWS = ["Projects", "Files", "History", "PR", "Features", "Git"] as const;

export type SidebarView = (typeof SIDEBAR_VIEWS)[number];

/** The views with a number chord of their own; MOD-3 walks the rest. */
const NUMBERED_VIEWS: readonly SidebarView[] = ["Projects", "Files"];

export const CYCLED_VIEWS: readonly SidebarView[] = SIDEBAR_VIEWS.filter(
  (item) => !NUMBERED_VIEWS.includes(item),
);

const [view, setView] = createSignal<SidebarView>("Projects");
const [open, setOpen] = createSignal(true);

/*
 * Window state, not the container's own: the shortcuts that name a view have
 * to work while the bar is collapsed, and a signal that only exists while the
 * container is mounted cannot answer them.
 */
export const sidebarView = view;
export const sidebarOpen = open;

/** Choose a view and remember it. The read is deferred to `restoreSidebar`. */
export function setSidebarView(next: SidebarView): void {
  if (view() === next) return;
  setView(next);
  writeChoice(SIDEBAR_VIEW_KEY, next);
}

export function setSidebarOpen(next: boolean): void {
  if (open() === next) return;
  setOpen(next);
  writeFlag(SIDEBAR_OPEN_KEY, next);
}

export function toggleSidebar(): void {
  setSidebarOpen(!open());
}

/** Show a view, opening the bar if needed. Never closes it. */
export function showView(next: SidebarView): void {
  setSidebarView(next);
  setSidebarOpen(true);
}

/** A view's own chord: a second press on the view already up closes the bar. */
export function toggleView(next: SidebarView): void {
  if (open() && view() === next) setSidebarOpen(false);
  else showView(next);
}

/**
 * Where MOD-3 goes from here. A press made off the cycle — the bar closed, or
 * on a view with its own number — always starts it at the first view; only a
 * press made while on it advances.
 */
export function cycleTarget(current: SidebarView, open: boolean): SidebarView {
  const index = CYCLED_VIEWS.indexOf(current);
  if (!open || index < 0) return CYCLED_VIEWS[0];
  return CYCLED_VIEWS[(index + 1) % CYCLED_VIEWS.length];
}

export function cycleSidebarView(): void {
  showView(cycleTarget(view(), open()));
}

/** Point the views at the stored choice. Called once the snapshot lands. */
export function restoreSidebar(): void {
  setView(readChoice(SIDEBAR_VIEW_KEY, SIDEBAR_VIEWS, "Projects"));
  setOpen(readFlag(SIDEBAR_OPEN_KEY, true));
}
