import { describe, expect, it } from "vitest";
import { cellAtPoint, columnsOn, ends, isEmpty, wordAt } from "./selection";

const at = (line: number, col: number) => ({ line, col });

describe("selection", () => {
  it("reads the same either way round", () => {
    const upwards = { anchor: at(2, 1), head: at(0, 3) };
    expect(ends(upwards)).toEqual([at(0, 3), at(2, 1)]);
  });

  it("does not leave a one-cell selection behind a plain click", () => {
    expect(isEmpty({ anchor: at(1, 1), head: at(1, 1) })).toBe(true);
    expect(isEmpty({ anchor: at(1, 1), head: at(1, 2) })).toBe(false);
  });

  describe("columnsOn", () => {
    const selection = { anchor: at(0, 2), head: at(2, 4) };

    it("runs to the end of a line it passes through", () => {
      expect(columnsOn(selection, 0, 10)).toEqual([2, 9]);
      expect(columnsOn(selection, 1, 10)).toEqual([0, 9]);
      expect(columnsOn(selection, 2, 10)).toEqual([0, 4]);
    });

    it("skips lines outside the range", () => {
      expect(columnsOn(selection, -1, 10)).toBeNull();
      expect(columnsOn(selection, 3, 10)).toBeNull();
    });

    it("clamps to the row it is painting", () => {
      expect(columnsOn(selection, 2, 3)).toEqual([0, 2]);
    });

    it("selects the one cell a drag never left", () => {
      expect(columnsOn({ anchor: at(1, 5), head: at(1, 5) }, 1, 10)).toEqual([5, 5]);
    });
  });

  describe("wordAt", () => {
    // A path is one word: double-clicking `~/dev/forge-node` has to give back
    // the whole thing, which is the only reason anyone double-clicks here.
    const columns = [..."cd ~/dev/forge-node && ls"];

    it("keeps a path whole", () => {
      const [from, to] = wordAt(columns, 8);
      expect(columns.slice(from, to + 1).join("")).toBe("~/dev/forge-node");
    });

    it("stops at whitespace and at shell punctuation", () => {
      const [from, to] = wordAt(columns, 0);
      expect(columns.slice(from, to + 1).join("")).toBe("cd");
    });

    it("selects a separator rather than doing nothing", () => {
      expect(wordAt(columns, 2)).toEqual([2, 2]);
    });

    // The continuation column of a wide grapheme carries no text, and
    // `terminal.rs` treats an empty cell as a boundary — so a word runs up to
    // the wide glyph and stops at the column it borrowed.
    it("ends a word at the blank continuation column of a wide grapheme", () => {
      expect(wordAt(["a", "漢", "", "b"], 1)).toEqual([0, 1]);
      expect(wordAt(["a", "漢", "", "b"], 3)).toEqual([3, 3]);
      expect(wordAt(["a", "漢", "", "b"], 2)).toEqual([2, 2]);
    });
  });

  describe("cellAtPoint", () => {
    it("is scroll-stable: the line counts from the live grid", () => {
      expect(cellAtPoint(0, 36, 8, 18, 5, 80)).toEqual({ line: -3, col: 0 });
    });

    it("clamps sideways so a drag off the pane means end of line", () => {
      expect(cellAtPoint(9999, 0, 8, 18, 0, 80).col).toBe(79);
      expect(cellAtPoint(-40, 0, 8, 18, 0, 80).col).toBe(0);
    });

    it("does not produce a column for a zero-width grid", () => {
      expect(cellAtPoint(50, 0, 8, 18, 0, 0).col).toBe(0);
    });
  });
});
