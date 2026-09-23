export type StepRange = { min: number; max: number; step: number };

/**
 * One stepper press: the value moved by `direction` steps, snapped to the step
 * and clamped. Snapped through `toFixed` because `0.1` steps accumulate float
 * drift, and a stored `1.2000000000000002` reads back as a different zoom.
 */
export function stepValue(current: number, direction: 1 | -1, range: StepRange): number {
  const snapped = Math.round(current / range.step) * range.step;
  const next = Math.min(range.max, Math.max(range.min, snapped + direction * range.step));
  return Number(next.toFixed(2));
}

export function canStep(current: number, direction: 1 | -1, range: StepRange): boolean {
  return stepValue(current, direction, range) !== Number(current.toFixed(2));
}

export const DENSITY_NOTES = {
  compact: "Rows are 26px — the most sessions visible at once.",
  cozy: "Rows are 32px — easier to hit, fewer on screen.",
} as const;
