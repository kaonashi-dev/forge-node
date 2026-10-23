import { afterEach, describe, expect, it, vi } from "vitest";
import { applyTrimChrome, trimTokens } from "./trim";
import { metrics } from "./tokens";

const px = (value: string) => Number.parseInt(value, 10);

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("trim finish", () => {
  it("pulls the chrome in without touching row density or the measured grid pads", () => {
    const tokens = trimTokens();
    expect(px(tokens["--forge-strip-h"])).toBeLessThan(metrics.stripH);
    expect(px(tokens["--forge-tab-h"])).toBeLessThan(metrics.tabH);
    expect(px(tokens["--forge-location-h"])).toBeLessThan(metrics.locationH);
    expect(px(tokens["--forge-open-files-h"])).toBeLessThan(metrics.openFilesH);
    expect(tokens["--forge-terminal-pad-x"]).toBeUndefined();
    expect(tokens["--forge-editor-pad"]).toBeUndefined();
    expect(tokens["--forge-row-h"]).toBeUndefined();
    expect(tokens["--forge-tree-row-h"]).toBeUndefined();
  });

  it("rounds every corner onto a trim rung", () => {
    const tokens = trimTokens();
    const radii = Object.entries(tokens).filter(([name]) => name.includes("radius"));
    expect(radii.length).toBeGreaterThan(0);
    for (const [name, value] of radii) {
      expect(px(value), name).toBeGreaterThan(0);
      expect(px(value), name).toBeLessThanOrEqual(6);
    }
  });

  it("leaves circles to --radius-full", () => {
    const tokens = trimTokens();
    expect(tokens["--forge-radius-xl"]).toBeUndefined();
    expect(tokens["--radius-full"]).toBeUndefined();
  });

  it("toggles the root attribute and removes its component radii when off", () => {
    const style = { setProperty: vi.fn(), removeProperty: vi.fn() };
    vi.stubGlobal("document", { documentElement: { dataset: {}, style } });

    applyTrimChrome(true);
    expect(document.documentElement.dataset.trim).toBe("true");
    expect(style.setProperty).toHaveBeenCalledWith("--radius-pill", "4px");

    applyTrimChrome(false);
    expect(document.documentElement.dataset.trim).toBeUndefined();
    expect(style.removeProperty).toHaveBeenCalledWith("--radius-pill");
  });
});
