import { describe, expect, it } from "vitest";
import { Viewport } from "./viewport";
import type { CellsPayload, WireRow } from "./types";
import { DEFAULT_MODES } from "./types";

function row(...runs: WireRow["r"]): WireRow {
  return { w: false, r: runs };
}

function frame(patch: CellsPayload["patch"], overrides: Partial<CellsPayload> = {}): CellsPayload {
  return {
    terminal: "t1",
    seq: 1,
    cols: 4,
    rows: 3,
    full: false,
    patch,
    cursor: { line: 0, col: 0, shape: "block", visible: true },
    modes: DEFAULT_MODES,
    scroll_offset: 0,
    scrollback_len: 0,
    title: null,
    bell: false,
    echo_id: 0,
    ...overrides,
  };
}

describe("Viewport", () => {
  it("reports only the rows a patch touched, plus the cursor's", () => {
    const viewport = new Viewport();
    viewport.apply(frame([], { full: true }));
    const dirty = viewport.apply(frame([[2, row(["ab", 2, -1, -2, 0])]]));
    expect([...dirty].sort()).toEqual([0, 2]);
  });

  /// The cursor is painted over the text rather than encoded into it, so the
  /// row it left has to repaint even though its content did not change.
  it("repaints the row the cursor left", () => {
    const viewport = new Viewport();
    viewport.apply(frame([], { full: true }));
    const dirty = viewport.apply(
      frame([], { cursor: { line: 2, col: 0, shape: "block", visible: true } }),
    );
    expect([...dirty].sort()).toEqual([0, 2]);
  });

  it("repaints everything when the geometry changed", () => {
    const viewport = new Viewport();
    viewport.apply(frame([], { full: true }));
    const dirty = viewport.apply(frame([], { cols: 8, rows: 5 }));
    expect([...dirty].sort((a, b) => a - b)).toEqual([0, 1, 2, 3, 4]);
    expect(viewport.rows).toHaveLength(5);
  });

  it("drops a patch row the geometry no longer has", () => {
    const viewport = new Viewport();
    viewport.apply(frame([[9, row(["x", 1, -1, -2, 0])]], { full: true }));
    expect(viewport.rows).toHaveLength(3);
  });

  it("keeps a null row so the text above does not jump when history lands", () => {
    const viewport = new Viewport();
    viewport.apply(frame([[1, null]], { full: true }));
    expect(viewport.rows[1]).toBeNull();
    expect(viewport.cellAt(1, 0)).toBeNull();
  });

  describe("cellAt", () => {
    it("indexes into a run of ordinary cells", () => {
      const viewport = new Viewport();
      viewport.apply(frame([[0, row(["abc", 3, 4, -2, 1])]], { full: true }));
      expect(viewport.cellAt(0, 1)).toEqual({ text: "b", fg: 4, bg: -2, flags: 1 });
    });

    it("gives a wide grapheme its leading column and blanks the continuation", () => {
      const viewport = new Viewport();
      viewport.apply(frame([[0, row(["漢", 2, -1, -2, 128])]], { full: true }));
      expect(viewport.cellAt(0, 0)?.text).toBe("漢");
      expect(viewport.cellAt(0, 1)?.text).toBe("");
    });

    it("returns null past the end of the row", () => {
      const viewport = new Viewport();
      viewport.apply(frame([[0, row(["ab", 2, -1, -2, 0])]], { full: true }));
      expect(viewport.cellAt(0, 3)).toBeNull();
    });
  });

  it("expands a row into one glyph per column for word boundaries", () => {
    const viewport = new Viewport();
    viewport.apply(
      frame([[0, row(["a", 1, -1, -2, 0], ["漢", 2, -1, -2, 128], ["b", 1, -1, -2, 0])]], {
        full: true,
      }),
    );
    expect(viewport.columns(0)).toEqual(["a", "漢", "", "b"]);
  });

  it("treats a blank row as spaces rather than as absent", () => {
    const viewport = new Viewport();
    viewport.apply(frame([[0, row()]], { full: true }));
    expect(viewport.columns(0)).toEqual([" ", " ", " ", " "]);
  });
});
