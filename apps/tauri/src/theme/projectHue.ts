import { HUE_COUNT } from "./tokens";

/**
 * Which of the nine hues a project wears: FNV-1a over its id, mod nine.
 *
 * Derived, never stored — no column, no migration — so it has to be the same
 * answer on every launch and in every window, and two projects may share one.
 */
export function projectHueIndex(projectId: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < projectId.length; index += 1) {
    hash ^= projectId.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % HUE_COUNT;
}

/** The CSS value of a project's hue, for `--project-hue` on the element that wears it. */
export function projectHue(projectId: string): string {
  return `var(--hue-${projectHueIndex(projectId)})`;
}
