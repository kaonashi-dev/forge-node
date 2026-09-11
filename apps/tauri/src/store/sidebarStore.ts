import { createSignal } from "solid-js";
import {
  SIDEBAR_OPEN_KEY,
  SIDEBAR_VIEW_KEY,
  readChoice,
  readFlag,
  writeChoice,
  writeFlag,
} from "../shell/layout";

export const SIDEBAR_VIEWS = [
  "Projects",
  "Files",
  "History",
  "PR",
  "Features",
  "Lieutenant",
  "Git",
] as const;

export type SidebarView = (typeof SIDEBAR_VIEWS)[number];

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

/** Point the views at the stored choice. Called once the snapshot lands. */
export function restoreSidebar(): void {
  setView(readChoice(SIDEBAR_VIEW_KEY, SIDEBAR_VIEWS, "Projects"));
  setOpen(readFlag(SIDEBAR_OPEN_KEY, true));
}
