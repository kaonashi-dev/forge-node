import { describe, expect, it } from "vitest";
import { relativeAge } from "./relativeAge";

const NOW = new Date("2026-08-30T12:00:00Z");

describe("relativeAge", () => {
  it("steps from minutes to hours to days", () => {
    expect(relativeAge("2026-08-30T11:55:00Z", NOW)).toBe("5m");
    expect(relativeAge("2026-08-30T09:00:00Z", NOW)).toBe("3h");
    expect(relativeAge("2026-08-27T12:00:00Z", NOW)).toBe("3d");
  });
});
