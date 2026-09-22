import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { createFileExplorer, type ExplorerHandle, type ExplorerOptions } from "../src/explorer";
import {
  directoryPaths,
  treeRows,
  unloadedDirectories,
  type FileEntry,
  type FileTree,
} from "../src/tree";

// Minimal DOM seam exercises the imperative controller without a new runtime dependency.
class Node {
  children: Node[] = [];
  parent: Node | null = null;
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>();
  style = { setProperty: (_key: string, _value: string) => {} };
  className = "";
  hidden = false;
  value = "";
  textContent = "";
  readOnly = false;
  clientHeight = 96;
  scrollTop = 0;
  selectionStart = 0;
  selectionEnd = 0;
  onkeydown?: (event: KeyboardEvent) => void;
  onclick?: (event: MouseEvent) => void;
  onpointerdown?: (event: PointerEvent) => void;
  oncontextmenu?: (event: MouseEvent) => void;
  append(...nodes: Node[]) {
    for (const node of nodes) {
      node.parent = this;
      this.children.push(node);
    }
  }
  remove() {
    if (this.parent) this.parent.children = this.parent.children.filter((node) => node !== this);
  }
  setAttribute(key: string, value: string) {
    this.attributes.set(key, value);
  }
  removeAttribute(key: string) {
    this.attributes.delete(key);
  }
  contains(node: Node): boolean {
    return this === node || this.children.some((child) => child.contains(node));
  }
  closest(): Node | null {
    return this.dataset.path !== undefined ? this : (this.parent?.closest() ?? null);
  }
  focus() {
    active = { node: this };
  }
  select() {
    this.setSelectionRange(0, this.value.length);
  }
  setSelectionRange(start: number, end: number) {
    this.selectionStart = start;
    this.selectionEnd = end;
  }
  getBoundingClientRect() {
    return { left: 0, top: 0, bottom: 96 };
  }
  all(): Node[] {
    return [this, ...this.children.flatMap((child) => child.all())];
  }
  find(name: string): Node {
    return this.all().find((node) => node.className === name)!;
  }
}
let active: { node: Node } | null = null;
let frames: Map<number, FrameRequestCallback>;
let nextFrame = 0;
let handle: ExplorerHandle | undefined;
const originals = new Map<string, PropertyDescriptor | undefined>();
function global(key: string, value: unknown) {
  originals.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
  Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
}
beforeEach(() => {
  frames = new Map();
  active = null;
  global("document", { createElement: () => new Node() });
  global("getComputedStyle", () => ({ getPropertyValue: () => "24" }));
  global("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.set(++nextFrame, callback);
    return nextFrame;
  });
  global("cancelAnimationFrame", (id: number) => frames.delete(id));
  global(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
});
afterEach(() => {
  handle?.destroy();
  handle = undefined;
  for (const [key, descriptor] of originals) {
    if (descriptor) Object.defineProperty(globalThis, key, descriptor);
    else Reflect.deleteProperty(globalThis, key);
  }
  originals.clear();
});
function paint() {
  const pending = [...frames.values()];
  frames.clear();
  for (const callback of pending) callback(0);
}
function mount(options: Partial<ExplorerOptions> = {}) {
  const host = new Node();
  handle = createFileExplorer(host as unknown as HTMLElement, {
    chrome: "list",
    onOpen: () => {},
    ...options,
  });
  return {
    host,
    explorer: handle,
    field: host.find("fw-tree-edit-field"),
    list: host.find("fw-tree"),
  };
}
function key(node: Node, key: string, extra = {}) {
  const event = {
    key,
    target: node,
    preventDefault() {},
    stopPropagation() {},
    ...extra,
  } as unknown as KeyboardEvent;
  node.onkeydown?.(event);
}
const entry = (path: string, kind = "Directory", ignored = false): FileEntry => ({
  path,
  kind,
  ignored,
});
const tree = (entries: FileEntry[], loadedDirectories = [""]): FileTree => ({
  entries,
  loadedDirectories,
  truncated: false,
});

describe("inline edits", () => {
  it("retargets selection after the old row has already left the listing", () => {
    const { explorer } = mount();
    explorer.setState({ tree: tree([entry("old.ts", "File")]) });
    explorer.reveal("old.ts");
    explorer.setState({ tree: tree([entry("new.ts", "File")]) });
    expect(explorer.selected()).toBeUndefined();
    explorer.retarget("old.ts", "new.ts");
    expect(explorer.selected()?.path).toBe("new.ts");
  });
  it("retains link status and keeps a directory link available as its own mutation target", () => {
    const rows = treeRows(
      tree(
        [
          { ...entry("alias"), symlink: "Directory" },
          entry("alias/child"),
          { ...entry("external", "File"), symlink: "External" },
        ],
        ["", "alias"],
      ),
      new Set(),
    );
    expect(rows.find((row) => row.path === "alias")?.symlink).toBe("Directory");
    expect(rows.find((row) => row.path === "external")?.symlink).toBe("External");
  });
  it("observes changed child arrays behind a stable reactive listing object", () => {
    const { explorer } = mount();
    const listing = tree([entry("old.ts", "File")]);
    explorer.setState({ tree: listing });
    listing.entries = [entry("new.ts", "File")];
    explorer.setState({ tree: listing });
    explorer.reveal("new.ts");
    expect(explorer.selected()?.path).toBe("new.ts");
  });
  it("creates at the empty root, guards double Enter, and retains text and focus on failure", () => {
    const commits: string[] = [];
    const { host, explorer, field, list } = mount({
      onEditCommit: (_request, name) => commits.push(name),
    });
    explorer.setState({ tree: tree([]) });
    explorer.edit({ kind: "create", parent: "", directory: true });
    expect(list.hidden).toBe(false);
    field.value = ".agents";
    key(field, "Enter");
    key(field, "Enter");
    expect(commits).toEqual([".agents"]);
    expect(field.readOnly).toBe(true);
    explorer.setState({ tree: tree([entry("unrelated", "File")]) });
    expect(field.value).toBe(".agents");
    expect(field.readOnly).toBe(true);
    explorer.editFailed("Permission denied");
    expect(host.find("fw-tree-edit-error").textContent).toBe("Permission denied");
    expect(active?.node).toBe(field);
    expect(field.value).toBe(".agents");
    expect(field.readOnly).toBe(false);
    key(field, "Enter");
    expect(commits).toHaveLength(2);
    explorer.edit(null);
    expect(host.find("fw-tree-edit").hidden).toBe(true);
  });

  it("edits a compact directory basename and retains the draft if the source disappears", () => {
    const commits: string[] = [];
    const { explorer, field, host } = mount({
      onEditCommit: (_request, name) => commits.push(name),
    });
    explorer.setState({ tree: { entries: [entry("a/b/c/file", "File")], truncated: false } });
    explorer.reveal("a/b/c");
    expect(explorer.selected()?.label).toBe("a/b/c");
    explorer.edit({ kind: "rename", path: "a/b/c" });
    expect(field.value).toBe("c");
    field.value = "renamed";
    key(field, "Enter");
    explorer.setState({ tree: tree([]) });
    expect(host.find("fw-tree-edit").hidden).toBe(false);
    explorer.editFailed("Source disappeared");
    expect(field.value).toBe("renamed");
    expect(commits).toEqual(["renamed"]);
  });

  it("retains root creation visibility when the last real row disappears", () => {
    const { explorer, field, list } = mount();
    explorer.setState({ tree: tree([entry("last", "File")]) });
    explorer.edit({ kind: "create", parent: "", directory: false });
    field.value = "new";
    explorer.setState({ tree: tree([]) });
    expect(field.value).toBe("new");
    expect(list.hidden).toBe(false);
    key(field, "Escape");
    expect(list.hidden).toBe(true);
  });

  it("retires the old UI attempt when an explicit reveal replaces a submitted field", () => {
    let cancelled = 0;
    let commits = 0;
    const { explorer, field } = mount({
      onEditCommit: () => commits++,
      onEditCancel: () => cancelled++,
    });
    explorer.setState({ tree: tree([entry("other", "File")]) });
    explorer.edit({ kind: "create", parent: "", directory: false });
    field.value = "new";
    key(field, "Enter");
    explorer.reveal("other");
    expect(commits).toBe(1);
    expect(cancelled).toBe(1);
    explorer.edit({ kind: "create", parent: "", directory: false });
    expect(field.readOnly).toBe(false);
  });

  it("keeps an unsent blank value open for host validation and cancels only on Escape", () => {
    let commits = 0;
    const { explorer, field, host } = mount({
      onEditCommit: () => {
        commits++;
        explorer.editFailed("Enter a name.");
      },
    });
    explorer.edit({ kind: "create", parent: "", directory: false });
    key(field, "Enter");
    expect(commits).toBe(1);
    expect(host.find("fw-tree-edit").hidden).toBe(false);
    key(field, "Escape");
    expect(host.find("fw-tree-edit").hidden).toBe(true);
  });
});

describe("lazy navigation", () => {
  it("loads every revealed ancestor in order and selects a late arriving path", () => {
    const reads: string[] = [];
    const { explorer } = mount({ onExpandDirectory: (path) => reads.push(path) });
    explorer.reveal("a/b/c/file");
    explorer.setState({ tree: tree([entry("a")]) });
    expect(reads).toEqual(["", "a"]);
    explorer.setState({ tree: tree([entry("a"), entry("a/b")], ["", "a"]) });
    expect(reads).toEqual(["", "a", "a/b"]);
    explorer.setState({ tree: tree([entry("a"), entry("a/b"), entry("a/b/c")], ["", "a", "a/b"]) });
    expect(reads).toEqual(["", "a", "a/b", "a/b/c"]);
    explorer.setState({
      tree: tree(
        [entry("a"), entry("a/b"), entry("a/b/c"), entry("a/b/c/file", "File")],
        ["", "a", "a/b", "a/b/c"],
      ),
    });
    expect(explorer.selected()?.path).toBe("a/b/c/file");
    explorer.collapseAll();
    expect(explorer.selected()).toBeUndefined();
    explorer.action("first");
    expect(explorer.selected()?.path).toBe("a");
    expect(explorer.selected()?.folded).toBe(true);
  });

  it("reveals through an ancestor omitted by a partial root listing", () => {
    const reads: string[] = [];
    const { explorer } = mount({ onExpandDirectory: (path) => reads.push(path) });
    explorer.setState({ tree: { ...tree([]), truncated: true } });
    explorer.reveal("omitted/child/file");
    expect(reads).toEqual(["omitted"]);
    explorer.setState({ tree: tree([entry("omitted/child")], ["", "omitted"]) });
    expect(reads).toEqual(["omitted", "omitted/child"]);
    explorer.setState({
      tree: tree(
        [entry("omitted/child"), entry("omitted/child/file", "File")],
        ["", "omitted", "omitted/child"],
      ),
    });
    expect(explorer.selected()?.path).toBe("omitted/child/file");
  });

  it("retries a failed expansion on reopen and never rereads a loaded empty folder on state churn", () => {
    const reads: string[] = [];
    const { explorer } = mount({ onExpandDirectory: (path) => reads.push(path) });
    const listing = tree([entry("empty", "Directory", true)]);
    explorer.setState({ tree: listing });
    explorer.action("expand");
    explorer.setState({ tree: listing, loading: true });
    explorer.setState({ tree: listing, loading: false });
    expect(reads).toEqual(["empty"]);
    explorer.action("collapse");
    explorer.action("expand");
    expect(reads).toEqual(["empty", "empty"]);
    const loaded = tree(listing.entries as FileEntry[], ["", "empty"]);
    explorer.setState({ tree: loaded });
    explorer.setState({ tree: loaded, loading: true });
    expect(reads).toHaveLength(2);
  });

  it("preserves a revealed ancestor as a selectable row instead of compacting past it", () => {
    const { explorer } = mount();
    explorer.reveal("a");
    explorer.setState({
      tree: tree([entry("a"), entry("a/b"), entry("a/b/file", "File")], ["", "a", "a/b"]),
    });
    expect(explorer.selected()?.path).toBe("a");
  });

  it("keeps empty and ignored directories explicit and does not load closed directories", () => {
    const listing = tree([entry("empty"), entry(".agents", "Directory", true)]);
    expect(directoryPaths(listing).sort()).toEqual([".agents", "empty"]);
    const closed = treeRows(listing, new Set(["empty", ".agents"]));
    expect(unloadedDirectories(listing, closed, new Set())).toEqual([]);
    const open = treeRows(listing, new Set(["empty"]));
    expect(unloadedDirectories(listing, open, new Set())).toEqual([".agents"]);
    expect(open.find((row) => row.path === ".agents")?.ignored).toBe(true);
  });
});

describe("stable interaction surface", () => {
  it("filters a large listing without rebuilding unrelated paths for unchanged watches", () => {
    let excludedReads = 0;
    const interests: string[][] = [];
    const { explorer, host } = mount({
      onDirectoriesChange: (paths) => interests.push(paths),
    });
    const entries = Array.from({ length: 2000 }, (_, i): FileEntry => ({
      get path() {
        excludedReads++;
        return `other/file-${i}`;
      },
      kind: "File",
      ignored: false,
    }));
    explorer.setState({
      tree: tree([...entries, entry("src/target.ts", "File")], ["", "other", "src"]),
    });
    explorer.reveal("src/target.ts");
    explorer.setFilter("tar");
    const watched = interests.at(-1);
    const updates = interests.length;
    excludedReads = 0;
    explorer.setFilter("target");
    paint();
    expect(excludedReads).toBe(entries.length);
    expect(interests).toHaveLength(updates);
    expect(watched).toEqual(["", "src"]);
    expect(
      host
        .all()
        .filter((node) => node.className === "fw-tree-row")
        .map((node) => node.dataset.path),
    ).toEqual(["src", "src/target.ts"]);
  });

  it("refreshes filtered watch interests after folds, listings and workspace reset", () => {
    const interests: string[][] = [];
    const reads: string[] = [];
    const { explorer } = mount({
      onDirectoriesChange: (paths) => interests.push(paths),
      onExpandDirectory: (path) => reads.push(path),
    });
    const loaded = tree([entry("src"), entry("src/target.ts", "File")], ["", "src"]);
    explorer.setState({ tree: loaded });
    explorer.reveal("src/target.ts");
    explorer.setFilter("target");
    expect(interests.at(-1)).toEqual(["", "src"]);

    explorer.collapseAll();
    expect(interests.at(-1)).toEqual([""]);
    explorer.setFilter("targ");
    expect(interests.at(-1)).toEqual([""]);
    explorer.setFilter("");
    explorer.expand("src");
    explorer.setState({ tree: { ...loaded } });
    explorer.setFilter("target");
    expect(interests.at(-1)).toEqual(["", "src"]);

    reads.length = 0;
    explorer.setState({ tree: tree([entry("src"), entry("src/target.ts", "File")]) });
    expect(reads).toEqual(["src"]);
    explorer.setState({ tree: tree([]) });
    expect(interests.at(-1)).toEqual([""]);

    explorer.reset();
    explorer.setState({
      tree: tree([entry("new"), entry("new/target.ts", "File")], ["", "new"]),
    });
    explorer.reveal("new/target.ts");
    explorer.setFilter("target");
    expect(interests.at(-1)).toEqual(["", "new"]);
  });

  it("dispatches real-row pointers from the stable root and excludes the edit field", () => {
    const received: string[] = [];
    const { explorer, host, field } = mount({
      onPointerDown: (_event, row) => received.push(row.path),
    });
    explorer.setState({ tree: tree([entry("file", "File")]) });
    paint();
    const root = host.find("fw-explorer");
    expect(root.dataset.fileTreeRoot).toBe("");
    const row = host.find("fw-tree-row");
    root.onpointerdown?.({ target: row } as unknown as PointerEvent);
    explorer.edit({ kind: "rename", path: "file" });
    root.onpointerdown?.({ target: field } as unknown as PointerEvent);
    expect(received).toEqual(["file"]);
  });

  it("opens the selected-row keyboard menu once and leaves empty-root handling to the host", () => {
    const menus: string[] = [];
    let stopped = 0;
    const { explorer, list } = mount({ onContextMenu: (row) => menus.push(row.path) });
    explorer.setState({ tree: tree([entry("file", "File")]) });
    key(list, "F10", { shiftKey: true, stopPropagation: () => stopped++ });
    expect(menus).toEqual(["file"]);
    expect(stopped).toBe(1);
    explorer.setState({ tree: tree([]) });
    key(list, "ContextMenu", { stopPropagation: () => stopped++ });
    expect(stopped).toBe(1);
  });

  it("virtualizes a large directory and keeps the input node stable across repaints", () => {
    const { explorer, host, field } = mount();
    const entries = Array.from({ length: 2000 }, (_, i) => entry(`file-${i}`, "File"));
    explorer.setState({ tree: tree(entries) });
    paint();
    expect(host.all().filter((node) => node.className === "fw-tree-row").length).toBeLessThan(25);
    explorer.edit({ kind: "rename", path: "file-0" });
    field.value = "my draft";
    explorer.setState({ tree: tree([...entries, entry("new", "File")]) });
    paint();
    expect(host.find("fw-tree-edit-field")).toBe(field);
    expect(field.value).toBe("my draft");
  });
});
