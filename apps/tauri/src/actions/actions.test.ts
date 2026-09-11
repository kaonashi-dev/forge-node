import { describe, expect, it } from "vitest";
import { ACTIONS, FOCUSABLE_SESSIONS, chordFor, defaultBindings } from "./actions";
import { MOD, PASTE_CHORD } from "./keys";

describe("default bindings (actions.rs port)", () => {
  /*
   * A typo in a spec would otherwise surface only as a shortcut that silently
   * never fires, which is exactly what `parseChord`'s throw prevents.
   *
   * The count used to be pinned here as well. It was a change detector rather
   * than an invariant — every chord added to the table failed it — so what is
   * asserted now is that each binding actually carries a key and a context,
   * which is the property a typo breaks.
   */
  it("parses every binding", () => {
    expect(() => defaultBindings()).not.toThrow();
    const bindings = defaultBindings();
    expect(bindings.length).toBeGreaterThan(FOCUSABLE_SESSIONS.length);
    for (const binding of bindings) {
      expect(binding.chord.key, binding.action).not.toBe("");
      expect(binding.context, binding.action).toBeTruthy();
    }
  });

  it("binds no chord twice inside one context", () => {
    const seen = new Map<string, string>();
    for (const binding of defaultBindings()) {
      const { chord, context } = binding;
      const key = [context, chord.key, chord.ctrl, chord.alt, chord.shift, chord.meta].join("|");
      const previous = seen.get(key);
      expect(previous, `${chord.key} is bound twice in ${context}`).toBeUndefined();
      seen.set(key, binding.action);
    }
  });

  /**
   * The chord on `1` belongs to the sidebar's Projects view, not to the first
   * tab. Both halves matter: a stale focus binding left behind would win or
   * lose the dispatch by table order, so the tab side has to be gone rather
   * than outranked.
   */
  it("gives the first number chord to the rail", () => {
    const onOne = defaultBindings().filter((binding) => binding.chord.key === "1");
    expect(onOne).toHaveLength(1);
    expect(onOne[0].action).toBe("toggle_projects");
    expect(FOCUSABLE_SESSIONS).not.toContain(1);
  });

  // The editor convention is the one a new user guesses; taking it away to pay
  // for the number chord would be a net loss.
  it("keeps the rail's letter chord too", () => {
    const letter = defaultBindings().some(
      (binding) => binding.action === "toggle_sidebar" && binding.chord.key === "b",
    );
    expect(letter).toBe(true);
  });

  /*
   * Both halves of the same letter: the rail keeps `MOD-b` app-wide, and the
   * editor takes it back for go-to-definition while it holds the keyboard.
   * `EDITOR` sits above `APP` in `CONTEXT_ORDER`, so the shadow is what the
   * dispatcher does with this, not something asserted here.
   */
  it("lends the rail's letter to the editor without taking it away", () => {
    const onB = defaultBindings().filter((binding) => binding.chord.key === "b");
    expect(onB.map((binding) => [binding.action, binding.context])).toEqual([
      ["toggle_sidebar", "App"],
      ["go_to_definition", "Editor"],
    ]);
  });

  // On Linux `MOD` is `ctrl`, and an app-wide `ctrl-s` would take XOFF and
  // readline's forward search away from every running shell.
  it("scopes save to the editor, not the app", () => {
    const save = defaultBindings().find((binding) => binding.action === "save_file");
    expect(save?.context).toBe("Editor");
  });

  /*
   * Bare letters are only safe because a context nests with the element tree.
   *
   * Two trees claim them now — the file tree and the project rail (§4.1 U4) —
   * and both enter their context on focus rather than on mount, which is the
   * property that makes `j` a chord in one place and a letter everywhere else.
   */
  it("keeps every bare-letter chord inside a tree's own context", () => {
    const bare = defaultBindings().filter(
      (binding) => binding.chord.key.length === 1 && !binding.chord.meta && !binding.chord.ctrl,
    );
    expect(bare.length).toBeGreaterThan(0);
    for (const binding of bare) expect(["Files", "Sidebar"]).toContain(binding.context);
  });

  // Select-all is the field's. Binding it anywhere would make the dispatcher
  // preventDefault in capture, which is how ⌘A stopped selecting in inputs.
  it("does not claim select-all", () => {
    for (const binding of defaultBindings()) {
      const { chord } = binding;
      if (chord.key !== "a") continue;
      expect(chord.shift, binding.action).toBe(true);
    }
  });

  it("puts the clipboard and scroll chords in the terminal", () => {
    for (const action of ["copy_terminal", "scroll_up", "scroll_down"] as const) {
      expect(defaultBindings().find((b) => b.action === action)?.context).toBe("Terminal");
    }
  });

  it("registers terminal paste only when the platform does not handle it natively", () => {
    const paste = defaultBindings().find((binding) => binding.action === "paste_terminal");
    expect(paste?.context).toBe(PASTE_CHORD === null ? undefined : "Terminal");
  });

  it("gives every palette-listed action a label and a detail", () => {
    for (const action of ACTIONS) {
      expect(action.label.length, action.id).toBeGreaterThan(0);
      expect(action.detail.length, action.id).toBeGreaterThan(0);
    }
  });

  it("finds the chord an action is drawn with", () => {
    expect(chordFor("new_terminal")?.key).toBe("t");
    expect(chordFor("about")).toBeNull();
  });

  it("uses the platform's shortcut modifier", () => {
    const chord = chordFor("new_terminal");
    expect(MOD === "cmd" ? chord?.meta : chord?.ctrl).toBe(true);
  });
});
