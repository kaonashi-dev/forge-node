import { describe, expect, it } from "vitest";
import { hasExited } from "./exited";

describe("hasExited", () => {
  it("is a session whose process is gone and whose terminal went with it", () => {
    expect(hasExited({ state: { Exited: { code: 0, signal: null } }, terminal_id: null })).toBe(
      true,
    );
    expect(hasExited({ state: "Orphaned", terminal_id: null })).toBe(true);
  });

  it("is never a live session, even between spawn and its first terminal", () => {
    expect(hasExited({ state: "Starting", terminal_id: null })).toBe(false);
    expect(hasExited({ state: "Running", terminal_id: "t" })).toBe(false);
  });
});
