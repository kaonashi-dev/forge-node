import { describe, expect, it } from "vitest";
import { nextHunk, PATCH_OVERSCAN, patchWindow } from "./patchWindow";

describe("patchWindow", () => {
  it("mounts only the visible lines and a fixed overscan", () => {
    expect(patchWindow(2000, 400, 20, 20_000)).toEqual({
      first: 100 - PATCH_OVERSCAN,
      end: 120 + PATCH_OVERSCAN,
    });
  });

  it("keeps the same row budget deep into a large patch", () => {
    const middle = patchWindow(2000, 400, 20, 20_000);
    const deep = patchWindow(200_000, 400, 20, 20_000);
    expect(deep.end - deep.first).toBe(middle.end - middle.first);
  });

  it("keeps distant files unmounted on either side of the viewport", () => {
    expect(patchWindow(-10_000, 400, 20, 2000)).toEqual({ first: 0, end: 0 });
    expect(patchWindow(50_000, 400, 20, 2000)).toEqual({ first: 0, end: 0 });
  });

  it("clips the window to a short patch and a partially visible file", () => {
    expect(patchWindow(0, 400, 20, 5)).toEqual({ first: 0, end: 5 });
    expect(patchWindow(-200, 400, 20, 2000)).toEqual({ first: 0, end: 10 + PATCH_OVERSCAN });
    expect(patchWindow(39_800, 400, 20, 2000)).toEqual({ first: 1990 - PATCH_OVERSCAN, end: 2000 });
  });

  it("includes partially visible rows with fractional font metrics", () => {
    const window = patchWindow(17.6 * 100 + 0.5, 17.6 * 20, 17.6, 2000);
    expect(window).toEqual({ first: 100 - PATCH_OVERSCAN, end: 121 + PATCH_OVERSCAN });
  });

  it("mounts nothing in a hidden viewport or an empty patch", () => {
    expect(patchWindow(0, 0, 20, 2000)).toEqual({ first: 0, end: 0 });
    expect(patchWindow(0, 400, 20, 0)).toEqual({ first: 0, end: 0 });
  });
});

describe("nextHunk", () => {
  it("navigates by document indexes even when the target has no DOM row", () => {
    const starts = [0, 1000, 2000];
    expect(nextHunk(starts, -1, true)).toBe(0);
    expect(nextHunk(starts, 0, true)).toBe(1000);
    expect(nextHunk(starts, 2000, false)).toBe(1000);
    expect(starts).toEqual([0, 1000, 2000]);
  });

  it("stops at either end", () => {
    expect(nextHunk([], 0, true)).toBeNull();
    expect(nextHunk([0, 1000], 1000, true)).toBeNull();
    expect(nextHunk([0, 1000], 0, false)).toBeNull();
  });
});
