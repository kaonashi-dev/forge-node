import { createRoot, createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { planFollow } from "@forge-node/file-workbench";
import { filterTree, treeRows } from "./filetree";
import { installTreeFollow } from "./treeFollow";
import type { FileTree } from "../../../contracts/workbench";
import { activeEditorPath, type WorkbenchView } from "../../../navigation/views";

/*
 * Following is decision plus wiring, and neither needs a DOM: `planFollow` is
 * the whole of what the explorer does about a follow, and `installTreeFollow`
 * is the effect that decides when to ask. The paint is out of reach here
 * (`environment: "node"`), but none of the behaviour below is in the paint.
 */

const checkout: FileTree = {
  workspace_id: "w1",
  truncated: false,
  entries: [
    { path: ".github/workflows/ci.yml", kind: "File", ignored: false },
    { path: ".github/workflows/release.yml", kind: "File", ignored: false },
    { path: "src/main.ts", kind: "File", ignored: false },
  ],
};

const listing = (query = "", collapsed: string[] = []) =>
  treeRows(filterTree(checkout, query), new Set(collapsed));

const editor = (path: string): WorkbenchView => ({ kind: "editor-terminal", session: "s-1", path });

describe("activeEditorPath", () => {
  it("names the file an editor view has open", () => {
    expect(activeEditorPath(editor("src/main.ts"))).toBe("src/main.ts");
  });

  it("is null for every view the tree must not follow", () => {
    expect(activeEditorPath({ kind: "diff" })).toBeNull();
    expect(activeEditorPath({ kind: "terminal" })).toBeNull();
    expect(activeEditorPath({ kind: "review", workspace: "w1" })).toBeNull();
    expect(activeEditorPath({ kind: "pr_review", key: "12" })).toBeNull();
    expect(activeEditorPath({ kind: "feature", id: 3 })).toBeNull();
  });
});

describe("planFollow", () => {
  const rows = listing();

  it("selects the active editor's row when the listing has it", () => {
    expect(
      planFollow({
        path: "src/main.ts",
        rows,
        selectedPath: null,
        visible: false,
        filtered: false,
        present: true,
      }),
    ).toEqual({ kind: "select", index: 4 });
  });

  it("is a no-op when that row is already selected on screen", () => {
    expect(
      planFollow({
        path: "src/main.ts",
        rows,
        selectedPath: "src/main.ts",
        visible: true,
        filtered: false,
        present: true,
      }),
    ).toEqual({ kind: "hold" });
  });

  it("scrolls back to an already-selected row that is off screen", () => {
    expect(
      planFollow({
        path: "src/main.ts",
        rows,
        selectedPath: "src/main.ts",
        visible: false,
        filtered: false,
        present: true,
      }),
    ).toEqual({ kind: "select", index: 4 });
  });

  it("reopens the ancestors a fold hides", () => {
    const folded = listing("", [".github/workflows"]);
    expect(
      planFollow({
        path: ".github/workflows/ci.yml",
        rows: folded,
        selectedPath: "src/main.ts",
        visible: true,
        filtered: false,
        present: true,
      }),
    ).toEqual({ kind: "reopen" });
  });

  it("leaves the selection alone when the tree does not have the path", () => {
    // A truncated listing, a file deleted under its open tab, one outside the
    // checkout: never row 0.
    expect(
      planFollow({
        path: "gone.ts",
        rows,
        selectedPath: "src/main.ts",
        visible: true,
        filtered: false,
        present: false,
      }),
    ).toEqual({ kind: "hold" });
  });

  it("selects a row an active filter keeps", () => {
    const filtered = listing("release");
    expect(
      planFollow({
        path: ".github/workflows/release.yml",
        rows: filtered,
        selectedPath: null,
        visible: false,
        filtered: true,
        present: true,
      }),
    ).toEqual({ kind: "select", index: 1 });
  });

  it("holds, with the filter kept, when the filter hides the path", () => {
    const filtered = listing("ci");
    expect(
      planFollow({
        path: ".github/workflows/release.yml",
        rows: filtered,
        selectedPath: "src/main.ts",
        visible: true,
        filtered: true,
        present: true,
      }),
    ).toEqual({ kind: "hold" });
  });

  /*
   * D2: filter the tree, click a result, and the filter must not disappear
   * with the follow that answers the view change. The decision is the whole of
   * what `follow` consults and it only ever selects or holds — only
   * `setFilter` and `reveal` write the query, so the host's box still matches
   * the listing.
   */
  it("keeps an active filter when a filtered row is opened", () => {
    const rows = listing("release");
    const path = ".github/workflows/release.yml";
    // The click selected the row before it opened the editor.
    expect(
      planFollow({
        path,
        rows,
        selectedPath: path,
        visible: true,
        filtered: true,
        present: true,
      }),
    ).toEqual({ kind: "hold" });
    // A follow that does move the selection inside the filter — a second tab —
    // is still only a select.
    expect(
      planFollow({
        path,
        rows,
        selectedPath: ".github/workflows/ci.yml",
        visible: true,
        filtered: true,
        present: true,
      }),
    ).toEqual({ kind: "select", index: 1 });
  });
});

describe("installTreeFollow", () => {
  function harness() {
    const [mounted, setMounted] = createSignal(false);
    const [tree, setTree] = createSignal<FileTree | null>(null);
    const [view, setView] = createSignal<WorkbenchView>({ kind: "terminal" });
    const followed: string[] = [];
    const dispose = createRoot((dispose) => {
      installTreeFollow({ mounted, tree, view, follow: (path) => followed.push(path) });
      return dispose;
    });
    return { setMounted, setTree, setView, followed, dispose };
  }

  it("holds the request until the listing lands", () => {
    const h = harness();
    h.setMounted(true);
    h.setView(editor("src/main.ts"));
    expect(h.followed).toEqual([]);
    h.setTree(checkout);
    expect(h.followed).toEqual(["src/main.ts"]);
    h.dispose();
  });

  it("waits for the panel to mount", () => {
    const h = harness();
    h.setTree(checkout);
    h.setView(editor("src/main.ts"));
    expect(h.followed).toEqual([]);
    h.setMounted(true);
    expect(h.followed).toEqual(["src/main.ts"]);
    h.dispose();
  });

  it("follows when the active editor changes", () => {
    const h = harness();
    h.setMounted(true);
    h.setTree(checkout);
    h.setView(editor("src/main.ts"));
    h.setView(editor(".github/workflows/ci.yml"));
    expect(h.followed).toEqual(["src/main.ts", ".github/workflows/ci.yml"]);
    h.dispose();
  });

  it("leaves the tree alone for a view that is not an editor", () => {
    const h = harness();
    h.setMounted(true);
    h.setTree(checkout);
    h.setView(editor("src/main.ts"));
    h.setView({ kind: "diff" });
    h.setView({ kind: "terminal" });
    expect(h.followed).toEqual(["src/main.ts"]);
    h.dispose();
  });

  // A new view object for the same file is a store write, not a change of
  // editor: following again would scroll a row the person can already see.
  it("does not re-follow a store write that keeps the same editor", () => {
    const h = harness();
    h.setMounted(true);
    h.setTree(checkout);
    h.setView(editor("src/main.ts"));
    h.setView(editor("src/main.ts"));
    expect(h.followed).toEqual(["src/main.ts"]);
    h.dispose();
  });

  // The watcher re-lists whenever the checkout changes, and a file appearing
  // elsewhere is not the active editor moving: snapping the list back from
  // wherever the person scrolled to is not following.
  it("does not re-follow when a re-listing replaces the tree", () => {
    const h = harness();
    h.setMounted(true);
    h.setTree(checkout);
    h.setView(editor("src/main.ts"));
    h.setTree({
      ...checkout,
      entries: [...checkout.entries, { path: "src/new.ts", kind: "File", ignored: false }],
    });
    expect(h.followed).toEqual(["src/main.ts"]);
    h.dispose();
  });
});
