import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  on,
  onCleanup,
  onMount,
} from "solid-js";
import { FILES } from "../actions/actions";
import { enterContext, registerAction } from "../actions/dispatch";
import { clearTreeReveal, openEditor, treeReveal } from "../store/viewsStore";
import { forgeStore } from "../store/forgeStore";
import { workbenchStore } from "../store/workbenchStore";
import {
  beginWorkbenchRequest,
  createPath,
  deletePath,
  failWorkbenchRequest,
  loadFileTree,
  openFile,
  renamePath,
  warmFileTree,
} from "../workbench/api";
import { ensureDiff } from "../workbench/decorations";
import { requestConfirm, requestTextInput } from "../store/runtimeStore";
import {
  collapseTarget,
  directoryPaths,
  expandTarget,
  filterTree,
  foldUnseen,
  treeRows,
} from "../workbench/filetree";
import { fileDecorations, folderCounts } from "../workbench/treeDecorations";
import { Icon, LangIcon } from "../theme/icons";
import {
  ContextMenu,
  EmptyState,
  FilterHeader,
  IconButton,
  Skeleton,
  Tooltip,
  type MenuItem,
} from "../ui";

const ROW_OVERSCAN = 8;

/**
 * A DOM id for a row, so `aria-activedescendant` can name it.
 *
 * The path goes through `encodeURIComponent` because it is arbitrary text and
 * an id has to survive being written into an attribute and read back.
 */
function rowId(path: string): string {
  return `file-tree-${encodeURIComponent(path)}`;
}

/**
 * The file browser (ADR-012).
 *
 * The GUI never touches a workspace with `std::fs`: every path here came from
 * the daemon's `ListFiles`, and opening one is a `ReadFile`.
 */
export function FileTreePanel() {
  let list!: HTMLDivElement;
  const [collapsed, setCollapsed] = createSignal<Set<string>>(new Set());
  /**
   * Directories this panel has already decided about.
   *
   * Kept beside `collapsed` so a re-read of the same checkout — after a
   * refresh, or after a write — does not shut the folders the person opened
   * while still folding whatever is genuinely new (see `foldUnseen`).
   */
  const [seenDirs, setSeenDirs] = createSignal<Set<string>>(new Set());
  const [selected, setSelected] = createSignal<string | null>(null);
  const [scrollTop, setScrollTop] = createSignal(0);
  /**
   * The list's own height, measured rather than assumed.
   *
   * Read straight off the element it was a plain property read, so a panel
   * that grew after the first paint — which is every panel, since the tree
   * arrives from the daemon after layout — kept the window it was sized for
   * and left the bottom of the list blank.
   */
  const [viewport, setViewport] = createSignal(0);
  /** U8: the in-panel filter. Narrows the listing; it does not reorder it. */
  const [query, setQuery] = createSignal("");
  const [menu, setMenu] = createSignal<{ x: number; y: number; path: string } | null>(null);
  let filterInput: HTMLInputElement | undefined;

  /*
   * A tree arrives closed.
   *
   * `focusWorkspace` clears the tree when the checkout changes, so the null in
   * between is what resets both sets: a fold set kept across workspaces would
   * be describing directories that are not there.
   *
   * `on` rather than a bare effect, because the tree must be the *only* thing
   * tracked and this body writes two signals it also reads. Reading them
   * tracked made the effect wake itself — every write is a new `Set`, equal
   * contents but new identity — until the stack ran out; hand-rolling that as
   * a read plus `untrack` then went the other way and registered no dependency
   * at all after the first run, so a second listing arrived unfolded. `on`
   * states the dependency once and runs the body untracked, and the identity
   * check below keeps one arrival to one update.
   */
  createEffect(
    on(
      () => workbenchStore.tree,
      (tree) => {
        if (!tree) {
          if (collapsed().size > 0) setCollapsed(new Set<string>());
          if (seenDirs().size > 0) setSeenDirs(new Set<string>());
          return;
        }
        const next = foldUnseen(collapsed(), seenDirs(), directoryPaths(tree));
        if (next.seen === seenDirs()) return;
        setCollapsed(next.collapsed);
        setSeenDirs(next.seen);
      },
    ),
  );

  /**
   * The listing, narrowed and folded.
   *
   * A filter opens everything it matched: a match hidden inside a folded
   * directory is a filter that found nothing as far as the person can tell.
   */
  const visible = createMemo(() => filterTree(workbenchStore.tree, query()));
  const rows = createMemo(() =>
    treeRows(visible(), query().trim() === "" ? collapsed() : new Set<string>()),
  );

  /** U8: what git says about each path, from the Diff tab's own answer. */
  const marks = createMemo(() => fileDecorations(workbenchStore.diff?.files ?? []));
  const counts = createMemo(() => folderCounts(marks().keys()));
  const index = createMemo(() => {
    const path = selected();
    const found = rows().findIndex((row) => row.path === path);
    return found < 0 ? 0 : found;
  });
  const windowed = createMemo(() => {
    const allRows = rows();
    const height = rowHeight();
    const first = Math.floor(scrollTop() / height);
    const start = Math.max(0, Math.min(first - ROW_OVERSCAN, Math.max(allRows.length - 1, 0)));
    const seen = viewport() || (typeof window === "undefined" ? 0 : window.innerHeight);
    const viewportRows = Math.ceil(seen / height);
    const count = Math.max(viewportRows + ROW_OVERSCAN * 2, ROW_OVERSCAN * 2 + 4);
    const end = Math.min(allRows.length, start + count);
    return { items: allRows.slice(start, end), start, end, total: allRows.length };
  });

  let sizeObserver: ResizeObserver | undefined;

  /** Follow the list's height for as long as it is on screen. */
  function watchSize(element: HTMLDivElement): void {
    list = element;
    setViewport(element.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    sizeObserver?.disconnect();
    sizeObserver = new ResizeObserver(() => setViewport(element.clientHeight));
    sizeObserver.observe(element);
  }

  onCleanup(() => sizeObserver?.disconnect());

  function rowHeight(): number {
    if (typeof document === "undefined") return 1;
    const value = Number.parseFloat(
      getComputedStyle(document.documentElement).getPropertyValue("--row-h"),
    );
    return Number.isFinite(value) && value > 0 ? value : 1;
  }

  function select(path: string): void {
    setSelected(path);
    queueMicrotask(() => {
      if (!list) return;
      const selectedIndex = rows().findIndex((row) => row.path === path);
      if (selectedIndex < 0) return;
      const height = rowHeight();
      const top = selectedIndex * height;
      const bottom = top + height;
      if (top < list.scrollTop) list.scrollTo({ top });
      else if (bottom > list.scrollTop + list.clientHeight) {
        list.scrollTo({ top: bottom - list.clientHeight });
      }
      setScrollTop(list.scrollTop);
    });
  }

  function step(delta: number): void {
    const allRows = rows();
    if (allRows.length === 0) return;
    const next = Math.min(Math.max(index() + delta, 0), allRows.length - 1);
    select(allRows[next].path);
  }

  function fold(path: string): void {
    setCollapsed(new Set(collapsed()).add(path));
  }

  function unfold(path: string): void {
    const next = new Set(collapsed());
    next.delete(path);
    setCollapsed(next);
  }

  function open(path: string): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    beginWorkbenchRequest("file");
    openEditor(path);
    void openFile(workspace, path).catch((error) => failWorkbenchRequest("file", error));
  }

  function activate(): void {
    const row = rows()[index()];
    if (!row) return;
    if (!row.isFile) {
      if (row.folded) unfold(row.path);
      else fold(row.path);
      return;
    }
    open(row.path);
  }

  /*
   * A new checkout selects nothing.
   *
   * It no longer clears the fold set: `focusWorkspace` nulls the tree on the
   * way to the new one, and the effect above resets both sets on that null and
   * folds again when the listing lands. Clearing it here as well is what made
   * a panel mounted with a tree already in the store open every folder — the
   * two effects both ran once on mount, in declaration order, and this one had
   * the last word.
   */
  let previousWorkspace: string | null | undefined;
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    if (workspace === previousWorkspace) return;
    previousWorkspace = workspace;
    setSelected(null);
  });

  createEffect(() => {
    const workspace = workbenchStore.workspace;
    if (workspace) warmFileTree(workspace);
    // The decorations this panel paints are read from `workbenchStore.diff`,
    // which until now only the Git tab ever filled.
    ensureDiff();
  });

  /**
   * Re-read the checkout on demand.
   *
   * The listing is a read and not a subscription (see `core.rs`), so the only
   * things that refresh it are this panel's own create, rename and delete. An
   * agent that writes a file in its terminal, a `git checkout`, a `rm` — none
   * of them reach the tree, and the panel would go on painting the listing it
   * was handed when the checkout was first opened. The effect above cannot do
   * this job: it is guarded on the tree being absent, which is exactly what a
   * stale tree is not.
   */
  function refresh(): void {
    const workspace = workbenchStore.workspace;
    if (!workspace || workbenchStore.loading.tree) return;
    beginWorkbenchRequest("tree");
    void loadFileTree(workspace).catch((error) => failWorkbenchRequest("tree", error));
  }

  /**
   * A7: someone asked for a path to be shown here.
   *
   * Every ancestor is unfolded before the row is selected, because a row
   * inside a collapsed folder is not in `rows()` and selecting it would scroll
   * to nothing. The request is cleared once acted on so re-mounting the panel
   * does not replay it.
   */
  createEffect(() => {
    const path = treeReveal();
    if (!path || !workbenchStore.tree) return;
    const ancestors = new Set(collapsed());
    const parts = path.split("/").filter(Boolean);
    for (let depth = 1; depth <= parts.length; depth += 1) {
      ancestors.delete(parts.slice(0, depth).join("/"));
    }
    setCollapsed(ancestors);
    select(path);
    clearTreeReveal();
  });

  /**
   * The bare-letter chords (`j`, `k`, `h`, `l`) exist while this panel holds
   * the keyboard and nowhere else: bound app-wide they would take four letters
   * away from every terminal on screen.
   *
   * Entered on *focus* and not on mount, which is how the terminal does it.
   * Held from mount, the panel took `j`/`k`/`h`/`l` and `Enter` away from the
   * editor and from any focused control for as long as the sidebar happened
   * to be on the Files view — the tree does not have the keyboard just because
   * it is visible.
   */
  let leaveFiles: (() => void) | undefined;

  function claimKeyboard(): void {
    leaveFiles ??= enterContext(FILES);
  }

  function releaseKeyboard(): void {
    leaveFiles?.();
    leaveFiles = undefined;
  }

  onCleanup(releaseKeyboard);

  onMount(() => {
    const bound = [
      registerAction("file_tree_next", () => step(1)),
      registerAction("file_tree_previous", () => step(-1)),
      registerAction("file_tree_collapse", () => {
        const target = collapseTarget(rows()[index()], collapsed());
        if (!target) return;
        if ("fold" in target) fold(target.fold);
        else select(target.select);
      }),
      registerAction("file_tree_expand", () => {
        const target = expandTarget(rows()[index()], collapsed(), rows(), index());
        if (!target) return;
        if ("unfold" in target) unfold(target.unfold);
        else select(target.select);
      }),
      registerAction("file_tree_open", activate),
      registerAction("file_tree_refresh", refresh),
      registerAction("file_tree_filter", () => filterInput?.focus({ preventScroll: true })),
      registerAction("file_tree_first", () => {
        const first = rows()[0];
        if (first) select(first.path);
      }),
      registerAction("file_tree_last", () => {
        const last = rows().at(-1);
        if (last) select(last.path);
      }),
      // A11's two chords, on the selected row. `F2` and `⌫` are what every
      // file browser has taught, and both are scoped to the tree's context.
      registerAction("file_tree_rename", () => {
        const row = rows()[index()];
        if (row) rename(row.path);
      }),
      registerAction("file_tree_delete", () => {
        const row = rows()[index()];
        if (row) remove(row.path, row.isFile);
      }),
    ];
    onCleanup(() => {
      for (const unbind of bound) unbind();
    });
  });

  /**
   * The directory a new path is created in.
   *
   * A file's parent, a directory itself, and the checkout root when nothing is
   * selected — which is what "new file" means with no context, and is why this
   * returns a prefix rather than requiring a selection.
   */
  function parentOf(path: string, isFile: boolean): string {
    if (!isFile) return path;
    const cut = path.lastIndexOf("/");
    return cut < 0 ? "" : path.slice(0, cut);
  }

  function withWorkspace(run: (workspace: string) => Promise<unknown>): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    void run(workspace).catch((error) => failWorkbenchRequest("tree", error));
  }

  /** A11: create, under the row the menu was opened on. */
  function create(path: string, isFile: boolean, directory: boolean): void {
    const parent = parentOf(path, isFile);
    requestTextInput({
      title: directory ? "New folder" : "New file",
      label: parent === "" ? "Name" : `Name, inside ${parent}`,
      value: "",
      confirmLabel: "Create",
      allowEmpty: false,
      onSubmit: (name) =>
        withWorkspace((workspace) =>
          createPath(workspace, parent === "" ? name : `${parent}/${name}`, directory),
        ),
    });
  }

  /** A11: rename in place. The name is pre-filled; the directory does not move. */
  function rename(path: string): void {
    const cut = path.lastIndexOf("/");
    const parent = cut < 0 ? "" : path.slice(0, cut);
    requestTextInput({
      title: "Rename",
      label: "New name",
      value: path.slice(cut + 1),
      confirmLabel: "Rename",
      allowEmpty: false,
      onSubmit: (name) =>
        withWorkspace((workspace) =>
          renamePath(workspace, path, parent === "" ? name : `${parent}/${name}`),
        ),
    });
  }

  /** A11: delete, after asking. There is no trash — git is the undo. */
  function remove(path: string, isFile: boolean): void {
    requestConfirm({
      title: `Delete ${path}?`,
      description: isFile
        ? "The file is removed from the checkout. Git is the only way back."
        : "The folder and everything in it are removed from the checkout. Git is the only way back.",
      confirmLabel: "Delete",
      destructive: true,
      onConfirm: () => withWorkspace((workspace) => deletePath(workspace, path)),
    });
  }

  /** U8's context menu, and A11's mutations. */
  function menuItems(path: string, isFile: boolean): MenuItem[] {
    const absolute = () => {
      const root = forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace)?.path;
      return root ? `${root.replace(/\/$/, "")}/${path}` : path;
    };
    return [
      ...(isFile
        ? [{ kind: "item" as const, label: "Open", icon: "file" as const, run: () => open(path) }]
        : []),
      {
        kind: "item",
        label: "Copy path",
        icon: "copy",
        run: () => void navigator.clipboard?.writeText(absolute()).catch(() => undefined),
      },
      {
        kind: "item",
        label: "Copy relative path",
        icon: "copy",
        run: () => void navigator.clipboard?.writeText(path).catch(() => undefined),
      },
      { kind: "rule" },
      {
        kind: "item",
        label: "New file…",
        icon: "file",
        run: () => create(path, isFile, false),
      },
      {
        kind: "item",
        label: "New folder…",
        icon: "folder",
        run: () => create(path, isFile, true),
      },
      { kind: "item", label: "Rename…", icon: "edit", run: () => rename(path) },
      {
        kind: "item",
        label: "Delete",
        icon: "trash",
        destructive: true,
        run: () => remove(path, isFile),
      },
    ];
  }

  return (
    // `focusin`/`focusout` rather than `focus`/`blur`: focus moving from the
    // panel to a row inside it must not read as leaving.
    <div
      class="panel-body"
      tabIndex={0}
      onFocusIn={claimKeyboard}
      onFocusOut={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) releaseKeyboard();
      }}
    >
      <FilterHeader
        label="Filter files"
        placeholder="Filter files…"
        query={query()}
        onQuery={setQuery}
        ref={(element) => (filterInput = element)}
      >
        <Tooltip label="Re-read the checkout" contents>
          <IconButton
            label="Re-read the checkout"
            disabled={workbenchStore.loading.tree || workbenchStore.workspace === null}
            onClick={refresh}
          >
            <Icon name="refresh" class="forge-icon-muted" size={13} />
          </IconButton>
        </Tooltip>
      </FilterHeader>
      <Show when={workbenchStore.treeError}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      <Show
        when={workbenchStore.tree}
        fallback={
          <Show
            when={!workbenchStore.loading.tree}
            fallback={<Skeleton label="Reading the checkout" rows={8} />}
          >
            <EmptyState
              message={
                workbenchStore.workspace === null
                  ? "No checkout selected."
                  : "Unable to read the checkout."
              }
              actions={
                workbenchStore.workspace === null
                  ? [
                      { action: "add_project", label: "Add a project", icon: "folder-open" },
                      { action: "new_worktree", label: "New worktree", icon: "git-branch" },
                    ]
                  : []
              }
            />
          </Show>
        }
      >
        <Show
          when={rows().length > 0}
          fallback={
            <p class="empty-copy">
              {query().trim() === "" ? "This checkout has no files." : "Nothing matches that."}
            </p>
          }
        >
          {/* U5. `role="tree"` with a single tab stop and
              `aria-activedescendant`: the rows are the tree items, only the
              container is focusable, and the row the chords act on is named
              rather than focused — which is what keeps a 50 000-row virtualised
              list from having to move real focus on every `j`. */}
          <div
            class="file-tree"
            role="tree"
            aria-label="Files"
            aria-activedescendant={rows()[index()] ? rowId(rows()[index()].path) : undefined}
            tabIndex={0}
            ref={watchSize}
            onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          >
            <div
              class="file-tree-spacer"
              role="presentation"
              style={{ height: `calc(${windowed().start} * var(--row-h))` }}
            />
            <For each={windowed().items}>
              {(row, position) => {
                const absolute = () => windowed().start + position();
                return (
                  <div
                    id={rowId(row.path)}
                    role="treeitem"
                    aria-level={row.depth + 1}
                    aria-selected={absolute() === index()}
                    aria-expanded={row.isFile || row.opaque ? undefined : !row.folded}
                    class="forge-row tree-row file"
                    classList={{
                      active: absolute() === index(),
                      directory: !row.isFile,
                      ignored: row.ignored,
                    }}
                    data-path={row.path}
                    style={{ "--depth": String(row.depth) }}
                    onClick={() => {
                      select(row.path);
                      // An ignored directory has no children to show: the
                      // listing stopped at its name, so a click that toggled
                      // it would flip a twisty over nothing.
                      if (row.opaque) return;
                      if (!row.isFile) {
                        row.folded ? unfold(row.path) : fold(row.path);
                      } else {
                        open(row.path);
                      }
                    }}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      select(row.path);
                      setMenu({ x: event.clientX, y: event.clientY, path: row.path });
                    }}
                  >
                    <span class="tree-twisty" classList={{ open: !row.folded }}>
                      {!row.isFile && !row.opaque ? "›" : ""}
                    </span>
                    {/* Directories carry the state the twisty already shows, so
                        the open folder is only ever under an open twisty, and
                        it stays a tinted lucide glyph: structure is chrome and
                        takes the row's colour. A file's mark comes from its
                        name and nothing else — the tree is a list of names,
                        not of contents. */}
                    <Show
                      when={row.isFile}
                      fallback={
                        <Icon
                          name={row.folded ? "folder" : "folder-open"}
                          class="forge-icon-muted"
                          size={14}
                        />
                      }
                    >
                      <LangIcon path={row.label} size={14} />
                    </Show>
                    {/* The one place `title` stays (§4.3 U17): the full path
                        on a truncated label is a detail nobody needs in order
                        to operate the row, and a Kobalte tooltip trigger per
                        row would put a portal and three listeners on every one
                        of the 50 000 paths the tree is budgeted for. */}
                    {/* U8: the name itself carries git's verdict, the way
                        every file browser this shell is measured against does.
                        The letter beside it stays — colour alone cannot say
                        *what* happened, and cannot be read at all by someone
                        who does not distinguish these five hues. A directory
                        borrows the modified tone from the roll-up: something
                        under it changed, and which of the five it was is a
                        question only its children can answer. */}
                    <span
                      class="tree-label"
                      data-git={
                        row.isFile
                          ? marks().get(row.path)?.tone
                          : counts().has(row.path)
                            ? "modified"
                            : undefined
                      }
                      title={
                        row.opaque ? `${row.path} — ignored by git; contents not listed` : row.path
                      }
                    >
                      {row.label}
                    </span>
                    {/* U8: what git says about this path, from the same
                        `LoadDiff` the Diff tab renders. A folder carries the
                        count of what is changed beneath it, which is the only
                        thing a folded directory can usefully say. */}
                    <Show when={row.isFile ? marks().get(row.path) : undefined}>
                      {(mark) => (
                        <span class={`tree-git tree-git-${mark().tone}`} aria-hidden="true">
                          {mark().mark}
                        </span>
                      )}
                    </Show>
                    <Show when={!row.isFile ? counts().get(row.path) : undefined}>
                      {(count) => <span class="tree-git-count">{count()}</span>}
                    </Show>
                  </div>
                );
              }}
            </For>
            <div
              class="file-tree-spacer"
              role="presentation"
              style={{ height: `calc(${windowed().total - windowed().end} * var(--row-h))` }}
            />
          </div>
        </Show>
        {/* A truncated listing that reads as exhaustive is worse than one that
            admits it stopped. */}
        <Show when={workbenchStore.tree?.truncated}>
          <p class="panel-note">Listing stopped at the scan budget — not every file is here.</p>
        </Show>
      </Show>
      <Show when={menu()}>
        {(open) => (
          <ContextMenu
            x={open().x}
            y={open().y}
            items={menuItems(
              open().path,
              rows().find((row) => row.path === open().path)?.isFile ?? true,
            )}
            onDismiss={() => setMenu(null)}
          />
        )}
      </Show>
    </div>
  );
}
