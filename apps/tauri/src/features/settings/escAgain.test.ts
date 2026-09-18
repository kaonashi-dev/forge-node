import { describe, expect, it } from "vitest";
import { ESC_AGAIN_MS, isSecondEsc } from "./escAgain";

describe("isSecondEsc", () => {
  it("needs a first Escape before it will close", () => {
    expect(isSecondEsc(1_000, null)).toBe(false);
  });

  it("closes when the second Escape is inside the window", () => {
    expect(isSecondEsc(1_000, 1_000 - ESC_AGAIN_MS)).toBe(true);
    expect(isSecondEsc(1_000, 1_000)).toBe(true);
  });

  it("arms again when the window has elapsed", () => {
    expect(isSecondEsc(1_000, 1_000 - ESC_AGAIN_MS - 1)).toBe(false);
  });
});
