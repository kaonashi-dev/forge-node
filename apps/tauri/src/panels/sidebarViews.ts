import type { SidebarView } from "../store/sidebarStore";
import type { ForgeIconName } from "../theme/icons";

export const VIEW_ICONS: Record<SidebarView, ForgeIconName> = {
  Projects: "layers",
  Files: "folder",
  History: "history",
  PR: "git-pull-request",
  Features: "layout-grid",
  Git: "git-branch",
};
