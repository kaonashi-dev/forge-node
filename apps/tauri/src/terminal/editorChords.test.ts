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

  /*
   * ⌘F / ⌘G / ⇧⌘G are the platform's find chords and reach the editor's own
   * find, next and previous. ⌘G is *not* "go to line" here — that is ⌥⌘L —
   * because a Mac user pressing ⌘G after a search expects the next match.
   */
  it("maps the platform's find chords onto the editor's keys", () => {
    expect(editorKeyForMeta(meta("f"))).toEqual({ key: "f", ctrl: true, alt: false, shift: false });
    expect(editorKeyForMeta(meta("g"))).toEqual({ key: "n", ctrl: true, alt: false, shift: false });
    expect(editorKeyForMeta(meta("g", { shiftKey: true }))).toEqual({
      key: "b",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForMeta(meta("f", { altKey: true }))).toEqual({
      key: "r",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForMeta(meta("l", { altKey: true }))).toEqual({
      key: "g",
      ctrl: true,
      alt: false,
      shift: false,
    });
  });

  it("delivers select-all, undo and redo", () => {
    expect(editorKeyForMeta(meta("a"))).toEqual({ key: "a", ctrl: true, alt: false, shift: false });
    expect(editorKeyForMeta(meta("z"))).toEqual({ key: "z", ctrl: true, alt: false, shift: false });
    expect(editorKeyForMeta(meta("z", { shiftKey: true }))).toEqual({
      key: "y",
      ctrl: true,
      alt: false,
      shift: false,
    });
  });

  it("leaves paste and every chord it does not own to the platform", () => {
    // The `paste` event is what carries the clipboard; a claimed ⌘V would
    // preventDefault the gesture the WebView needs.
    expect(editorKeyForMeta(meta("v"))).toBeNull();
    expect(editorKeyForMeta(meta("c", { shiftKey: true }))).toBeNull();
    expect(editorKeyForMeta(meta("c", { altKey: true }))).toBeNull();
    expect(editorKeyForMeta(meta("c", { ctrlKey: true }))).toBeNull();
    expect(editorKeyForMeta(meta("k"))).toBeNull();
    expect(editorKeyForMeta(meta("w"))).toBeNull();
    expect(editorKeyForMeta(meta("f", { altKey: true, shiftKey: true }))).toBeNull();
    expect(editorKeyForMeta(meta("c", { metaKey: false }))).toBeNull();
  });
});
