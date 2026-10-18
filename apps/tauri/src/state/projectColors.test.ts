import { describe, expect, it } from "vitest";
import { HUE_COUNT } from "../theme/tokens";
import { PROJECT_HUE_COUNT, parseProjectHues } from "./projectColors";

describe("project colours", () => {
  it("counts the same nine hues the theme emits", () => {
    expect(PROJECT_HUE_COUNT).toBe(HUE_COUNT);
  });

  it("keeps valid slots and drops everything else", () => {
    expect(parseProjectHues(undefined)).toEqual({});
    expect(parseProjectHues("not json")).toEqual({});
    expect(parseProjectHues("[1,2]")).toEqual({});
    expect(parseProjectHues(JSON.stringify({ a: 3, b: 9, c: -1, d: 1.5, e: "2", f: 0 }))).toEqual({
      a: 3,
      f: 0,
    });
  });
});
