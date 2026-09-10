import type { InspectorTab } from "../store/inspectorStore";
import type { ForgeIconName } from "../theme/icons";

/**
 * The glyph each inspector tab draws.
 *
 * A `Record`, not a partial map: adding a seventh tab should be a type error
 * here, not a blank square in the strip.
 */
export const TAB_ICONS: Record<InspectorTab, ForgeIconName> = {
  History: "history",
  PR: "git-pull-request",
  Features: "layout-grid",
  Lieutenant: "agent",
  Files: "folder",
  Git: "git-branch",
};
