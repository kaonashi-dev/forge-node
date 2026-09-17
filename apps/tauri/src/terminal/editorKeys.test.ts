import { describe, expect, it } from "vitest";
import {
  MOD_ALT,
  MOD_CONTROL,
  MOD_META,
  MOD_SHIFT,
  inputFor,
  keyOf,
  modifiersOf,
  textInput,
} from "./editorKeys";

function press(init: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
  return {
    shiftKey: false,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    isComposing: false,
    keyCode: 0,
    ...init,
  } as KeyboardEvent;
}

describe("keyOf", () => {
  it("names the keys a browser names differently", () => {
    expect(keyOf(press({ key: "ArrowLeft" }))).toBe("Left");
    expect(keyOf(press({ key: "Escape" }))).toBe("Escape");
    expect(keyOf(press({ key: "F7" }))).toEqual({ Function: 7 });
  });

  it("carries a printable key as its character", () => {
    expect(keyOf(press({ key: "a" }))).toEqual({ Char: "a" });
    expect(keyOf(press({ key: "é" }))).toEqual({ Char: "é" });
    // An astral character is two UTF-16 units and one code point.
    expect(keyOf(press({ key: "😀" }))).toEqual({ Char: "😀" });
  });

  /* A bare modifier and a name this build does not know both mean "nothing to
     send": a guess here is an edit nobody asked for. */
  it("refuses a bare modifier and an unknown name", () => {
    expect(keyOf(press({ key: "Shift" }))).toBeNull();
    expect(keyOf(press({ key: "AudioVolumeUp" }))).toBeNull();
    expect(keyOf(press({ key: "F13" }))).toBeNull();
  });

  /* Mid-composition the key is the half an IME is still choosing between; the
     committed text arrives as its own event. */
  it("stays out of an IME composition", () => {
    expect(keyOf(press({ key: "a", isComposing: true }))).toBeNull();
    expect(keyOf(press({ key: "Process", keyCode: 229 }))).toBeNull();
  });
});

describe("modifiersOf", () => {
  it("packs each modifier into its own bit", () => {
    expect(modifiersOf({ shiftKey: true, ctrlKey: false, altKey: false, metaKey: false })).toBe(
      MOD_SHIFT,
    );
    expect(modifiersOf({ shiftKey: false, ctrlKey: true, altKey: true, metaKey: true })).toBe(
      MOD_CONTROL | MOD_ALT | MOD_META,
    );
  });
});

describe("inputFor", () => {
  it("uses the same Mac save and comment chords as the terminal surface", () => {
    expect(inputFor(press({ key: "s", metaKey: true }))).toEqual({
      Key: { key: { Char: "s" }, modifiers: MOD_CONTROL },
    });
    expect(inputFor(press({ key: "?", metaKey: true, shiftKey: true }))).toEqual({
      Key: { key: { Char: "_" }, modifiers: MOD_CONTROL },
    });
    expect(inputFor(press({ key: "v", metaKey: true }))).toBeNull();
    expect(inputFor(press({ key: "w", metaKey: true }))).toBeNull();
    expect(inputFor(press({ key: "s", metaKey: true, isComposing: true }))).toBeNull();
  });
  it("builds the event the daemon deserializes", () => {
    expect(inputFor(press({ key: "s", ctrlKey: true }))).toEqual({
      Key: { key: { Char: "s" }, modifiers: MOD_CONTROL },
    });
    expect(inputFor(press({ key: "Shift", shiftKey: true }))).toBeNull();
  });
});

describe("textInput", () => {
  it("carries committed text whole", () => {
    expect(textInput("héllo 😀")).toEqual({ Text: "héllo 😀" });
  });
});
