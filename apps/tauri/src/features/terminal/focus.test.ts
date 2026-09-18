import { describe, expect, it } from "vitest";
import { focusTerminal, mayTakeCaret, registerTerminalFocus } from "./focus";

/* Only `tagName`/`type` are read, and vitest runs in a node environment with
   no `HTMLElement` to build. The cast is what says so. */
const el = (shape: { tagName: string; type?: string }) => shape as unknown as EventTarget;

const KEYS = el({ tagName: "TEXTAREA" });
const BUTTON = el({ tagName: "BUTTON" });
const FIELD = el({ tagName: "INPUT", type: "text" });

describe("mayTakeCaret", () => {
  it("takes the caret off the tab or rail row that was clicked", () => {
    expect(mayTakeCaret("session", "s1", BUTTON, KEYS)).toBe(true);
    expect(mayTakeCaret("session", "s1", null, KEYS)).toBe(true);
  });

  it("leaves a field alone: a session ending in the background is not a gesture", () => {
    expect(mayTakeCaret("session", "s1", FIELD, KEYS)).toBe(false);
  });

  it("re-focuses its own textarea, which is a typing target too", () => {
    expect(mayTakeCaret("session", "s1", KEYS, KEYS)).toBe(true);
  });

  it("stays off the Code tab, where the keyboard is the editor's", () => {
    expect(mayTakeCaret("code", "s1", BUTTON, KEYS)).toBe(false);
  });

  it("has nothing to focus with no session", () => {
    expect(mayTakeCaret("session", null, BUTTON, KEYS)).toBe(false);
  });
});

describe("registerTerminalFocus", () => {
  it("is a no-op before a pane has mounted", () => {
    expect(() => focusTerminal()).not.toThrow();
  });

  it("withdraws only its own registration", () => {
    let first = 0;
    let second = 0;
    const withdrawFirst = registerTerminalFocus(() => (first += 1));
    const withdrawSecond = registerTerminalFocus(() => (second += 1));
    // The pane that took over is still the one that answers.
    withdrawFirst();
    focusTerminal();
    expect([first, second]).toEqual([0, 1]);
    withdrawSecond();
    focusTerminal();
    expect([first, second]).toEqual([0, 1]);
  });
});
