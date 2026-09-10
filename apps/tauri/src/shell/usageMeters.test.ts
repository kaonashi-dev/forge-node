import { describe, expect, it } from "vitest";
import {
  usageIsStale,
  usagePercent,
  usageResetLabel,
  usageShortResetLabel,
  usageTone,
  usageUpdatedLabel,
} from "./usageMeters";

describe("usage meters", () => {
  it("clamps percentages and applies the warning thresholds", () => {
    expect(usagePercent(-1)).toBe(0);
    expect(usagePercent(105)).toBe(100);
    expect(usageTone(74, false)).toBe("normal");
    expect(usageTone(75, false)).toBe("warning");
    expect(usageTone(90, false)).toBe("full");
    expect(usageTone(100, true)).toBe("stale");
  });

  it("marks readings stale only after ten minutes", () => {
    const now = Date.parse("2026-08-30T12:10:00Z");
    expect(usageIsStale("2026-08-30T12:00:00Z", now)).toBe(false);
    expect(usageIsStale("2026-08-30T11:59:59Z", now)).toBe(true);
  });

  it("formats future resets and omits unknown or expired ones", () => {
    const now = Date.parse("2026-08-30T10:00:00Z");
    expect(usageResetLabel("2026-08-30T12:05:00Z", now)).toBe("resets in 2h 5m");
    expect(usageResetLabel("2026-08-30T23:59:00Z", now)).toBe("resets in 13h 59m");
    expect(usageResetLabel(null, now)).toBeNull();
    expect(usageResetLabel("2026-08-30T09:59:00Z", now)).toBeNull();
  });

  it("rolls resets a day or more out into days and hours", () => {
    const now = Date.parse("2026-08-30T10:00:00Z");
    expect(usageResetLabel("2026-08-31T10:00:00Z", now)).toBe("resets in 1d 0h");
    expect(usageResetLabel("2026-08-31T01:30:00Z", now)).toBe("resets in 15h 30m");
    expect(usageResetLabel("2026-08-31T11:45:00Z", now)).toBe("resets in 1d 1h");
    expect(usageResetLabel("2026-09-06T09:00:00Z", now)).toBe("resets in 6d 23h");
  });

  it("formats updated labels from collected_at", () => {
    const now = Date.parse("2026-08-30T12:10:00Z");
    expect(usageUpdatedLabel("2026-08-30T12:09:30Z", now)).toBe("Updated just now");
    expect(usageUpdatedLabel("2026-08-30T12:05:00Z", now)).toBe("Updated 5m ago");
    expect(usageUpdatedLabel("2026-08-30T10:10:00Z", now)).toBe("Updated 2h 0m ago");
    expect(usageUpdatedLabel("2026-08-25T12:10:00Z", now)).toBe("Updated 5d ago");
  });

  it("shortens reset labels", () => {
    const now = Date.parse("2026-08-30T10:00:00Z");
    expect(usageShortResetLabel("2026-08-30T12:05:00Z", now)).toBe("2h 5m");
    expect(usageShortResetLabel("2026-09-06T09:00:00Z", now)).toBe("6d 23h");
    expect(usageShortResetLabel(null, now)).toBeNull();
  });
});
