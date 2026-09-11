import { describe, expect, it } from "vitest";
import { APP, EDITOR, FILES, type Binding } from "./actions";
import {
  conflicts,
  mergeBindings,
  parseKeymap,
  serializeKeymap,
  type KeymapOverride,
} from "./keymap";
import { parseChord } from "./keys";

const binding = (spec: string, action: string, context = APP, argument?: number): Binding =>
  ({ chord: parseChord(spec), action, context, argument }) as Binding;

describe("parseKeymap", () => {
  it("reads a stored table", () => {
    const raw = serializeKeymap([{ action: "new_terminal", context: APP, chord: "cmd-shift-t" }]);
    expect(parseKeymap(raw)).toEqual([
      { action: "new_terminal", context: APP, chord: "cmd-shift-t" },
    ]);
  });

  it("keeps an unbind, which is a real preference", () => {
    const raw = serializeKeymap([{ action: "new_terminal", context: APP, chord: null }]);
    expect(parseKeymap(raw)[0].chord).toBeNull();
  });

  it("drops one bad row rather than losing the whole keymap", () => {
    // Written by an older build, or by hand. Throwing here would cost every
    // shortcut in the app, because this is read on the way to installing them.
    const raw = JSON.stringify([
      { action: "new_terminal", context: APP, chord: "cmd-shift-t" },
      { action: "new_terminal", context: APP, chord: "not-a-real-chord" },
      { nonsense: true },
      null,
    ]);
    expect(parseKeymap(raw)).toHaveLength(1);
  });

  it("has nothing to say about missing or unparseable storage", () => {
    expect(parseKeymap(undefined)).toEqual([]);
    expect(parseKeymap("{")).toEqual([]);
    expect(parseKeymap('"a string"')).toEqual([]);
  });
});

describe("mergeBindings", () => {
  const defaults = [binding("cmd-t", "new_terminal"), binding("cmd-k", "open_command_palette")];

  it("replaces the chord an action was drawn with", () => {
    const merged = mergeBindings(
      [{ action: "new_terminal", context: APP, chord: "cmd-shift-t" }],
      defaults,
    );
    const found = merged.find((item) => item.action === "new_terminal");
    expect(found?.chord.key).toBe("t");
    expect(found?.chord.shift).toBe(true);
  });

  it("leaves every other binding alone", () => {
    const merged = mergeBindings(
      [{ action: "new_terminal", context: APP, chord: "cmd-shift-t" }],
      defaults,
    );
    expect(merged.find((item) => item.action === "open_command_palette")?.chord.key).toBe("k");
  });

  it("removes the binding entirely for an unbind", () => {
    const merged = mergeBindings([{ action: "new_terminal", context: APP, chord: null }], defaults);
    expect(merged.some((item) => item.action === "new_terminal")).toBe(false);
  });

  it("collapses an action's two default chords to the one that was set", () => {
    // The tree answers to both `down` and `j`. Rebinding it must not leave a
    // second chord behind that nobody asked for.
    const two = [binding("down", "file_tree_next", FILES), binding("j", "file_tree_next", FILES)];
    const merged = mergeBindings([{ action: "file_tree_next", context: FILES, chord: "n" }], two);
    expect(merged).toHaveLength(1);
    expect(merged[0].chord.key).toBe("n");
  });

  it("adds a chord for an action that ships unbound", () => {
    const merged = mergeBindings([{ action: "about", context: APP, chord: "cmd-i" }], defaults);
    expect(merged.find((item) => item.action === "about")?.chord.key).toBe("i");
  });

  it("keeps the argument of the binding it replaced", () => {
    // Rebinding "focus tab 3" has to still focus tab 3.
    const tabs = [binding("cmd-3", "focus_session", APP, 3)];
    const merged = mergeBindings(
      [{ action: "focus_session", context: APP, chord: "cmd-f3" }],
      tabs,
    );
    expect(merged[0].argument).toBe(3);
  });

  it("drops an override naming an action that no longer exists", () => {
    // A removed action leaves a stored override behind; without the filter it
    // would still become a binding, and the dispatcher swallows its chord.
    const merged = mergeBindings(
      [{ action: "no_such_action", context: APP, chord: "cmd-e" } as unknown as KeymapOverride],
      [binding("cmd-t", "new_terminal")],
    );
    expect(merged.some((item) => item.chord.key === "e")).toBe(false);
  });
});

describe("conflicts", () => {
  it("reports a chord two actions in one context both answer to", () => {
    // `resolve` returns the first, so the second is dead — silently, which is
    // the whole reason this is surfaced instead of resolved.
    const found = conflicts([binding("cmd-t", "new_terminal"), binding("cmd-t", "new_agent")]);
    expect(found).toHaveLength(1);
    expect(found[0].actions).toEqual(["new_terminal", "new_agent"]);
  });

  it("says nothing about the same chord in two different contexts", () => {
    expect(
      conflicts([
        binding("cmd-f", "save_file", EDITOR),
        binding("cmd-f", "file_tree_filter", FILES),
      ]),
    ).toEqual([]);
  });

  it("says nothing about one chord bound once", () => {
    expect(conflicts([binding("cmd-t", "new_terminal")])).toEqual([]);
  });

  it("does not call a numbered chord a conflict with its own siblings", () => {
    expect(
      conflicts([
        binding("cmd-2", "focus_session", APP, 2),
        binding("cmd-3", "focus_session", APP, 3),
      ]),
    ).toEqual([]);
  });
});
