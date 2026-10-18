import { describe, expect, it } from "vitest";
import { justCreated } from "./motion";

describe("justCreated", () => {
  const now = Date.parse("2026-10-16T12:00:10Z");
  it("marks a row minted inside the window, and nothing older, future or unparseable", () => {
    expect(justCreated("2026-10-16T12:00:09Z", now)).toBe(true);
    expect(justCreated("2026-10-16T12:00:00Z", now)).toBe(false);
    expect(justCreated("2026-10-16T12:00:20Z", now)).toBe(false);
    expect(justCreated("not a date", now)).toBe(false);
  });
});
