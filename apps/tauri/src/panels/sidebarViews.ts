import type { SidebarView } from "../store/sidebarStore";
import type { ForgeIconName } from "../theme/icons";

/**
 * The glyph each sidebar view draws.
 *
 * A `Record`, not a partial map: adding an eighth view should be a type error
 * here, not a blank square in the strip.
 */
export const VIEW_ICONS: Record<SidebarView, ForgeIconName> = {
  Projects: "layers",
  Files: "folder",
  History: "history",
  PR: "git-pull-request",
  Features: "layout-grid",
  Lieutenant: "agent",
  Git: "git-branch",
};
