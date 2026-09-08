import { describe, expect, it } from "vitest";
import { tooltipAnchorRect, tooltipOverflowPaddingFor } from "./tooltipInset";

describe("tooltipOverflowPaddingFor", () => {
  it("uses the default gutter off macOS", () => {
    expect(tooltipOverflowPaddingFor(undefined, "78px")).toBe(8);
    expect(tooltipOverflowPaddingFor("linux", "78px")).toBe(8);
  });

  it("reserves the traffic-light width on macOS", () => {
    expect(tooltipOverflowPaddingFor("macos", "78px")).toBe(86);
  });
});

describe("tooltipAnchorRect", () => {
  it("anchors to the child when the trigger draws no box", () => {
    const childRect = { x: 900, y: 8, width: 60, height: 24 };
    const triggerRect = { x: 0, y: 0, width: 0, height: 0 };
    const anchor = {
      getBoundingClientRect: () => triggerRect,
      firstElementChild: { getBoundingClientRect: () => childRect },
    };
    expect(tooltipAnchorRect(anchor)).toBe(childRect);
  });

  it("falls back to the trigger itself without a child", () => {
    const triggerRect = { x: 12, y: 20, width: 32, height: 32 };
    expect(
      tooltipAnchorRect({ getBoundingClientRect: () => triggerRect, firstElementChild: null }),
    ).toBe(triggerRect);
    expect(tooltipAnchorRect(undefined)).toBeUndefined();
  });
});
