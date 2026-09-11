import { describe, expect, it, vi } from "vitest";
import {
  ACTIONS,
  CONTEXT_ORDER,
  defaultBindings,
  FOCUSABLE_SESSIONS,
  type ActionId,
} from "./actions";
import { describeChord, PASTE_CHORD, type Chord } from "./keys";

function chordKey(chord: Chord): string {
  return [
    chord.ctrl ? "ctrl" : "",
    chord.alt ? "alt" : "",
    chord.shift ? "shift" : "",
    chord.meta ? "cmd" : "",
    chord.key,
  ]
    .filter(Boolean)
    .join("-");
}

describe("the binding table", () => {
  const bindings = defaultBindings();

  // Two bindings on one chord in one context is not a conflict the dispatcher
  // reports: `resolve` returns whichever comes first in the list, so the
  // second one is simply dead.
  it("binds each chord at most once per context", () => {
    const seen = new Map<string, ActionId>();
    for (const binding of bindings) {
      const key = `${binding.context}:${chordKey(binding.chord)}:${binding.argument ?? ""}`;
      expect(
        seen.has(key),
        `${describeChord(binding.chord)} is bound twice in ${binding.context}`,
      ).toBe(false);
      seen.set(key, binding.action);
    }
  });

  it("binds every chord to an action that exists", () => {
    const known = new Set(ACTIONS.map((action) => action.id));
    for (const binding of bindings) {
      expect(known.has(binding.action), `${binding.action} has a chord but no entry`).toBe(true);
    }
  });

  it("declares every binding in a context the dispatcher orders", () => {
    for (const binding of bindings) {
      expect(CONTEXT_ORDER).toContain(binding.context);
    }
  });

  // macOS paste is reached through the textarea’s native paste event.
  it("leaves no action unreachable", () => {
    const bound = new Set(bindings.map((binding) => binding.action));
    for (const action of ACTIONS) {
      expect(
        action.palette ||
          bound.has(action.id) ||
          (action.id === "paste_terminal" && PASTE_CHORD === null),
        `${action.id} has no chord, palette row or native handler`,
      ).toBe(true);
    }
  });

  it("numbers the session chords the way a person counts tabs", () => {
    for (const index of FOCUSABLE_SESSIONS) {
      const binding = bindings.find(
        (item) => item.action === "focus_session" && item.argument === index,
      );
      expect(binding, `no chord for tab ${index}`).toBeDefined();
      expect(binding?.chord.key).toBe(String(index));
      // On `MOD-alt`: the bare number row belongs to the sidebar.
      expect(binding?.chord.alt, `tab ${index} must carry alt`).toBe(true);
    }
  });
});

describe("the terminal's clipboard chords", () => {
  const bindings = defaultBindings();

  it("keeps copy bound, because the selection is not the browser's", () => {
    expect(bindings.some((binding) => binding.action === "copy_terminal")).toBe(true);
  });

  it("binds paste only where the chord is not the platform's own", () => {
    const bound = bindings.some((binding) => binding.action === "paste_terminal");
    expect(bound).toBe(PASTE_CHORD !== null);
  });

  it.each([
    { platform: "MacIntel", copy: "cmd-c", paste: null },
    { platform: "Linux x86_64", copy: "ctrl-shift-c", paste: "ctrl-shift-v" },
  ])("reserves the correct clipboard chords on $platform", async ({ platform, copy, paste }) => {
    vi.stubGlobal("navigator", { platform, userAgent: "" });
    vi.resetModules();
    try {
      const { defaultBindings: platformBindings } = await import("./actions");
      const { parseChord } = await import("./keys");
      const bindings = platformBindings();
      expect(bindings.find((binding) => binding.action === "copy_terminal")?.chord).toEqual(
        parseChord(copy),
      );
      const pasteBinding = bindings.find((binding) => binding.action === "paste_terminal");
      if (paste === null) {
        expect(pasteBinding).toBeUndefined();
        expect(bindings.some((binding) => chordKey(binding.chord) === "cmd-v")).toBe(false);
      } else {
        expect(pasteBinding?.chord).toEqual(parseChord(paste));
        expect(pasteBinding?.context).toBe("Terminal");
        expect(bindings.some((binding) => chordKey(binding.chord) === "ctrl-v")).toBe(false);
      }
    } finally {
      vi.unstubAllGlobals();
      vi.resetModules();
    }
  });
});
