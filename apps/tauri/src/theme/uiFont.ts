// One multiplier on the chrome type scale.
//
// Density already moves row height and the control ladder, and it is
// deliberately *not* this: a compact window is the same type at tighter
// spacing. The person who wants larger tabs and labels is asking for a
// different number, and the editor and the terminal keep the sizes they
// already own so a bigger shell does not blow up a cell grid.

import { metrics } from "./tokens";

/**
 * Bounds on the chrome's `sm` rung, which is the size the shell is written in.
 *
 * Whole pixels, and a short ladder: past 16px the 26px default row starts
 * clipping labels, and below 11px metadata (`xs`) drops under 10.
 */
export const UI_FONT_SIZE_RANGE = { min: 11, max: 16, fallback: metrics.textSM };

/** The variables a UI font size writes. Pure, so a test can read them. */
export function uiFontTokens(sm: number): Record<string, string> {
  const ratio = sm / metrics.textSM;
  const xs = Math.min(Math.round(metrics.textXS * ratio), sm - 1);
  const md = Math.max(Math.round(metrics.textMD * ratio), sm + 1);
  const lg = Math.max(Math.round(metrics.textLG * ratio), md + 1);
  const xl = Math.max(Math.round(metrics.textXL * ratio), lg + 1);
  return {
    "--forge-text-xs": `${xs}px`,
    "--forge-text-sm": `${sm}px`,
    "--forge-text-md": `${md}px`,
    "--forge-text-lg": `${lg}px`,
    "--forge-text-xl": `${xl}px`,
  };
}

export function applyUiFont(sm: number): void {
  const clamped = Math.min(
    Math.max(Math.round(sm), UI_FONT_SIZE_RANGE.min),
    UI_FONT_SIZE_RANGE.max,
  );
  const root = document.documentElement;
  for (const [name, value] of Object.entries(uiFontTokens(clamped))) {
    root.style.setProperty(name, value);
  }
  root.dataset.uiFont = String(clamped);
}
