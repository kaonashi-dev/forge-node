/** Settings pages, in the order the sidebar lists them. */
export const SETTINGS_SECTIONS = [
  "General",
  "Agents",
  "Keyboard",
  "Shared files",
  "Stats & usage",
  "Appearance",
] as const;

export type Section = (typeof SETTINGS_SECTIONS)[number];
