import { describe, expect, it } from "vitest";
import {
  FEATURE_COMPOSE_VIEW,
  TERMINAL_VIEW,
  closeOthers,
  closeToRight,
  closeView,
  emptyViews,
  focusView,
  stepView,
  openView,
  hasCode,
  strip,
  viewKey,
  viewLabel,
  viewTitle,
} from "./views";

const diff = { kind: "diff" } as const;
const lib = { kind: "editor", path: "crates/client/src/lib.rs" } as const;
const store = { kind: "editor", path: "crates/client/src/store.rs" } as const;

describe("parked views", () => {
  it("starts on the terminal with nothing else open", () => {
    const views = emptyViews();
    expect(views.active).toEqual(TERMINAL_VIEW);
    expect(strip(views)).toEqual([]);
    expect(hasCode(views)).toBe(false);
  });

  // The terminal has its own tab in the window strip. Listed here too, the
  // same pane would have two places to be selected from.
  it("keeps the terminal off the Code strip", () => {
    const views = openView(openView(emptyViews(), diff), lib);
    expect(strip(views).map(viewKey)).toEqual(["diff", "editor:crates/client/src/lib.rs"]);
    expect(hasCode(views)).toBe(true);
  });

  // A strip that reshuffles itself as you revisit tabs is one you have to read
  // before every click.
  it("focuses without duplicating or reordering on re-open", () => {
    const views = openView(openView(openView(emptyViews(), diff), lib), diff);
    expect(views.open.map(viewKey)).toEqual(["diff", "editor:crates/client/src/lib.rs"]);
    expect(views.active).toEqual(diff);
  });

  // A draft and the feature it becomes are two tabs, not one: `#3` is a number
  // the daemon has not minted while the draft is open.
  it("keeps the feature draft apart from a numbered feature", () => {
    const numbered = { kind: "feature", id: 3 } as const;
    expect(viewKey(FEATURE_COMPOSE_VIEW)).toBe("feature_compose");
    expect(viewKey(numbered)).toBe("feature:3");
    expect(viewLabel(FEATURE_COMPOSE_VIEW)).toBe("New Feature");
    expect(strip(openView(openView(emptyViews(), FEATURE_COMPOSE_VIEW), numbered))).toHaveLength(2);
  });

  it("names an editor after its file, not its path", () => {
    expect(viewLabel(lib)).toBe("lib.rs");
    expect(viewLabel(diff)).toBe("Diff");
  });

  // Two `mod.rs` tabs are indistinguishable by label alone.
  it("keeps the whole path reachable as the tab's title", () => {
    expect(viewTitle(lib)).toBe("crates/client/src/lib.rs");
    expect(viewTitle(diff)).toBe("Diff");
  });

  describe("closeView", () => {
    // Never the tab to the right: closing a run left-to-right would walk the
    // selection through every tab it is about to close.
    it("falls back to the neighbour on the left", () => {
      const views = openView(openView(openView(emptyViews(), diff), lib), store);
      expect(closeView(views, store).active).toEqual(lib);
    });

    it("falls back to the terminal when the last one closes", () => {
      const views = openView(emptyViews(), diff);
      expect(closeView(views, diff)).toEqual(emptyViews());
    });

    it("leaves the selection alone when another tab closes", () => {
      const views = openView(openView(emptyViews(), diff), lib);
      const after = closeView(views, diff);
      expect(after.active).toEqual(lib);
      expect(after.open.map(viewKey)).toEqual(["editor:crates/client/src/lib.rs"]);
    });

    it("ignores a view that is not open", () => {
      const views = openView(emptyViews(), diff);
      expect(closeView(views, store)).toEqual(views);
    });
  });

  describe("focusView", () => {
    it("refuses a view that is not open", () => {
      const views = openView(emptyViews(), diff);
      expect(focusView(views, store).active).toEqual(diff);
    });

    // The terminal is the floor the strip sits on: it is always reachable.
    it("always accepts the terminal", () => {
      const views = openView(emptyViews(), diff);
      expect(focusView(views, TERMINAL_VIEW).active).toEqual(TERMINAL_VIEW);
    });
  });
});

describe("closeOthers", () => {
  const views = { open: [editor("a"), editor("b"), editor("c")], active: editor("a") };

  it("keeps one view and makes it active", () => {
    const next = closeOthers(views, editor("b"));
    expect(next.open).toEqual([editor("b")]);
    expect(next.active).toEqual(editor("b"));
  });

  it("falls back to the terminal when the kept view is not open", () => {
    expect(closeOthers(views, editor("z")).active).toEqual(TERMINAL_VIEW);
  });
});

describe("closeToRight", () => {
  const views = { open: [editor("a"), editor("b"), editor("c")], active: editor("c") };

  it("keeps everything up to and including the anchor", () => {
    expect(closeToRight(views, editor("b")).open).toEqual([editor("a"), editor("b")]);
  });

  it("moves the selection when what was active has just been closed", () => {
    expect(closeToRight(views, editor("b")).active).toEqual(editor("b"));
  });

  it("leaves the selection where it was when it survived", () => {
    const left = { open: [editor("a"), editor("b"), editor("c")], active: editor("a") };
    expect(closeToRight(left, editor("b")).active).toEqual(editor("a"));
  });

  it("does nothing for an anchor that is not open", () => {
    expect(closeToRight(views, editor("z"))).toBe(views);
  });
});

describe("stepView", () => {
  const views = { open: [editor("a"), editor("b"), editor("c")], active: editor("b") };

  it("moves along the strip and wraps at both ends", () => {
    expect(stepView(views, 1)).toEqual(editor("c"));
    expect(stepView({ ...views, active: editor("c") }, 1)).toEqual(editor("a"));
    expect(stepView({ ...views, active: editor("a") }, -1)).toEqual(editor("c"));
  });

  it("lands on an end when the terminal is what is active", () => {
    expect(stepView({ ...views, active: TERMINAL_VIEW }, 1)).toEqual(editor("a"));
    expect(stepView({ ...views, active: TERMINAL_VIEW }, -1)).toEqual(editor("c"));
  });

  it("has nowhere to go with fewer than two views", () => {
    // A shortcut that fires and changes nothing reads as broken.
    expect(stepView({ open: [editor("a")], active: editor("a") }, 1)).toBeNull();
    expect(stepView(emptyViews(), 1)).toBeNull();
  });
});

function editor(path: string) {
  return { kind: "editor" as const, path };
}
