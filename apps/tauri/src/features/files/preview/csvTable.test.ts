import { describe, expect, it } from "vitest";

import { csvDelimiter, parseCsv } from "./csvTable";

describe("csv preview table", () => {
  it("splits rows and fields", () => {
    expect(parseCsv("a,b\n1,2\n", ",")).toEqual({
      rows: [
        ["a", "b"],
        ["1", "2"],
      ],
      width: 2,
      truncated: false,
    });
  });

  it("keeps delimiters, newlines and doubled quotes inside a quoted field", () => {
    const { rows } = parseCsv('name,note\r\n"Doe, J","said ""hi""\nthen left"\r\n', ",");
    expect(rows).toEqual([
      ["name", "note"],
      ["Doe, J", 'said "hi"\nthen left'],
    ]);
  });

  it("reads a last row with no trailing newline and drops a byte-order mark", () => {
    expect(parseCsv("﻿a,b\n1,", ",").rows).toEqual([
      ["a", "b"],
      ["1", ""],
    ]);
  });

  /* Ragged rows are common in hand-edited files; the widest one sets the grid. */
  it("reports the widest row", () => {
    expect(parseCsv("a\n1,2,3\n", ",").width).toBe(3);
  });

  it("stops at the row and column caps and says so", () => {
    const capped = parseCsv("1\n2\n3\n", ",", 2);
    expect(capped.rows).toEqual([["1"], ["2"]]);
    expect(capped.truncated).toBe(true);
    expect(parseCsv("1\n2\n", ",", 2).truncated).toBe(false);
    const narrow = parseCsv("a,b,c\n", ",", 10, 2);
    expect(narrow.rows).toEqual([["a", "b"]]);
    expect(narrow.truncated).toBe(true);
  });

  it("uses a tab for tsv", () => {
    expect(csvDelimiter("data/export.TSV")).toBe("\t");
    expect(csvDelimiter("data/export.csv")).toBe(",");
    expect(parseCsv("a\tb,c\n", "\t").rows).toEqual([["a", "b,c"]]);
  });
});
