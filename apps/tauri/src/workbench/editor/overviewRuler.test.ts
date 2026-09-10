import { describe, expect, it } from "vitest";
import { lineAt, rulerTicks } from "./overviewRuler";
import type { GitMark, GitMarks } from "./createEditor";

const marks = (entries: Array<[number, GitMark]>): GitMarks => new Map(entries);

describe("rulerTicks", () => {
  it("has nothing to draw for a clean file", () => {
    expect(rulerTicks(marks([]), 100)).toEqual([]);
  });

  it("joins a consecutive run of one mark into one band", () => {
    const ticks = rulerTicks(
      marks([
        [10, "added"],
        [11, "added"],
        [12, "added"],
      ]),
      100,
    );
    expect(ticks).toHaveLength(1);
    expect(ticks[0].line).toBe(10);
    expect(ticks[0].top).toBeCloseTo(9);
    expect(ticks[0].height).toBeCloseTo(3);
  });

  it("breaks a run where the mark changes", () => {
    const ticks = rulerTicks(
      marks([
        [10, "added"],
        [11, "modified"],
      ]),
      100,
    );
    expect(ticks.map((tick) => tick.mark)).toEqual(["added", "modified"]);
  });

  it("breaks a run where the lines are not adjacent", () => {
    const ticks = rulerTicks(
      marks([
        [10, "added"],
        [40, "added"],
      ]),
      100,
    );
    expect(ticks.map((tick) => tick.line)).toEqual([10, 40]);
  });

  // A `Map` built from a patch is in hunk order, which is not line order.
  it("orders bands by line, whatever order the marks arrived in", () => {
    const ticks = rulerTicks(
      marks([
        [80, "modified"],
        [5, "added"],
      ]),
      100,
    );
    expect(ticks.map((tick) => tick.line)).toEqual([5, 80]);
  });

  it("keeps a one-line change visible in a long file", () => {
    const ticks = rulerTicks(marks([[1000, "modified"]]), 5000);
    expect(ticks[0].height).toBeGreaterThan(0.3);
  });

  it("never runs a band past the bottom edge", () => {
    const ticks = rulerTicks(marks([[5000, "deleted"]]), 5000);
    expect(ticks[0].top + ticks[0].height).toBeLessThanOrEqual(100);
  });

  it("drops a mark the document does not have a line for", () => {
    // A stale patch outliving an edit that shortened the file.
    expect(rulerTicks(marks([[900, "added"]]), 10)).toEqual([]);
  });
});

describe("lineAt", () => {
  const ticks = rulerTicks(
    marks([
      [10, "added"],
      [90, "modified"],
    ]),
    100,
  );

  it("answers with the nearest band, not the raw position", () => {
    expect(lineAt(ticks, 0.12, 100)).toBe(10);
    expect(lineAt(ticks, 0.7, 100)).toBe(90);
  });

  it("clamps a click outside the column", () => {
    expect(lineAt(ticks, -1, 100)).toBe(10);
    expect(lineAt(ticks, 2, 100)).toBe(90);
  });

  it("has no answer when there is nothing to jump to", () => {
    expect(lineAt([], 0.5, 100)).toBeNull();
  });
});
