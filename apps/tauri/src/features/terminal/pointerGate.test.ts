import { describe, expect, it } from "vitest";
import { acceptsPointer, type PointerGate } from "./pointerGate";

const live: PointerGate = {
  active: true,
  selectionPending: false,
  viewportTerminal: "t1",
  hasRows: true,
  connected: true,
  boundTerminal: "t1",
};

describe("acceptsPointer", () => {
  it("accepts a live pane showing the terminal it is bound to", () => {
    expect(acceptsPointer(live)).toBe(true);
  });

  it("accepts the split's second pane while the window stays attached elsewhere", () => {
    // The gate reads the pane's own terminal, never the window attachment, so
    // "the window is attached to t1" cannot appear among its inputs.
    expect(acceptsPointer({ ...live, viewportTerminal: "t2", boundTerminal: "t2" })).toBe(true);
  });

  it("refuses a pane whose grid still shows another terminal", () => {
    expect(acceptsPointer({ ...live, viewportTerminal: "t1", boundTerminal: "t2" })).toBe(false);
  });

  it("refuses when hidden, mid-switch, empty or disconnected", () => {
    expect(acceptsPointer({ ...live, active: false })).toBe(false);
    expect(acceptsPointer({ ...live, selectionPending: true })).toBe(false);
    expect(acceptsPointer({ ...live, viewportTerminal: null })).toBe(false);
    expect(acceptsPointer({ ...live, hasRows: false })).toBe(false);
    expect(acceptsPointer({ ...live, connected: false })).toBe(false);
  });

  it("treats an unset active flag as active", () => {
    expect(acceptsPointer({ ...live, active: undefined })).toBe(true);
  });
});
