import { describe, expect, it } from "vitest";
import { DENSITIES, densityOffset, densityTokens, isDensity, readDensity } from "./density";
import { metrics } from "./tokens";

const px = (value: string) => Number.parseInt(value, 10);

describe("density", () => {
  it("leaves the default exactly where the scale put it", () => {
    // The point of a default is that choosing it changes nothing.
    const tokens = densityTokens("compact");
    expect(px(tokens["--forge-row-h"])).toBe(metrics.rowH);
    expect(px(tokens["--forge-control-md"])).toBe(metrics.controlMD);
  });

  it("moves the row height to 26 / 32", () => {
    expect(DENSITIES.map((d) => px(densityTokens(d)["--forge-row-h"]))).toEqual([26, 32]);
  });

  it("keeps the control ladder ascending at every density", () => {
    for (const density of DENSITIES) {
      const t = densityTokens(density);
      const ladder = [
        px(t["--forge-control-xs"]),
        px(t["--forge-control-sm"]),
        px(t["--forge-control-md"]),
        px(t["--forge-control-lg"]),
      ];
      expect(
        ladder.every((v, i) => i === 0 || v > ladder[i - 1]),
        density,
      ).toBe(true);
    }
  });

  it("keeps the rungs the same distance apart, which a ratio would not", () => {
    const gap = (density: (typeof DENSITIES)[number]) => {
      const t = densityTokens(density);
      return px(t["--forge-control-lg"]) - px(t["--forge-control-xs"]);
    };
    expect(new Set(DENSITIES.map(gap)).size).toBe(1);
  });

  it("never puts a control below a usable pointer target", () => {
    for (const density of DENSITIES) {
      expect(px(densityTokens(density)["--forge-control-xs"]), density).toBeGreaterThanOrEqual(16);
    }
  });

  it("offsets cozy up from compact", () => {
    expect(densityOffset("compact")).toBe(0);
    expect(densityOffset("cozy")).toBe(6);
  });

  it("accepts only the two density ids", () => {
    expect(isDensity("cozy")).toBe(true);
    expect(isDensity("comfortable")).toBe(false);
    expect(isDensity(undefined)).toBe(false);
  });

  it("maps the retired three-step ids onto the two that remain", () => {
    expect(readDensity("default")).toBe("compact");
    expect(readDensity("comfortable")).toBe("cozy");
    expect(readDensity("cozy")).toBe("cozy");
    expect(readDensity("bogus")).toBe("compact");
  });
});
