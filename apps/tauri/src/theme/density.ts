// One offset on the row height and the control ladder. Type, spacing, radii
// and colour are the same at every density; they move together so a button
// still fits its row.

import { metrics } from "./tokens";

export const DENSITIES = ["compact", "cozy"] as const;
export type Density = (typeof DENSITIES)[number];

export const DEFAULT_DENSITY: Density = "compact";

export const DENSITY_LABELS: Record<Density, string> = { compact: "Compact", cozy: "Cozy" };

export function isDensity(value: string | undefined): value is Density {
  return value !== undefined && (DENSITIES as readonly string[]).includes(value);
}

// Stored preferences predate the two-step scale.
const LEGACY: Record<string, Density> = { default: "compact", comfortable: "cozy" };

/** A stored value as a density, mapping the retired three-step ids. */
export function readDensity(value: string | undefined): Density {
  if (isDensity(value)) return value;
  return (value !== undefined && LEGACY[value]) || DEFAULT_DENSITY;
}

const ROW_HEIGHT: Record<Density, number> = {
  compact: metrics.rowH,
  cozy: 32,
};

/**
 * The controls move by the same *difference*, not the same ratio, so the four
 * rungs stay the distance apart the scale put them.
 */
export function densityOffset(density: Density): number {
  return ROW_HEIGHT[density] - metrics.rowH;
}

/** The variables a density writes. Pure, so a test can read them. */
export function densityTokens(density: Density): Record<string, string> {
  const offset = densityOffset(density);
  return {
    "--forge-row-h": `${ROW_HEIGHT[density]}px`,
    "--forge-control-xs": `${metrics.controlXS + offset}px`,
    "--forge-control-sm": `${metrics.controlSM + offset}px`,
    "--forge-control-md": `${metrics.controlMD + offset}px`,
    "--forge-control-lg": `${metrics.controlLG + offset}px`,
  };
}

export function applyDensity(density: Density): void {
  const root = document.documentElement;
  for (const [name, value] of Object.entries(densityTokens(density))) {
    root.style.setProperty(name, value);
  }
  root.dataset.density = density;
}
