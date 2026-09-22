import { describe, expect, it } from "vitest";
import { DENSITIES } from "../../theme/density";
import { DENSITY_NOTES, canStep, stepValue } from "./typeSteps";

const chrome = { min: 12, max: 16, step: 0.5 };
const zoom = { min: 0.5, max: 2, step: 0.1 };

describe("type steppers", () => {
  it("moves the chrome size by half pixels", () => {
    expect(stepValue(12.5, 1, chrome)).toBe(13);
    expect(stepValue(12.5, -1, chrome)).toBe(12);
  });

  it("stops at either end instead of writing past it", () => {
    expect(stepValue(16, 1, chrome)).toBe(16);
    expect(canStep(16, 1, chrome)).toBe(false);
    expect(canStep(12, -1, chrome)).toBe(false);
    expect(canStep(12, 1, chrome)).toBe(true);
  });

  it("keeps a tenth-step zoom free of float drift", () => {
    let value = 1;
    for (let press = 0; press < 3; press += 1) value = stepValue(value, 1, zoom);
    expect(value).toBe(1.3);
  });

  it("snaps an off-grid stored value before stepping", () => {
    expect(stepValue(12.7, 1, chrome)).toBe(13);
  });

  it("has a note for every density", () => {
    for (const density of DENSITIES) expect(DENSITY_NOTES[density]).toMatch(/^Rows are \d+px/);
  });
});
