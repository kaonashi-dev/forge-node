import { describe, expect, it } from "vitest";
import { elapsedLabel, since } from "./elapsed";

describe("elapsedLabel", () => {
  it("never shows seconds", () => {
    expect(elapsedLabel(12_000)).toBe("<1m");
    expect(elapsedLabel(4 * 60_000 + 12_000)).toBe("4m");
  });

  it("carries the next unit down once there is one", () => {
    expect(elapsedLabel(72 * 60_000)).toBe("1h 12m");
    expect(elapsedLabel(2 * 3_600_000)).toBe("2h");
    expect(elapsedLabel(51 * 3_600_000)).toBe("2d 3h");
  });

  it("treats a clock that ran backwards as no time at all", () => {
    expect(elapsedLabel(-5_000)).toBe("<1m");
  });
});

describe("since", () => {
  it("is null for a missing or unparseable stamp", () => {
    expect(since(undefined, 0)).toBeNull();
    expect(since("not a date", 0)).toBeNull();
  });

  it("measures from the stamp", () => {
    expect(since("2026-01-01T00:00:00Z", Date.parse("2026-01-01T00:04:00Z"))).toBe(240_000);
  });
});
