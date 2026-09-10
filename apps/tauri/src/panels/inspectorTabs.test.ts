import { describe, expect, it } from "vitest";
import { INSPECTOR_TABS } from "../store/inspectorStore";
import { ICONS } from "../theme/icons/forgeIcons";
import { TAB_ICONS } from "./inspectorTabs";

describe("TAB_ICONS", () => {
  /* A tab with no glyph draws a blank square, and a glyph with no component
     throws at render. Both are type errors at the call site and this is the
     test that catches a `Record` that was cast around one. */
  it("covers every inspector tab with a registered glyph", () => {
    expect(Object.keys(TAB_ICONS).sort()).toEqual([...INSPECTOR_TABS].sort());
    for (const icon of Object.values(TAB_ICONS)) {
      expect(Object.keys(ICONS)).toContain(icon);
    }
  });
});
