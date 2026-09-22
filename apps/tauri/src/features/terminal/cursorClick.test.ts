import { describe, expect, it } from "vitest";
import { Viewport } from "../../shared/cell-grid/viewport";
import { DEFAULT_MODES } from "../../contracts/terminal";
import { CursorClick } from "./cursorClick";

const click = {
  button: 0,
  detail: 1,
  clientX: 80,
  clientY: 20,
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
};
const point = { line: 1, col: 8 };

function fixture() {
  const viewport = new Viewport();
  viewport.terminal = "terminal-A";
  viewport.seq = 7;
  viewport.cols = 40;
  viewport.rows = [null, { w: false, r: [] }];
  viewport.cursor = { line: 1, col: 24, shape: "beam", visible: true };
  viewport.modes = { ...DEFAULT_MODES };
  return { gesture: new CursorClick(), viewport };
}

describe("terminal cursor clicks", () => {
  it("cancels a click when the tab changes or the window loses focus", () => {
    const { gesture, viewport } = fixture();
    gesture.begin(click, point, viewport);
    gesture.cancel();
    expect(gesture.finish(click, point, viewport)).toBeNull();
  });
  it("waits for release and consumes a click only once", () => {
    const { gesture, viewport } = fixture();
    gesture.begin(click, point, viewport);
    expect(gesture.finish(click, point, viewport)).toEqual({
      terminal_id: "terminal-A",
      seq: 7,
      row: 1,
      col: 8,
    });
    expect(gesture.finish(click, point, viewport)).toBeNull();
  });

  it("leaves a drag as selection even if it returns to its starting cell", () => {
    const { gesture, viewport } = fixture();
    gesture.begin(click, point, viewport);
    gesture.move({ clientX: 100, clientY: 20 });
    expect(gesture.finish(click, point, viewport)).toBeNull();
    gesture.begin(click, point, viewport);
    expect(gesture.finish({ ...click, clientX: 100 }, point, viewport)).toBeNull();
  });

  it.each([
    { detail: 2 },
    { detail: 3 },
    { button: 1 },
    { button: 2 },
    { altKey: true },
    { ctrlKey: true },
    { metaKey: true },
    { shiftKey: true },
  ])("keeps modified and multiple clicks for their existing gestures: %j", (change) => {
    const { gesture, viewport } = fixture();
    gesture.begin({ ...click, ...change }, point, viewport);
    expect(gesture.finish(click, point, viewport)).toBeNull();
    gesture.begin(click, point, viewport);
    expect(gesture.finish({ ...click, ...change }, point, viewport)).toBeNull();
  });

  it.each(["terminal", "seq", "scroll"])("rejects a changed %s during the gesture", (change) => {
    const { gesture, viewport } = fixture();
    gesture.begin(click, point, viewport);
    if (change === "terminal") viewport.terminal = "terminal-B";
    if (change === "seq") viewport.seq += 1;
    if (change === "scroll") viewport.scrollOffset = 1;
    expect(gesture.finish(click, point, viewport)).toBeNull();
  });

  it.each(["scroll", "fullscreen", "mouse", "hidden"])("ignores %s viewports", (mode) => {
    const { gesture, viewport } = fixture();
    if (mode === "scroll") viewport.scrollOffset = 1;
    if (mode === "fullscreen") viewport.modes.alt_screen = true;
    if (mode === "mouse") viewport.modes.mouse_mode = "Normal";
    if (mode === "hidden") viewport.cursor.visible = false;
    gesture.begin(click, point, viewport);
    expect(gesture.finish(click, point, viewport)).toBeNull();
  });
});
