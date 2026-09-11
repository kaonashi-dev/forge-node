import { describe, expect, it } from "vitest";
import { SIDEBAR_VIEWS } from "../store/sidebarStore";
import { ICONS } from "../theme/icons/forgeIcons";
import { VIEW_ICONS } from "./sidebarViews";

describe("VIEW_ICONS", () => {
  /* A view with no glyph draws a blank square, and a glyph with no component
     throws at render. Both are type errors at the call site and this is the
     test that catches a `Record` that was cast around one. */
  it("covers every sidebar view with a registered glyph", () => {
    expect(Object.keys(VIEW_ICONS).sort()).toEqual([...SIDEBAR_VIEWS].sort());
    for (const icon of Object.values(VIEW_ICONS)) {
      expect(Object.keys(ICONS)).toContain(icon);
    }
  });
});
