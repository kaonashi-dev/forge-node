import { describe, expect, it } from "vitest";

import { editorKeyForMeta } from "./editorChords";

function meta(
  key: string,
  overrides: Partial<{
    metaKey: boolean;
    ctrlKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
  }> = {},
) {
  return { key, metaKey: true, ctrlKey: false, altKey: false, shiftKey: false, ...overrides };
}

describe("the editor's platform chords", () => {
  it("delivers the platform's edit chords as the editor's own keys", () => {
    for (const key of ["s", "c", "x"]) {
      expect(editorKeyForMeta(meta(key))).toEqual({ key, ctrl: true, alt: false, shift: false });
    }
  });

  it("leaves paste and every modified chord to the platform", () => {
    // The `paste` event is what carries the clipboard; a claimed ⌘V would
    // preventDefault the gesture the WebView needs.
    expect(editorKeyForMeta(meta("v"))).toBeNull();
    expect(editorKeyForMeta(meta("c", { shiftKey: true }))).toBeNull();
    expect(editorKeyForMeta(meta("c", { altKey: true }))).toBeNull();
    expect(editorKeyForMeta(meta("c", { ctrlKey: true }))).toBeNull();
    expect(editorKeyForMeta(meta("k"))).toBeNull();
    expect(editorKeyForMeta(meta("c", { metaKey: false }))).toBeNull();
  });
});
