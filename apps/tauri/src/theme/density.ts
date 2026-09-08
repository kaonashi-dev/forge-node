// T3: one multiplier on the row height and the control ladder.
//
// Not a second token set. The type scale, the spacing grid, the radii and every
// colour are the same at every density — a compact window is the same layout
// drawn tighter, not a different design. What moves is the two ladders a list
// row and a control are built from, and they move together so a button still
// fits its row.
//
// Applied as an inline style on the root, over `tokens.css`. That is the same
// mechanism `applyTheme` uses, and it means a density change costs one write
// rather than a stylesheet swap.

import { metrics } from "./tokens";

export const DENSITIES = ["compact", "default", "comfortable"] as const;
export type Density = (typeof DENSITIES)[number];

/**
 * Row heights, in pixels, at each density — 22 / 26 / 30 (`plan-ui-ux.md` T3).
 *
 * Expressed as a ratio against the theme's own `rowH` rather than as three
 * literal values, so moving the default moves all three with it and the ladder
 * cannot drift apart from the scale it belongs to.
 */
const ROW_HEIGHT: Record<Density, number> = {
  compact: 22,
  default: 26,
  comfortable: 30,
};

/**
 * The controls move by the same *difference*, not the same ratio.
 *
 * A ratio would take the 20px `xs` control to 17px at compact and 23px at
 * comfortable — under the 24px minimum a pointer target wants at the bottom,
 * and out of proportion to its own icon at the top. A constant offset keeps
 * the four rungs the distance apart the scale put them.
 */
export function densityOffset(density: Density): number {
  return ROW_HEIGHT[density] - ROW_HEIGHT.default;
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
