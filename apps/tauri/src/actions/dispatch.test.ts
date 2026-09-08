import { beforeEach, describe, expect, it, vi } from "vitest";
import { APP, EDITOR, FILES, TERMINAL, type Binding } from "./actions";
import {
  enterContext,
  invokeAction,
  installKeymap,
  isTypingTarget,
  registerAction,
  resetActions,
  resolve,
  selectAllIn,
} from "./dispatch";
import { isSelectAll, parseChord } from "./keys";

function event(key: string, code: string, meta = true): KeyboardEvent {
  return {
    key,
    code,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: meta,
  } as KeyboardEvent;
}

const bindings: Binding[] = [
  { chord: parseChord("cmd-c"), action: "copy_terminal", context: TERMINAL },
  { chord: parseChord("cmd-c"), action: "about", context: APP },
  { chord: parseChord("cmd-t"), action: "new_terminal", context: APP },
];

describe("resolve", () => {
  const bound = () => true;

  // A binding in a nested context outbids the same chord on `App`, which is
  // how ⌘C copies the terminal rather than doing whatever the shell would.
  it("lets the innermost active context win", () => {
    expect(resolve(event("c", "KeyC"), [TERMINAL, APP], bindings, bound)?.action).toBe(
      "copy_terminal",
    );
    expect(resolve(event("c", "KeyC"), [APP], bindings, bound)?.action).toBe("about");
  });

  it("ignores a context that is not active", () => {
    expect(resolve(event("t", "KeyT"), [TERMINAL], bindings, bound)).toBeNull();
  });

  // A chord with nothing behind it must fall through rather than be swallowed:
  // otherwise ⌘C would be consumed by a pane that cannot copy.
  it("skips a binding whose action nothing answers to", () => {
    const onlyApp = (id: string) => id === "about";
    expect(resolve(event("c", "KeyC"), [TERMINAL, APP], bindings, onlyApp as never)?.action).toBe(
      "about",
    );
  });

  it("returns null when nothing matches", () => {
    expect(resolve(event("z", "KeyZ"), [APP], bindings, bound)).toBeNull();
  });
});

describe("action registry", () => {
  beforeEach(resetActions);

  it("runs the most recent registration and restores the one before it", () => {
    const first = vi.fn();
    const second = vi.fn();
    registerAction("new_terminal", first);
    const undo = registerAction("new_terminal", second);
    invokeAction("new_terminal");
    expect(second).toHaveBeenCalledOnce();
    expect(first).not.toHaveBeenCalled();
    undo();
    invokeAction("new_terminal");
    expect(first).toHaveBeenCalledOnce();
  });

  it("passes the chord's argument through", () => {
    const handler = vi.fn();
    registerAction("focus_session", handler);
    invokeAction("focus_session", 4);
    expect(handler).toHaveBeenCalledWith(4);
  });

  it("reports that nothing ran when no handler is registered", () => {
    expect(invokeAction("about")).toBe(false);
  });
});

describe("contexts", () => {
  beforeEach(resetActions);

  // Two panes can hold one context during a transition, and the first to leave
  // must not close it for the other.
  it("counts entries rather than flagging them", () => {
    const leaveA = enterContext(TERMINAL);
    const leaveB = enterContext(TERMINAL);
    leaveA();
    expect(resolve(event("c", "KeyC"), [TERMINAL], bindings, () => true)?.action).toBe(
      "copy_terminal",
    );
    leaveB();
  });

  it("is idempotent when a leave is called twice", () => {
    const leave = enterContext(TERMINAL);
    leave();
    expect(() => leave()).not.toThrow();
  });
});

describe("typing targets", () => {
  const bare: Binding = { chord: parseChord("j"), action: "file_tree_next", context: FILES };
  const stroke = (key: string, meta = false) => event(key, `Key${key.toUpperCase()}`, meta);

  it("leaves a bare letter alone while it is being typed into a field", () => {
    // The tree binds `j`; its own filter box is inside the tree's context, and
    // without this every `j` typed into it would move the selection instead.
    expect(resolve(stroke("j"), [FILES], [bare], () => true, true)).toBeNull();
  });

  it("still fires that letter when nothing is taking text", () => {
    expect(resolve(stroke("j"), [FILES], [bare], () => true, false)).toBe(bare);
  });

  it("keeps a modified chord working inside a field", () => {
    // `cmd-s` has to save while the caret is in the editor; only *bare* keys
    // are ambiguous with typing.
    const save: Binding = { chord: parseChord("cmd-s"), action: "save_file", context: EDITOR };
    expect(resolve(stroke("s", true), [EDITOR], [save], () => true, true)).toBe(save);
  });
});

describe("isTypingTarget", () => {
  /* The real argument is an `EventTarget`; only three of its properties are
     read, and vitest runs in a node environment with no `HTMLElement` to
     build. The cast is what says so. */
  const target = (shape: { tagName?: string; type?: string; isContentEditable?: boolean }) =>
    isTypingTarget(shape as unknown as EventTarget);

  it("counts a text input, a textarea, a select and a contenteditable", () => {
    expect(target({ tagName: "INPUT", type: "text" })).toBe(true);
    expect(target({ tagName: "INPUT" })).toBe(true);
    expect(target({ tagName: "TEXTAREA" })).toBe(true);
    expect(target({ tagName: "SELECT" })).toBe(true);
    // CodeMirror's content element, which must see every key it is given.
    expect(target({ tagName: "DIV", isContentEditable: true })).toBe(true);
  });

  it("does not count a checkbox: the tree's chords still work from its chips", () => {
    expect(target({ tagName: "INPUT", type: "checkbox" })).toBe(false);
  });

  it("does not count a plain element, or nothing at all", () => {
    expect(target({ tagName: "DIV" })).toBe(false);
    expect(isTypingTarget(null)).toBe(false);
  });
});

describe("selectAllIn", () => {
  const fake = (shape: { tagName: string; className?: string; select?: () => void }) =>
    ({
      tagName: shape.tagName,
      classList: { contains: (name: string) => name === shape.className },
      select: shape.select,
    }) as unknown as EventTarget;

  it("selects an input and a textarea", () => {
    const select = vi.fn();
    expect(selectAllIn(fake({ tagName: "INPUT", select }))).toBe(true);
    expect(selectAllIn(fake({ tagName: "TEXTAREA", select }))).toBe(true);
    expect(select).toHaveBeenCalledTimes(2);
  });

  it("leaves the terminal's hidden textarea alone", () => {
    const select = vi.fn();
    expect(selectAllIn(fake({ tagName: "TEXTAREA", className: "terminal-keys", select }))).toBe(
      false,
    );
    expect(select).not.toHaveBeenCalled();
  });

  it("does not claim a contenteditable, where the editor's keymap owns ⌘A", () => {
    expect(selectAllIn({ tagName: "DIV", isContentEditable: true } as unknown as EventTarget)).toBe(
      false,
    );
  });
});

describe("isSelectAll", () => {
  const stroke = (
    key: string,
    mods: { meta?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean } = {},
  ) =>
    ({
      key,
      altKey: mods.alt ?? false,
      shiftKey: mods.shift ?? false,
      metaKey: mods.meta ?? false,
      ctrlKey: mods.ctrl ?? false,
    }) as KeyboardEvent;

  it.each([
    { platform: "MacIntel", modifier: "meta", other: "ctrl" },
    { platform: "Linux x86_64", modifier: "ctrl", other: "meta" },
  ])(
    "matches select-all on $platform without stealing modified chords",
    ({ platform, modifier, other }) => {
      vi.stubGlobal("navigator", { platform, userAgent: "" });
      try {
        expect(isSelectAll(stroke("a", { [modifier]: true }))).toBe(true);
        expect(isSelectAll(stroke("A", { [modifier]: true }))).toBe(true);
        expect(isSelectAll(stroke("a", { [other]: true }))).toBe(false);
        expect(isSelectAll(stroke("a", { [modifier]: true, shift: true }))).toBe(false);
        expect(isSelectAll(stroke("a", { [modifier]: true, alt: true }))).toBe(false);
        expect(isSelectAll(stroke("a", { meta: true, ctrl: true }))).toBe(false);
        expect(isSelectAll(stroke("b", { [modifier]: true }))).toBe(false);
        expect(isSelectAll(stroke("a"))).toBe(false);
      } finally {
        vi.unstubAllGlobals();
      }
    },
  );
});

describe("clipboard in file search modals", () => {
  it("lets the native field receive paste even when an app shortcut claims it", () => {
    vi.stubGlobal("navigator", { platform: "MacIntel", userAgent: "" });
    let listener: ((event: KeyboardEvent) => void) | undefined;
    vi.stubGlobal("window", {
      addEventListener: (_name: string, handler: (event: KeyboardEvent) => void) => {
        listener = handler;
      },
      removeEventListener: () => undefined,
    });
    const action = vi.fn();
    const leave = enterContext(APP);
    const unbind = registerAction("paste_terminal", action);
    const stop = installKeymap(() => [
      { chord: parseChord("cmd-v"), action: "paste_terminal", context: APP },
    ]);
    try {
      for (const target of [
        { tagName: "INPUT", type: "search" },
        { tagName: "TEXTAREA" },
        { tagName: "DIV", isContentEditable: true },
      ]) {
        const preventDefault = vi.fn();
        listener?.({
          ...event("v", "KeyV"),
          target,
          preventDefault,
          stopPropagation: vi.fn(),
        } as unknown as KeyboardEvent);
        expect(preventDefault).not.toHaveBeenCalled();
      }
      expect(action).not.toHaveBeenCalled();
    } finally {
      stop();
      unbind();
      leave();
      vi.unstubAllGlobals();
    }
  });
});
