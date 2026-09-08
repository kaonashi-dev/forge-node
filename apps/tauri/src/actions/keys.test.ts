import { describe, expect, it } from "vitest";
import { describeChord, matches, parseChord } from "./keys";

function event(init: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
  return {
    key: init.key,
    code: init.code ?? "",
    ctrlKey: init.ctrlKey ?? false,
    altKey: init.altKey ?? false,
    shiftKey: init.shiftKey ?? false,
    metaKey: init.metaKey ?? false,
  } as KeyboardEvent;
}

describe("parseChord", () => {
  it("reads modifiers left to right", () => {
    expect(parseChord("cmd-shift-o")).toMatchObject({
      key: "o",
      code: "KeyO",
      meta: true,
      shift: true,
      ctrl: false,
      alt: false,
    });
  });

  it("takes a bare key", () => {
    expect(parseChord("down")).toMatchObject({ key: "down", code: "ArrowDown", meta: false });
    expect(parseChord("j")).toMatchObject({ key: "j", code: "KeyJ" });
  });

  it("keeps punctuation that shares its spelling with the separator", () => {
    expect(parseChord("cmd--")).toMatchObject({ key: "-", code: "Minus", meta: true });
    expect(parseChord("cmd-,")).toMatchObject({ key: ",", code: "Comma", meta: true });
    expect(parseChord("cmd-shift-]")).toMatchObject({ key: "]", code: "BracketRight" });
  });

  it("refuses a spec that is only modifiers", () => {
    expect(() => parseChord("cmd-shift")).toThrow();
  });
});

describe("matches", () => {
  // `cmd-shift-[` produces `{` on a US layout and something else again on a
  // Latin one. The position does not move, and neither should the shortcut.
  it("prefers the physical key over the character it produced", () => {
    const chord = parseChord("cmd-shift-[");
    expect(
      matches(chord, event({ key: "{", code: "BracketLeft", metaKey: true, shiftKey: true })),
    ).toBe(true);
  });

  it("falls back to the key name when there is no code", () => {
    expect(matches(parseChord("cmd-k"), event({ key: "k", metaKey: true }))).toBe(true);
    expect(matches(parseChord("up"), event({ key: "ArrowUp" }))).toBe(true);
  });

  // A chord must not swallow the one drawn beside it.
  it("compares modifiers exactly", () => {
    const chord = parseChord("cmd-k");
    expect(matches(chord, event({ key: "k", code: "KeyK", metaKey: true, shiftKey: true }))).toBe(
      false,
    );
    expect(matches(chord, event({ key: "k", code: "KeyK" }))).toBe(false);
    expect(matches(chord, event({ key: "k", code: "KeyK", metaKey: true }))).toBe(true);
  });

  it("does not match a different key", () => {
    expect(matches(parseChord("cmd-k"), event({ key: "j", code: "KeyJ", metaKey: true }))).toBe(
      false,
    );
  });
});

describe("describeChord", () => {
  it("renders something a person can read", () => {
    const text = describeChord(parseChord("cmd-shift-o"));
    expect(text).toMatch(/O$/);
    expect(text.length).toBeGreaterThan(1);
  });
});
