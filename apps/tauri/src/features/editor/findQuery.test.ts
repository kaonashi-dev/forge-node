import { describe, expect, it } from "vitest";
import type { EditorFind } from "../../contracts/runtime";
import {
  MAX_FIND_PATTERN_BYTES,
  findCounter,
  findKeyCommand,
  findPatternTooLong,
  flagsOf,
  setFind,
} from "./findQuery";

const find = (partial: Partial<EditorFind> = {}): EditorFind => ({
  focus: 1,
  pattern: "llvm",
  case_sensitive: false,
  whole_word: false,
  regex: false,
  total: 12,
  capped: false,
  index: 3,
  error: null,
  ...partial,
});

describe("findCounter", () => {
  it("says where the caret is among the matches", () => {
    expect(findCounter(find(), "llvm")).toBe("3 of 12");
  });

  it("counts without a position once the caret left a match", () => {
    expect(findCounter(find({ index: 0 }), "llvm")).toBe("12 results");
    expect(findCounter(find({ index: 0, total: 1 }), "llvm")).toBe("1 result");
  });

  it("marks a count that stopped at the cap", () => {
    expect(findCounter(find({ total: 5000, capped: true }), "a")).toBe("3 of 5000+");
  });

  it("says no results, and nothing for an empty field", () => {
    expect(findCounter(find({ total: 0, index: 0 }), "zz")).toBe("No results");
    expect(findCounter(find({ total: 0, index: 0 }), "")).toBe("");
  });

  it("shows why a pattern cannot be searched with", () => {
    expect(findCounter(find({ error: "unclosed group" }), "(")).toBe("unclosed group");
  });
});

describe("findKeyCommand", () => {
  it("steps with Enter, back with Shift+Enter, and closes with Escape", () => {
    expect(findKeyCommand({ key: "Enter", shiftKey: false })).toBe("Next");
    expect(findKeyCommand({ key: "Enter", shiftKey: true })).toBe("Previous");
    expect(findKeyCommand({ key: "Escape", shiftKey: false })).toBe("Close");
    expect(findKeyCommand({ key: "a", shiftKey: false })).toBeNull();
  });
});

describe("setFind", () => {
  it("sends the typed pattern with the panel's flags", () => {
    expect(setFind("x", flagsOf(find({ regex: true })))).toEqual({
      Set: { pattern: "x", case_sensitive: false, whole_word: false, regex: true },
    });
  });
});

describe("findPatternTooLong", () => {
  it("counts bytes, as the daemon does, not UTF-16 units", () => {
    expect(findPatternTooLong("a".repeat(MAX_FIND_PATTERN_BYTES))).toBe(false);
    expect(findPatternTooLong("a".repeat(MAX_FIND_PATTERN_BYTES + 1))).toBe(true);
    expect(findPatternTooLong("é".repeat(MAX_FIND_PATTERN_BYTES / 2))).toBe(false);
    expect(findPatternTooLong("é".repeat(MAX_FIND_PATTERN_BYTES / 2 + 1))).toBe(true);
  });

  it("says so rather than the count for the last pattern the editor took", () => {
    const long = "x".repeat(MAX_FIND_PATTERN_BYTES + 1);
    expect(findCounter(find(), long)).toBe("Pattern is too long");
  });
});
