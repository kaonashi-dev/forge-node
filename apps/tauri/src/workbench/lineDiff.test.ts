import { expect, it } from "vitest";
import { lineDiff } from "./lineDiff";

it("preserves line numbers for an insertion and a removal", () => {
  expect(lineDiff("a\nb\nc\n", "a\nc\nd\n")).toEqual([
    { kind: "equal", text: "a", left: 1, right: 1 },
    { kind: "removed", text: "b", left: 2, right: null },
    { kind: "equal", text: "c", left: 3, right: 2 },
    { kind: "added", text: "d", left: null, right: 3 },
  ]);
});

it("falls back for 10,000 completely different lines", () => {
  const rows = lineDiff("left\n".repeat(10_000), "right\n".repeat(10_000));
  expect(rows).toHaveLength(20_000);
  expect(rows.slice(0, 2)).toEqual([
    { kind: "removed", text: "left", left: 1, right: null },
    { kind: "added", text: "right", left: null, right: 1 },
  ]);
});

it("handles empty buffers", () => {
  expect(lineDiff("", "")).toEqual([]);
});
