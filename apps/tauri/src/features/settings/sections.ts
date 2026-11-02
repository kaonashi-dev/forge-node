import type { ForgeIconName } from "../../theme/icons/index";

/** Settings pages, in the order the sidebar lists them. */
export const SETTINGS_SECTIONS = [
  "General",
  "Agents",
  "Keyboard",
  "Shared files",
  "Storage",
  "Stats & usage",
  "Appearance",
] as const;

export type Section = (typeof SETTINGS_SECTIONS)[number];

export const SECTION_ICONS: Record<Section, ForgeIconName> = {
  General: "settings",
  Agents: "agent",
  Keyboard: "keyboard",
  "Shared files": "folder-symlink",
  Storage: "folder",
  "Stats & usage": "stats",
  Appearance: "appearance",
};
