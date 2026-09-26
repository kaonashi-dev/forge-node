import { describe, expect, it } from "vitest";
import { LinkPress } from "./linkPress";

describe("LinkPress", () => {
  it("opens when the pointer stays put", () => {
    const press = new LinkPress();
    press.arm(10, 10);
    expect(press.drag(12, 11)).toBe(false);
    expect(press.release(12, 11)).toBe(true);
    expect(press.release(12, 11)).toBe(false);
  });

  it("becomes a drag past the slop and does not also open", () => {
    const press = new LinkPress();
    press.arm(10, 10);
    expect(press.drag(16, 10)).toBe(true);
    expect(press.pending).toBe(false);
    expect(press.release(16, 10)).toBe(false);
  });

  it("does not open when the release is beyond the slop without an intervening move", () => {
    const press = new LinkPress();
    press.arm(10, 10);
    expect(press.release(16, 10)).toBe(false);
  });

  it("cancels", () => {
    const press = new LinkPress();
    press.arm(0, 0);
    press.cancel();
    expect(press.release(0, 0)).toBe(false);
  });
});
