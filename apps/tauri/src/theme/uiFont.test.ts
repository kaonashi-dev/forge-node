import { describe, expect, it } from "vitest";
import { UI_FONT_SIZE_RANGE, uiFontTokens } from "./uiFont";
import { metrics } from "./tokens";

const px = (value: string) => Number.parseFloat(value);

function ladder(sm: number): number[] {
  const tokens = uiFontTokens(sm);
  return [
    px(tokens["--forge-text-xs"]),
    px(tokens["--forge-text-sm"]),
    px(tokens["--forge-text-md"]),
    px(tokens["--forge-text-lg"]),
    px(tokens["--forge-text-xl"]),
  ];
}

describe("ui font scale", () => {
  it("leaves the default exactly where the scale put it", () => {
    const tokens = uiFontTokens(metrics.textSM);
    expect(px(tokens["--forge-text-xs"])).toBe(metrics.textXS);
    expect(px(tokens["--forge-text-sm"])).toBe(metrics.textSM);
    expect(px(tokens["--forge-text-md"])).toBe(metrics.textMD);
    expect(px(tokens["--forge-text-lg"])).toBe(metrics.textLG);
    expect(px(tokens["--forge-text-xl"])).toBe(metrics.textXL);
  });

  it("keeps the type scale ascending at every offered size", () => {
    for (
      let size = UI_FONT_SIZE_RANGE.min;
      size <= UI_FONT_SIZE_RANGE.max;
      size += UI_FONT_SIZE_RANGE.step
    ) {
      const rungs = ladder(size);
      expect(
        rungs.every((value, index) => index === 0 || value > rungs[index - 1]),
        String(size),
      ).toBe(true);
    }
  });

  it("writes the size the person picked as the chrome default", () => {
    for (
      let size = UI_FONT_SIZE_RANGE.min;
      size <= UI_FONT_SIZE_RANGE.max;
      size += UI_FONT_SIZE_RANGE.step
    ) {
      expect(px(uiFontTokens(size)["--forge-text-sm"])).toBe(size);
    }
  });

  it("uses the theme's sm rung as the fallback", () => {
    expect(UI_FONT_SIZE_RANGE.fallback).toBe(metrics.textSM);
  });
});
