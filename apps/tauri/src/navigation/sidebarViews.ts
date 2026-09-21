import type { SidebarView } from "./sidebarStore";
import type { ForgeIconName } from "../theme/icons/index";

export const VIEW_ICONS: Record<SidebarView, ForgeIconName> = {
  Projects: "layers",
  Files: "folder",
  History: "history",
  PR: "git-pull-request",
  Git: "git-branch",
};
