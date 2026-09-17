import {
  collapseTarget,
  directoryPaths,
  expandTarget,
  filterTree,
  foldUnseen,
  treeRows,
  unloadedDirectories,
  watchDirectories,
  type FileTree,
  type TreeRow,
} from "./tree.js";

export type FileDecoration = {
  label: string;
  tone?: "added" | "modified" | "deleted" | "untracked" | "conflict";
};
/**
 * The mark a row wears, resolved by the host.
 *
 * A URL and not a glyph: a language mark is drawn in its own colours, and the
 * artwork belongs to whoever ships it. The package only places it.
 */
export type RowIcon = {
  url: string;
  /**
   * Draw the artwork as a `mask-image` over `currentColor` instead of as a
   * background: a structural mark — a folder — is chrome and takes the row's
   * colour, where a language mark's own hue is the information.
   */
  tint?: boolean;
};
/**
 * How much of the surface the package draws.
 *
 * `"list"` is for a host with a component kit of its own: the package keeps
 * the virtualised tree and nothing else, and the header, filter box, message
 * and footer become the host's to render from `onDerived` and drive through
 * `setFilter` / `onFilterFocus`.
 */
export type ExplorerChrome = "full" | "list";
/** What a host-drawn chrome needs to know about the listing it is framing. */
export type ExplorerDerived = {
  /** Rows after folds and filter — zero is the empty state. */
  rows: number;
  /** Files among those rows; a footer count is about files, not directories. */
  files: number;
  /** A filter is narrowing the listing, so "nothing here" is not the same as empty. */
  filtered: boolean;
  /** The listing stopped at the host's scan budget. */
  truncated: boolean;
  loading: boolean;
};
/**
 * An inline edit: a row renamed in place, or a name for a path that does not
 * exist yet, typed on a blank row under `parent` (`""` is the listing root).
 */
export type EditRequest =
  | { kind: "rename"; path: string }
  | { kind: "create"; parent: string; directory: boolean };
export type ExplorerOptions = {
  label?: string;
  chrome?: ExplorerChrome;
  /** Called per painted row; the URL is read off the name, never the contents. */
  icon?: (row: TreeRow) => RowIcon | null;
  onOpen: (path: string) => void;
  onRefresh?: () => void;
  onContextMenu?: (row: TreeRow, point: { x: number; y: number }) => void;
  onDirectoriesChange?: (paths: string[]) => void;
  /** Reported on every change, so a host chrome never re-derives the listing. */
  onDerived?: (derived: ExplorerDerived) => void;
  /** Where `action("filter")` sends focus when the filter box is the host's. */
  onFilterFocus?: () => void;
  /**
   * A name was accepted in the inline field. The field stays open and focused:
   * the host writes to disk and then closes it with `edit(null)`, or leaves it
   * up with `editFailed(message)` so the name can be corrected.
   */
  onEditCommit?: (request: EditRequest, name: string) => void;
  /** The field closed itself — Escape, a click away, or a name that changed nothing. */
  onEditCancel?: (request: EditRequest) => void;
  /**
   * An opaque ignored directory was opened. The host peels it with
   * `ListDirectory` and merges the answer into the listing; the package has
   * already marked the path as opened so a later `setState` does not fold it
   * shut again.
   */
  onExpandOpaque?: (path: string) => void;
};
export type ExplorerState = {
  tree: FileTree | null;
  loading?: boolean;
  error?: string | null;
  decorations?: ReadonlyMap<string, FileDecoration>;
  /**
   * Opaque to the package; a new value repaints the rows.
   *
   * For host state the rows read but the state does not carry — an icon set
   * that follows the theme, say. Without it a resolver's answer could change
   * with nothing here to notice.
   */
  revision?: unknown;
};
export type ExplorerAction =
  | "next"
  | "previous"
  | "expand"
  | "collapse"
  | "first"
  | "last"
  | "open"
  | "filter";
export type ExplorerHandle = {
  setState: (state: ExplorerState) => void;
  /** Narrow the listing. The only way in when the filter box is the host's. */
  setFilter: (query: string) => void;
  reset: () => void;
  reveal: (path: string) => void;
  /**
   * Follow the host's active document.
   *
   * Unlike `reveal` it never drops the filter — following is not a "show me
   * this" gesture — and a path the listing does not have leaves the selection
   * alone rather than falling back to the first row.
   */
  follow: (path: string) => void;
  /** Open the inline field, or close it (`null`) once the host has written. */
  edit: (request: EditRequest | null) => void;
  /** Keep the open field and say, on the row, why the last name was refused. */
  editFailed: (message: string) => void;
  action: (action: ExplorerAction) => void;
  selected: () => TreeRow | undefined;
  destroy: () => void;
};

let nextId = 0;
const OVERSCAN = 8;
/* The draft row's stand-in path. `treeRows` joins non-empty segments, so no
   listing can produce a doubled separator and collide with it. */
const DRAFT = "//draft";

/**
 * The part of a name an editor preselects: the stem, not the extension.
 *
 * Renaming `README.md` is nearly always about `README`, and typing over the
 * suffix by accident changes what the file *is*. A dotfile is all stem — the
 * leading dot of `.gitignore` does not start an extension — and a directory
 * has no extension to protect.
 */
export function nameSelection(name: string, isFile: boolean): { start: number; end: number } {
  const dot = name.lastIndexOf(".");
  return { start: 0, end: !isFile || dot <= 0 ? name.length : dot };
}

/**
 * The name a commit should carry, or `null` when the gesture was a cancel.
 *
 * Blank and unchanged are both "never mind": asking the host to write them
 * turns an abandoned rename into a failed request the person has to dismiss.
 */
export function editedName(value: string, original: string): string | null {
  const name = value.trim();
  return name === "" || name === original ? null : name;
}

/**
 * What following the active editor should do to the tree.
 *
 * Pure because the decision is data: the listing, the selected path and the
 * fold state are all in hand, and none of it is a paint. `hold` is the case
 * that matters — a path the listing does not name is not an instruction to
 * select the first row, and a path an active filter hides is not an
 * instruction to drop the filter.
 */
export type FollowPlan = { kind: "select"; index: number } | { kind: "reopen" } | { kind: "hold" };

export function planFollow(request: {
  /** The active editor's path. */
  path: string;
  /** The rows the tree is showing, after folds and filter. */
  rows: readonly TreeRow[];
  /** The row the tree has selected, if any. */
  selectedPath: string | null;
  /** The selected row is on screen, so selecting it again need not scroll. */
  visible: boolean;
  /** A filter narrows the listing. */
  filtered: boolean;
  /** The unfiltered listing contains the path at all. */
  present: boolean;
}): FollowPlan {
  const at = request.rows.findIndex((row) => row.path === request.path);
  if (at >= 0) {
    if (request.path === request.selectedPath && request.visible) return { kind: "hold" };
    return { kind: "select", index: at };
  }
  // A filtered listing is flat: the path was filtered out, and expanding
  // folds underneath the filter would fight the person typing it.
  if (request.filtered) return { kind: "hold" };
  // Not a row: it may sit under a fold, or not be in the listing at all.
  // `present` separates the two — truncated listings, deleted files and paths
  // outside the checkout leave the selection where it is.
  if (!request.present) return { kind: "hold" };
  return { kind: "reopen" };
}

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Owns only its DOM and transient navigation; callers own files and mutations. */
export function createFileExplorer(host: HTMLElement, options: ExplorerOptions): ExplorerHandle {
  const id = `file-workbench-${++nextId}`;
  const framed = (options.chrome ?? "full") === "full";
  const root = element("section", "fw-explorer");
  // Unnamed, a `section` is not a landmark, which is what the tree alone
  // should be: the host's own frame carries the name in `"list"` chrome.
  if (framed) root.setAttribute("aria-label", options.label ?? "Files");
  const header = element("header", "fw-explorer-head");
  const title = element("span", "fw-explorer-title", options.label ?? "Files");
  const refresh = element("button", "fw-button", "↻");
  refresh.type = "button";
  refresh.title = "Refresh files";
  refresh.setAttribute("aria-label", "Refresh files");
  refresh.hidden = !options.onRefresh;
  refresh.onclick = () => options.onRefresh?.();
  header.append(title, refresh);
  const search = element("input", "fw-filter");
  search.type = "search";
  search.placeholder = "Filter files…";
  search.setAttribute("aria-label", "Filter files");
  const list = element("div", "fw-tree");
  list.tabIndex = 0;
  list.setAttribute("role", "tree");
  list.setAttribute("aria-label", options.label ?? "Files");
  const spacer = element("div", "fw-tree-space");
  spacer.setAttribute("role", "presentation");
  const items = element("div", "fw-tree-items");
  items.setAttribute("role", "presentation");
  /* One field, moved by `top` rather than re-parented into the row it edits:
     the pool rewrites row nodes on every paint, and an input that is removed
     and re-inserted loses focus and caret — which is exactly what a re-listing
     arriving mid-type would do to it. */
  const editBox = element("div", "fw-tree-edit");
  editBox.hidden = true;
  const editLine = element("div", "fw-tree-edit-line");
  const editBranch = element("span", "fw-tree-branch");
  editBranch.setAttribute("aria-hidden", "true");
  const editIcon = element("span", "fw-tree-icon");
  editIcon.setAttribute("aria-hidden", "true");
  const field = element("input", "fw-tree-edit-field");
  field.type = "text";
  field.setAttribute("autocomplete", "off");
  field.setAttribute("autocapitalize", "off");
  field.setAttribute("spellcheck", "false");
  const editError = element("p", "fw-tree-edit-error");
  editError.setAttribute("role", "alert");
  editError.hidden = true;
  editLine.append(editBranch, editIcon, field);
  editBox.append(editLine, editError);
  spacer.append(items, editBox);
  list.append(spacer);
  const message = element("p", "fw-empty");
  message.setAttribute("role", "status");
  const footer = element("footer", "fw-explorer-foot");
  root.append(...(framed ? [header, search, list, message, footer] : [list]));
  host.append(root);

  type RowNode = {
    node: HTMLDivElement;
    branch: HTMLSpanElement;
    icon: HTMLSpanElement;
    label: HTMLSpanElement;
    mark: HTMLSpanElement;
    key: string;
  };

  let state: ExplorerState = { tree: null };
  let rows: TreeRow[] = [];
  /* The filter lives here rather than on the input, because in `"list"` chrome
     the input is the host's and the package never sees it. */
  let query = "";
  let files = 0;
  let reported = "";
  let editing: EditRequest | null = null;
  let editOriginal = "";
  /* Bumped when the draft row appears or goes: a create is the one edit that
     changes the listing, and `built` has to notice. A rename does not. */
  let drafts = 0;
  let collapsed = new Set<string>();
  let seen = new Set<string>();
  // Expansion intent survives a root listing, which contains only ignored placeholders.
  let openedIgnored = new Set<string>();
  const pendingDirectories = new Set<string>();
  let selectedPath: string | null = null;
  let index = 0;
  let rowHeight = 24;
  let viewport = 0;
  let frame = 0;
  let destroyed = false;
  let directories = "";
  /* What the current `rows` were derived from. A listing of 50 000 paths is a
     tree build and 50 000 allocations; a loading flag going up and down, or a
     decoration arriving, changes none of it. */
  let revision = 0;
  let folds = 0;
  let built = "";
  /* One node per visible slot, reused across paints. Rebuilding the window
     from scratch on every store touch — a loading flag, a re-listing that
     changed nothing, a keystroke in the filter — drops and recreates the row
     under the pointer and reloads every icon, which is the flicker. */
  const pool: RowNode[] = [];
  /* A resolver that answers for some rows and not others still reserves the
     gutter on all of them: a directory whose mark collapsed would pull its
     name left of the files at the same depth. A host with no resolver at all
     gets no gutter. */
  const gutter = typeof options.icon === "function";

  function createRowNode(): RowNode {
    const node = element("div", "fw-tree-row");
    node.setAttribute("role", "treeitem");
    const branch = element("span", "fw-tree-branch");
    branch.setAttribute("aria-hidden", "true");
    const icon = element("span", "fw-tree-icon");
    icon.setAttribute("aria-hidden", "true");
    const label = element("span", "fw-tree-label");
    const mark = element("span", "fw-tree-mark");
    node.append(branch, icon, label, mark);
    return { node, branch, icon, label, mark, key: "" };
  }

  function fill(
    entry: RowNode,
    at: number,
    row: TreeRow,
    icon: RowIcon | null,
    decoration: FileDecoration | undefined,
  ): void {
    const { node } = entry;
    node.id = `${id}-${at}`;
    node.dataset.index = String(at);
    node.dataset.path = row.path;
    node.dataset.ignored = String(row.ignored);
    node.setAttribute("aria-level", String(row.depth + 1));
    node.setAttribute("aria-selected", String(at === index));
    if (row.isFile || row.opaque) node.removeAttribute("aria-expanded");
    else node.setAttribute("aria-expanded", String(!row.folded));
    node.style.paddingInlineStart = `${row.depth * 2 + 1}ch`;
    node.title = row.opaque ? `${row.path} — ignored; contents not listed` : row.path;
    entry.branch.textContent = row.isFile ? "·" : row.opaque ? "─" : row.folded ? "▸" : "▾";
    entry.icon.hidden = !gutter;
    entry.icon.style.setProperty("--fw-icon-url", icon ? `url("${icon.url}")` : "none");
    if (icon?.tint) entry.icon.dataset.tint = "true";
    else delete entry.icon.dataset.tint;
    entry.label.textContent = row.label + (row.isFile ? "" : "/");
    if (decoration) {
      if (decoration.tone) node.dataset.tone = decoration.tone;
      else delete node.dataset.tone;
      entry.mark.textContent = decoration.label;
      entry.mark.hidden = false;
    } else {
      delete node.dataset.tone;
      entry.mark.textContent = "";
      entry.mark.hidden = true;
    }
  }

  function schedule(): void {
    if (frame || destroyed) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      paint();
    });
  }

  function measure(): void {
    rowHeight = Math.max(
      16,
      Number.parseFloat(getComputedStyle(root).getPropertyValue("--fw-row-height")) || 24,
    );
    viewport = list.clientHeight;
    // The field is placed in row units too, so a changed row height moves it.
    placeEdit();
    schedule();
  }

  function paint(): void {
    const start = Math.max(0, Math.floor(list.scrollTop / rowHeight) - OVERSCAN);
    const end = Math.min(rows.length, start + Math.ceil(viewport / rowHeight) + 2 * OVERSCAN);
    spacer.style.height = `${rows.length * rowHeight}px`;
    items.style.transform = `translateY(${start * rowHeight}px)`;
    const count = Math.max(0, end - start);
    while (pool.length > count) pool.pop()?.node.remove();
    for (let slot = 0; slot < count; slot++) {
      const at = start + slot;
      const row = rows[at];
      const icon = options.icon?.(row) ?? null;
      const decoration = state.decorations?.get(row.path);
      // Everything the row paints, so an unchanged row is left alone and a
      // changed one is rewritten in place rather than replaced.
      const key = [
        at,
        at === index,
        row.path,
        row.label,
        row.depth,
        row.isFile,
        row.folded,
        row.opaque,
        row.ignored,
        icon?.url ?? "",
        icon?.tint === true,
        decoration?.label ?? "",
        decoration?.tone ?? "",
      ].join("\u0000");
      let entry = pool[slot];
      if (!entry) {
        entry = createRowNode();
        pool[slot] = entry;
        items.append(entry.node);
      } else if (entry.key === key) {
        continue;
      }
      entry.key = key;
      fill(entry, at, row, icon, decoration);
    }
    if (index >= start && index < end) list.setAttribute("aria-activedescendant", `${id}-${index}`);
    else list.removeAttribute("aria-activedescendant");
  }

  /** Where the field sits, or `-1` once the row it was opened on is gone. */
  function editIndex(): number {
    if (!editing) return -1;
    const path = editing.kind === "rename" ? editing.path : DRAFT;
    return rows.findIndex((row) => row.path === path);
  }

  /**
   * Follow the edited row.
   *
   * Position only — never the value: a re-listing that arrives while someone
   * is typing moves the field and leaves the half-typed name and the caret
   * exactly where they were.
   */
  function placeEdit(): void {
    if (!editing) return;
    const at = editIndex();
    if (at < 0) {
      closeEdit(true);
      return;
    }
    const row = rows[at];
    editBox.hidden = false;
    editBox.style.top = `${at * rowHeight}px`;
    editBox.style.paddingInlineStart = `${row.depth * 2 + 1}ch`;
    editBranch.textContent = row.isFile ? "·" : "▾";
    const icon = options.icon?.(row) ?? null;
    editIcon.hidden = !gutter;
    editIcon.style.setProperty("--fw-icon-url", icon ? `url("${icon.url}")` : "none");
    if (icon?.tint) editIcon.dataset.tint = "true";
    else delete editIcon.dataset.tint;
  }

  /** Never calls `rebuild`: `placeEdit` calls this, and the two would recurse. */
  function closeEdit(notify: boolean): void {
    const request = editing;
    if (!request) return;
    editing = null;
    editBox.hidden = true;
    editError.hidden = true;
    editError.textContent = "";
    field.removeAttribute("aria-invalid");
    if (request.kind === "create") drafts += 1;
    if (notify) options.onEditCancel?.(request);
  }

  function abandonEdit(): void {
    closeEdit(true);
    rebuild();
    list.focus({ preventScroll: true });
  }

  function commitEdit(): void {
    const request = editing;
    if (!request) return;
    const name = editedName(field.value, editOriginal);
    if (name === null) {
      abandonEdit();
      return;
    }
    options.onEditCommit?.(request, name);
  }

  function edit(request: EditRequest | null): void {
    if (!request) {
      closeEdit(false);
      rebuild();
      return;
    }
    const previous = editing;
    editing = request;
    if (previous?.kind === "create" || request.kind === "create") drafts += 1;
    if (request.kind === "create" && collapsed.delete(request.parent)) folds += 1;
    editError.hidden = true;
    editError.textContent = "";
    field.removeAttribute("aria-invalid");
    if (request.kind === "rename") {
      const row = rows.find((item) => item.path === request.path);
      editOriginal = row?.label ?? request.path.slice(request.path.lastIndexOf("/") + 1);
      selectedPath = request.path;
    } else {
      editOriginal = "";
      selectedPath = DRAFT;
    }
    field.value = editOriginal;
    rebuild();
    const at = editIndex();
    if (at < 0) return;
    select(at);
    field.focus({ preventScroll: true });
    const span = nameSelection(editOriginal, rows[at].isFile);
    field.setSelectionRange(span.start, span.end);
  }

  /** The blank row a create is typed on, slotted in as the parent's first child. */
  function withDraft(base: TreeRow[], parent: string, directory: boolean): TreeRow[] {
    const found = parent === "" ? -1 : base.findIndex((row) => row.path === parent);
    if (parent !== "" && found < 0) return base;
    const at = found + 1;
    const row: TreeRow = {
      depth: found < 0 ? 0 : base[found].depth + 1,
      label: "",
      path: DRAFT,
      isFile: !directory,
      folded: true,
      ignored: false,
      opaque: false,
    };
    return [...base.slice(0, at), row, ...base.slice(at)];
  }

  function rebuild(): void {
    const filtered = query.trim() !== "";
    const signature = `${revision}\u0000${folds}\u0000${drafts}\u0000${query}`;
    if (signature !== built) {
      built = signature;
      rows = treeRows(
        filterTree(state.tree, query),
        filtered ? new Set() : collapsed,
        openedIgnored,
      );
      files = 0;
      for (const row of rows) if (row.isFile) files += 1;
      // A filter reveals matches, not additional directory interests.
      const watchedRows = filtered ? treeRows(state.tree, collapsed, openedIgnored) : rows;
      const paths = watchDirectories(watchedRows);
      const key = JSON.stringify(paths);
      if (key !== directories) {
        directories = key;
        options.onDirectoriesChange?.(paths);
      }
      for (const path of unloadedDirectories(state.tree, watchedRows, openedIgnored)) {
        if (pendingDirectories.has(path)) continue;
        pendingDirectories.add(path);
        options.onExpandOpaque?.(path);
      }
      // After the counts and the watch interest: a name nobody has typed yet
      // is not a file, and an unwritten row is nothing to watch.
      if (editing?.kind === "create") rows = withDraft(rows, editing.parent, editing.directory);
    }
    if (selectedPath === null) {
      // No selection yet: the first row stands in, so the keyboard has a
      // start.
      index = Math.max(0, Math.min(rows.length - 1, index));
      selectedPath = rows[index]?.path ?? null;
    } else {
      // A path the listing does not have keeps `-1` and the path itself: it
      // must not silently become row 0, and it may come back.
      index = rows.findIndex((row) => row.path === selectedPath);
    }
    spacer.style.height = `${rows.length * rowHeight}px`;
    list.scrollTop = Math.min(list.scrollTop, Math.max(0, rows.length * rowHeight - viewport));
    list.hidden = rows.length === 0;
    const truncated = state.tree?.truncated ?? false;
    const loading = state.loading ?? false;
    if (framed) {
      footer.textContent = `${files} files shown${truncated ? " · partial listing" : ""}`;
      message.textContent =
        state.error ??
        (loading && !state.tree
          ? "Reading files…"
          : filtered
            ? "No matching files."
            : "No files to show.");
      message.hidden = rows.length > 0 && !state.error;
      refresh.disabled = loading;
    }
    placeEdit();
    root.setAttribute("aria-busy", String(loading));
    // Only on a change: a host chrome is a render, and `rebuild` runs on every
    // `setState` — including the ones that carry the same listing back.
    const stamp = `${rows.length}\u0000${files}\u0000${filtered}\u0000${truncated}\u0000${loading}`;
    if (stamp !== reported) {
      reported = stamp;
      options.onDerived?.({ rows: rows.length, files, filtered, truncated, loading });
    }
    schedule();
  }

  function select(at: number): void {
    index = Math.max(0, Math.min(rows.length - 1, at));
    selectedPath = rows[index]?.path ?? null;
    const top = index * rowHeight;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + rowHeight > list.scrollTop + viewport)
      list.scrollTop = top + rowHeight - viewport;
    schedule();
  }

  /** Whether the row at `at` is inside the visible range. */
  function visibleIndex(at: number): boolean {
    const top = at * rowHeight;
    return top >= list.scrollTop && top + rowHeight <= list.scrollTop + viewport;
  }

  /** Open every directory leading to `path`; reports whether a fold moved. */
  function expandAncestors(path: string): boolean {
    const parts = path.split("/");
    let moved = false;
    for (let i = 1; i <= parts.length; i++) {
      if (collapsed.delete(parts.slice(0, i).join("/"))) moved = true;
    }
    return moved;
  }

  /** Whether the unfiltered listing names the path at all. */
  function treeHasPath(path: string): boolean {
    return state.tree?.entries.some((entry) => entry.path === path) ?? false;
  }

  function activate(): void {
    const row = rows[index];
    if (!row || row.path === DRAFT) return;
    if (row.opaque || (!row.isFile && row.ignored && row.folded)) {
      peelOpaque(row.path);
      return;
    }
    if (row.isFile) options.onOpen(row.path);
    else {
      row.folded ? collapsed.delete(row.path) : collapsed.add(row.path);
      folds += 1;
      rebuild();
    }
  }

  /** Ask the host for one level under an opaque ignored directory. */
  function peelOpaque(path: string): void {
    openedIgnored.add(path);
    pendingDirectories.add(path);
    seen.add(path);
    collapsed.delete(path);
    folds += 1;
    options.onExpandOpaque?.(path);
    rebuild();
  }

  function action(command: ExplorerAction): void {
    if (command === "filter") {
      if (!framed) options.onFilterFocus?.();
      else {
        search.focus();
        search.select();
      }
      return;
    }
    if (command === "next") select(index + 1);
    else if (command === "previous") select(index - 1);
    else if (command === "first") select(0);
    else if (command === "last") select(rows.length - 1);
    else if (command === "open") activate();
    else {
      const row = rows[index];
      if (command === "expand" && row && (row.opaque || (row.ignored && row.folded))) {
        peelOpaque(row.path);
        return;
      }
      const target =
        command === "collapse"
          ? collapseTarget(rows[index], collapsed)
          : expandTarget(rows[index], collapsed, rows, index);
      if (!target) return;
      if ("select" in target) select(rows.findIndex((row) => row.path === target.select));
      else {
        if ("fold" in target) collapsed.add(target.fold);
        if ("unfold" in target) collapsed.delete(target.unfold);
        folds += 1;
        rebuild();
      }
    }
  }

  /* By path and not by the painted `data-index`: closing the field drops the
     draft row and shifts every index below it, and the paint that would fix
     the attributes is a frame away when the click lands. */
  function rowFrom(event: MouseEvent): number | null {
    const node = (event.target as Element).closest<HTMLElement>("[data-path]");
    const path = node?.dataset.path;
    if (path === undefined) return null;
    const at = rows.findIndex((row) => row.path === path);
    return at < 0 ? null : at;
  }
  list.onclick = (event) => {
    if (editBox.contains(event.target as Node)) return;
    const at = rowFrom(event);
    if (at === null) return;
    list.focus({ preventScroll: true });
    select(at);
    activate();
  };
  list.oncontextmenu = (event) => {
    if (editBox.contains(event.target as Node)) return;
    const at = rowFrom(event);
    if (at === null || !options.onContextMenu) return;
    event.preventDefault();
    select(at);
    options.onContextMenu(rows[at], { x: event.clientX, y: event.clientY });
  };
  field.oninput = () => {
    if (editError.hidden) return;
    editError.hidden = true;
    editError.textContent = "";
    field.removeAttribute("aria-invalid");
  };
  field.onkeydown = (event) => {
    if (event.isComposing) return;
    if (event.key !== "Enter" && event.key !== "Escape") return;
    event.preventDefault();
    // The tree's own keymap and the host's chords both sit above this input;
    // Enter would open a file and Escape would close whatever is behind it.
    event.stopPropagation();
    if (event.key === "Enter") commitEdit();
    else abandonEdit();
  };
  field.onblur = (event) => {
    if (!editing) return;
    /* Focus that went *nowhere* is not the person leaving: a menu unmounting
       as the field opens, or the window losing focus, both land on nothing,
       and a rename abandoned by switching apps is not a rename abandoned.
       Cancel rather than commit, so a half-typed name is never applied by
       clicking somewhere; Enter is the only thing that writes. */
    if (event.relatedTarget === null) return;
    closeEdit(true);
    rebuild();
  };
  list.onkeydown = (event) => {
    if (event.target === field) return;
    if (event.isComposing || event.altKey || event.metaKey || event.ctrlKey) return;
    const commands: Record<string, ExplorerAction> = {
      ArrowDown: "next",
      ArrowUp: "previous",
      ArrowLeft: "collapse",
      ArrowRight: "expand",
      Home: "first",
      End: "last",
      Enter: "open",
    };
    const command = commands[event.key];
    if (command) {
      event.preventDefault();
      action(command);
    }
  };
  list.onscroll = schedule;
  function filter(next: string): void {
    if (next === query) return;
    query = next;
    if (search.value !== next) search.value = next;
    list.scrollTop = 0;
    rebuild();
  }
  search.oninput = () => filter(search.value);
  search.onkeydown = (event) => {
    if (event.isComposing) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      list.focus();
      select(0);
    }
    if (event.key === "Escape" && search.value) {
      event.preventDefault();
      filter("");
    }
  };
  const observer = new ResizeObserver(measure);
  observer.observe(list);
  measure();
  rebuild();

  return {
    setState(next) {
      const changed = next.tree !== state.tree;
      state = next;
      if (changed) {
        if (!state.tree?.loadedDirectories) pendingDirectories.clear();
        for (const path of state.tree?.loadedDirectories ?? []) pendingDirectories.delete(path);
        revision += 1;
        const folded = foldUnseen(collapsed, seen, directoryPaths(state.tree));
        if (folded.collapsed !== collapsed) folds += 1;
        collapsed = folded.collapsed;
        seen = folded.seen;
      }
      rebuild();
    },
    setFilter: filter,
    edit,
    editFailed(message) {
      if (!editing) return;
      editError.textContent = message;
      editError.hidden = false;
      field.setAttribute("aria-invalid", "true");
      field.focus({ preventScroll: true });
    },
    reset() {
      closeEdit(false);
      collapsed.clear();
      seen.clear();
      openedIgnored.clear();
      pendingDirectories.clear();
      folds += 1;
      selectedPath = null;
      query = "";
      search.value = "";
      list.scrollTop = 0;
      state = { tree: null };
      revision += 1;
      rebuild();
    },
    reveal(path) {
      closeEdit(false);
      query = "";
      search.value = "";
      expandAncestors(path);
      folds += 1;
      selectedPath = path;
      rebuild();
      // A path the listing does not have stays unselected rather than
      // selecting row 0; `rebuild` has already kept the wanted path.
      if (rows[index]?.path === path) select(index);
    },
    follow(path) {
      // A name being typed is not interrupted by an unrelated tab change; the
      // next follow catches up.
      if (editing) return;
      const plan = planFollow({
        path,
        rows,
        selectedPath,
        visible: index >= 0 && visibleIndex(index),
        filtered: query.trim() !== "",
        present: treeHasPath(path),
      });
      if (plan.kind === "hold") return;
      if (plan.kind === "select") {
        select(plan.index);
        return;
      }
      // Hidden under a fold: open the way to it like `reveal`, but only ever
      // open — someone comparing three folders keeps all three.
      if (expandAncestors(path)) folds += 1;
      selectedPath = path;
      rebuild();
      if (rows[index]?.path === path) select(index);
    },
    action,
    // Never the draft: a row with no path yet is nothing a host can act on.
    selected: () => (rows[index]?.path === DRAFT ? undefined : rows[index]),
    destroy() {
      destroyed = true;
      closeEdit(false);
      pool.length = 0;
      cancelAnimationFrame(frame);
      observer.disconnect();
      root.remove();
    },
  };
}
