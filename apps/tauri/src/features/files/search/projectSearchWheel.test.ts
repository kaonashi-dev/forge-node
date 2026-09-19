import { describe, expect, it } from "vitest";
import { isSearchExcerptTarget, searchWheelChainsVertically } from "./projectSearchWheel";

const wheel = (
  delta: { x?: number; y?: number },
  keys: { ctrl?: boolean; meta?: boolean } = {},
) => ({
  ctrlKey: keys.ctrl ?? false,
  metaKey: keys.meta ?? false,
  deltaX: delta.x ?? 0,
  deltaY: delta.y ?? 0,
});

describe("searchWheelChainsVertically", () => {
  it("takes a vertical wheel", () => {
    expect(searchWheelChainsVertically(wheel({ y: 40 }))).toBe(true);
  });

  it("leaves a horizontal or diagonal-horizontal gesture to the line", () => {
    expect(searchWheelChainsVertically(wheel({ x: 40 }))).toBe(false);
    expect(searchWheelChainsVertically(wheel({ x: 30, y: 10 }))).toBe(false);
    expect(searchWheelChainsVertically(wheel({ x: 20, y: 20 }))).toBe(false);
  });

  it("does not steal pinch-zoom", () => {
    expect(searchWheelChainsVertically(wheel({ y: 40 }, { ctrl: true }))).toBe(false);
    expect(searchWheelChainsVertically(wheel({ y: 40 }, { meta: true }))).toBe(false);
  });
});

describe("isSearchExcerptTarget", () => {
  const excerpt = {
    closest: (selector: string) => (selector === ".project-search-excerpt" ? {} : null),
  };
  const head = { closest: () => null };

  it("matches an excerpt and walks up from a text node", () => {
    expect(isSearchExcerptTarget(excerpt)).toBe(true);
    expect(isSearchExcerptTarget({ parentElement: excerpt })).toBe(true);
  });

  it("ignores the file header and a missing target", () => {
    expect(isSearchExcerptTarget(head)).toBe(false);
    expect(isSearchExcerptTarget(null)).toBe(false);
  });
});
