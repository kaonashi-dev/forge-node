import type { ReviewSettings } from "../../contracts/workbench";

export function presetKey(projectId: string): string {
  return `ui.review.preset.${projectId}`;
}

/** The remembered preset while the repository still lists it, else its default. */
export function initialPreset(settings: ReviewSettings, remembered: string | undefined): string {
  return remembered !== undefined && settings.presets.some((preset) => preset.id === remembered)
    ? remembered
    : settings.default_preset;
}
