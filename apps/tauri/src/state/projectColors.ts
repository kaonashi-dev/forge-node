import { forgeStore } from "./forgeStore";
import { writeChoice } from "./preferences";

/**
 * A project's colour: one of the theme's nine hues, chosen per project from
 * its menu and kept in `app_state` beside the other UI preferences, so it is
 * no column and no migration. A project nobody coloured wears the theme's
 * default hue.
 */
export const PROJECT_HUES_KEY = "ui.project_hues";

/** Mirrors `HUE_COUNT` in `theme/tokens.ts`, which this layer may not import. */
export const PROJECT_HUE_COUNT = 9;

/** The stored map, dropping anything that is not a hue slot. Pure, so a test can read it. */
export function parseProjectHues(raw: string | undefined): Record<string, number> {
  if (!raw) return {};
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return {};
  const out: Record<string, number> = {};
  for (const [id, slot] of Object.entries(value)) {
    if (Number.isInteger(slot) && slot >= 0 && slot < PROJECT_HUE_COUNT) out[id] = slot;
  }
  return out;
}

// Rows ask once per render; the map is re-parsed only when the stored string changes.
let lastRaw: string | undefined;
let lastMap: Record<string, number> = {};
function stored(): Record<string, number> {
  const raw = forgeStore.app_state[PROJECT_HUES_KEY];
  if (raw !== lastRaw) {
    lastRaw = raw;
    lastMap = parseProjectHues(raw);
  }
  return lastMap;
}

/** The slot a project was given, or `null` for the theme default. */
export function projectHueChoice(projectId: string): number | null {
  return stored()[projectId] ?? null;
}

/** The CSS value of a project's hue, for `--project-hue` on the element that wears it. */
export function projectHue(projectId: string): string {
  const slot = projectHueChoice(projectId);
  return slot === null ? "var(--hue-default)" : `var(--hue-${slot})`;
}

/** `null` returns the project to the theme default. */
export function setProjectHue(projectId: string, slot: number | null): void {
  const next = { ...stored() };
  if (slot === null) delete next[projectId];
  else next[projectId] = slot;
  writeChoice(PROJECT_HUES_KEY, JSON.stringify(next));
}
