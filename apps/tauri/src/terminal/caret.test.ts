import { describe, expect, it } from "vitest";
import { caretRect } from "./caret";

const cell = { width: 7.8, height: 16 };

describe("caretRect", () => {
  it("is one physical pixel at DPR 1 and two at DPR 2", () => {
    const dpr1 = caretRect(0, 0, false, cell, "beam", 1);
    const dpr2 = caretRect(0, 0, false, cell, "beam", 2);
    expect(dpr1.w * 1).toBe(1);
    expect(dpr2.w * 2).toBe(2);
  });

  it("snaps a beam onto the device grid when the cell width is fractional", () => {
    const rect = caretRect(1, 0, false, cell, "beam", 2);
    expect(rect.x * 2).toBe(Math.round(1 * 7.8 * 2));
    expect(Number.isInteger(rect.x * 2)).toBe(true);
  });

  it("widens a block over a wide cell", () => {
    const narrow = caretRect(0, 0, false, cell, "block", 1);
    const wide = caretRect(0, 0, true, cell, "block", 1);
    expect(narrow.w).toBe(cell.width);
    expect(wide.w).toBe(cell.width * 2);
    expect(wide.h).toBe(cell.height);
  });

  it("keeps an underline inside the cell", () => {
    const rect = caretRect(3, 2, false, cell, "underline", 2);
    const top = 2 * cell.height;
    const bottom = top + cell.height;
    expect(rect.y).toBeGreaterThanOrEqual(top);
    expect(rect.y + rect.h).toBeLessThanOrEqual(bottom);
    expect(rect.h * 2).toBe(2);
  });
});
