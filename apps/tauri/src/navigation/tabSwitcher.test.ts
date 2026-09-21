import { beforeEach, describe, expect, it, vi } from "vitest";
import { createEffect, createRoot, createSignal } from "solid-js";
import {
  bindTabSwitcherCommit,
  chooseTabSwitcher,
  pinTabSwitcher,
  recordTabFocus,
  releaseTabSwitcher,
  resetTabSwitcher,
  stepTabSwitcher,
  tabSwitcherView,
} from "./tabSwitcher";
import type { SwitchTarget } from "./tabTargets";

// The gesture listens on `window` for the release the keymap cannot express;
// the suite runs without a DOM, so the listeners are collected here instead.
const listeners = new Map<string, Set<(event: KeyboardEvent) => void>>();
vi.stubGlobal("window", {
  addEventListener(type: string, listener: (event: KeyboardEvent) => void) {
    const set = listeners.get(type) ?? new Set();
    set.add(listener);
    listeners.set(type, set);
  },
  removeEventListener(type: string, listener: (event: KeyboardEvent) => void) {
    listeners.get(type)?.delete(listener);
  },
});

function release(key: string): void {
  for (const listener of new Set(listeners.get("keyup"))) {
    listener({ key } as KeyboardEvent);
  }
}

const FILE: SwitchTarget = {
  kind: "view",
  workspace: "w1",
  view: { kind: "editor-terminal", session: "e1", path: "src/main.ts" },
};
const OTHER_FILE: SwitchTarget = {
  kind: "view",
  workspace: "w1",
  view: { kind: "editor-terminal", session: "e2", path: "src/other.ts" },
};
const TERMINAL: SwitchTarget = { kind: "session", id: "t1" };
const local = [FILE, TERMINAL];

let committed: SwitchTarget | null = null;

beforeEach(() => {
  resetTabSwitcher();
  listeners.clear();
  committed = null;
  bindTabSwitcherCommit((target) => {
    committed = target;
  });
});

describe("tabSwitcher", () => {
  it("records reactive focus without subscribing the effect to history", () => {
    const [focused, setFocused] = createSignal("session:t1");
    let runs = 0;
    let dispose = () => {};
    try {
      createRoot((cleanup) => {
        dispose = cleanup;
        createEffect(() => {
          const key = focused();
          if (++runs > 10) throw new Error("Focus history retriggered its own effect");
          recordTabFocus(key);
        });
      });
      expect(runs).toBe(1);

      setFocused("view:w1:editor-terminal:e1");
      expect(runs).toBe(2);
      recordTabFocus("view:w1:editor-terminal:e2");
      expect(runs).toBe(2);
    } finally {
      dispose();
    }
  });

  it("returns to the file that was on screen before the terminal", () => {
    // Three panes, and the one to come back to is *second* in the strip: with
    // two, "the other pane" is the right answer for the wrong reason, and a
    // ring that had forgotten the file would still look correct here.
    recordTabFocus("view:w1:editor-terminal:e1");
    recordTabFocus("view:w1:editor-terminal:e2");
    recordTabFocus("session:t1");

    stepTabSwitcher(1, [FILE, OTHER_FILE, TERMINAL], "session:t1", ["t1"]);
    release("Control");

    expect(committed).toEqual(OTHER_FILE);
  });

  it("toggles with the Code tab even while nothing is parked in it", () => {
    // One terminal and an empty Code tab is two tabs in the strip; a ring that
    // only counted the terminal opened nothing at all.
    const code: SwitchTarget = { kind: "code", workspace: "w1" };
    recordTabFocus("session:t1");

    stepTabSwitcher(1, [code, TERMINAL], "session:t1", ["t1"]);
    release("Control");

    expect(committed).toEqual(code);
  });

  it("walks the files and the terminals in one ring", () => {
    recordTabFocus("session:t1");
    stepTabSwitcher(1, local, "session:t1", ["t1"]);
    release("Tab");
    stepTabSwitcher(1, local, "session:t1", ["t1"]);

    expect(tabSwitcherView()?.targets).toEqual([TERMINAL, FILE]);
    expect(tabSwitcherView()?.index).toBe(0);
  });

  it("hands the list to the pointer, so releasing Control leaves it open to click", () => {
    recordTabFocus("session:t1");
    stepTabSwitcher(1, local, "session:t1", ["t1"]);
    release("Tab");
    stepTabSwitcher(1, local, "session:t1", ["t1"]);
    pinTabSwitcher();

    release("Control");
    expect(committed).toBeNull();
    expect(tabSwitcherView()).not.toBeNull();

    chooseTabSwitcher(1);
    expect(committed).toEqual(FILE);
    expect(tabSwitcherView()).toBeNull();
  });

  it("gives the release back to the keyboard when the pointer leaves", () => {
    // Pinning is not a latch: a cursor that merely crossed the card must not
    // take Control-up with it for the rest of the gesture.
    recordTabFocus("session:t1");
    stepTabSwitcher(1, local, "session:t1", ["t1"]);
    pinTabSwitcher();
    releaseTabSwitcher();

    release("Control");
    expect(committed).toEqual(FILE);
    expect(tabSwitcherView()).toBeNull();
  });

  it("keeps accepting Tab after a ring too short to open", () => {
    // Nothing was armed for a ring of one, so no keyup is coming to clear the
    // repeat latch — and the chord would stay dead for the rest of the session.
    stepTabSwitcher(1, [TERMINAL], "session:t1", ["t1"]);
    expect(tabSwitcherView()).toBeNull();

    stepTabSwitcher(1, local, "session:t1", ["t1"]);
    release("Control");
    expect(committed).toEqual(FILE);
  });
});
