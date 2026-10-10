import { describe, expect, it } from "vitest";

import { editorKeyForChord } from "./editorChords";

function meta(
  key: string,
  overrides: Partial<{
    code: string;
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
      expect(editorKeyForChord(meta(key))).toEqual({ key, ctrl: true, alt: false, shift: false });
    }
  });

  it("maps the platform's find chords onto the editor's keys", () => {
    expect(editorKeyForChord(meta("f"))).toEqual({
      key: "f",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForChord(meta("g"))).toEqual({
      key: "n",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForChord(meta("g", { shiftKey: true }))).toEqual({
      key: "b",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForChord(meta("f", { altKey: true }))).toEqual({
      key: "r",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForChord(meta("l", { altKey: true }))).toEqual({
      key: "g",
      ctrl: true,
      alt: false,
      shift: false,
    });
  });

  it("maps slash and shifted slash to a PTY-safe comment chord", () => {
    for (const event of [meta("/"), meta("/", { shiftKey: true }), meta("?", { shiftKey: true })]) {
      expect(editorKeyForChord(event)).toEqual({ key: "_", ctrl: true, alt: false, shift: false });
    }
    expect(editorKeyForChord(meta("/", { altKey: true }))).toBeNull();
  });

  it("maps Alt-slash even when Option changes the character", () => {
    for (const event of [
      meta("/", { metaKey: false, altKey: true }),
      meta("÷", { code: "Slash", metaKey: false, altKey: true }),
      meta("/", { code: "Digit7", metaKey: false, altKey: true, shiftKey: true }),
    ]) {
      expect(editorKeyForChord(event)).toEqual({ key: "_", ctrl: true, alt: false, shift: false });
    }
  });

  it("does not claim other slash chords or an IME composition", () => {
    for (const event of [
      meta("/", { metaKey: false }),
      meta("/", { metaKey: false, altKey: true, ctrlKey: true }),
      meta("?", { code: "Slash", metaKey: false, altKey: true, shiftKey: true }),
      { ...meta("÷", { code: "Slash", metaKey: false, altKey: true }), isComposing: true },
      { ...meta("÷", { code: "Slash", metaKey: false, altKey: true }), keyCode: 229 },
      meta("Process", { code: "Slash", metaKey: false, altKey: true }),
      meta("÷", { code: "KeyD", metaKey: false, altKey: true }),
    ]) {
      expect(editorKeyForChord(event)).toBeNull();
    }
  });

  it("delivers select-all, undo and redo", () => {
    expect(editorKeyForChord(meta("a"))).toEqual({
      key: "a",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForChord(meta("z"))).toEqual({
      key: "z",
      ctrl: true,
      alt: false,
      shift: false,
    });
    expect(editorKeyForChord(meta("z", { shiftKey: true }))).toEqual({
      key: "y",
      ctrl: true,
      alt: false,
      shift: false,
    });
  });

  it("leaves paste and every chord it does not own to the platform", () => {
    expect(editorKeyForChord(meta("v"))).toBeNull();
    expect(editorKeyForChord(meta("c", { shiftKey: true }))).toBeNull();
    expect(editorKeyForChord(meta("c", { altKey: true }))).toBeNull();
    expect(editorKeyForChord(meta("c", { ctrlKey: true }))).toBeNull();
    expect(editorKeyForChord(meta("k"))).toBeNull();
    expect(editorKeyForChord(meta("w"))).toBeNull();
    expect(editorKeyForChord(meta("f", { altKey: true, shiftKey: true }))).toBeNull();
    expect(editorKeyForChord(meta("c", { metaKey: false }))).toBeNull();
  });
});
