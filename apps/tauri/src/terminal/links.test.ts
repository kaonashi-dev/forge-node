import { describe, expect, it } from "vitest";
import { indexOfCell, lineTextAt, spansOfRange } from "./links";
import { FLAG_WIDE_CHAR } from "./types";
import type { WireRow } from "./types";

const row = (text: string, wrapped = false): WireRow => ({
  w: wrapped,
  r: [[text, [...text].length, -1, -2, 0]],
});

describe("lineTextAt", () => {
  it("pads a row to the width, so every column maps to a character", () => {
    const line = lineTextAt([row("ab")], 0, 4);
    expect(line.text).toBe("ab  ");
    expect(indexOfCell(line, 0, 3)).toBe(3);
  });

  it("joins a soft wrap from either side of it", () => {
    const rows = [row("apps/tau", true), row("ri/x.ts")];
    expect(lineTextAt(rows, 0, 8).text).toBe("apps/tauri/x.ts ");
    expect(lineTextAt(rows, 1, 8).text).toBe("apps/tauri/x.ts ");
  });

  it("stops at a row that did not wrap", () => {
    const rows = [row("first"), row("second")];
    expect(lineTextAt(rows, 1, 6).text).toBe("second");
  });

  it("keeps a wide glyph on the column it paints in", () => {
    const wide: WireRow = {
      w: false,
      r: [
        ["漢", 2, -1, -2, FLAG_WIDE_CHAR],
        ["x", 1, -1, -2, 0],
      ],
    };
    const line = lineTextAt([wide], 0, 3);
    expect(line.text).toBe("漢x");
    expect(indexOfCell(line, 0, 0)).toBe(0);
    // The continuation column paints nothing, so nothing points back at it.
    expect(indexOfCell(line, 0, 1)).toBe(-1);
    expect(indexOfCell(line, 0, 2)).toBe(1);
  });
});

describe("spansOfRange", () => {
  it("is one range on one row", () => {
    const line = lineTextAt([row("see src/a.rs")], 0, 12);
    expect(spansOfRange(line, 4, 12)).toEqual([{ row: 0, from: 4, to: 11 }]);
  });

  it("splits at the wrap so both halves are underlined", () => {
    const rows = [row("apps/tau", true), row("ri/x.ts")];
    const line = lineTextAt(rows, 0, 8);
    expect(spansOfRange(line, 0, 15)).toEqual([
      { row: 0, from: 0, to: 7 },
      { row: 1, from: 0, to: 6 },
    ]);
  });
});
