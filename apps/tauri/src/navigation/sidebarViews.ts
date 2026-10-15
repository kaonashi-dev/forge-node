import type { SidebarView } from "./sidebarStore";
import type { ForgeIconName } from "../theme/icons/index";

export const VIEW_ICONS: Record<SidebarView, ForgeIconName> = {
  Projects: "layers",
  Files: "folder",
  History: "history",
  PR: "git-pull-request",
  Git: "git-branch",
};

/** The switcher lives in the title bar and the panels in the sidebar, so ARIA links them by id. */
export function sidebarTabId(view: SidebarView): string {
  return `sidebar-tab-${view}`;
}

export function sidebarPanelId(view: SidebarView): string {
  return `sidebar-panel-${view}`;
}
