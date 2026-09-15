// Forge adapts workspace reads, mutations and shortcuts to the portable explorer.
import {
  createFileExplorer,
  type EditRequest,
  type ExplorerDerived,
  type ExplorerHandle,
  type FileDecoration,
  type RowIcon,
  type TreeRow,
} from "@forge-node/file-workbench";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  untrack,
} from "solid-js";
import { FILES } from "../actions/actions";
import { enterContext, registerAction } from "../actions/dispatch";
import {
  clearFindInFiles,
  clearTreeReveal,
  currentViews,
  findInFilesPending,
  openEditor,
  openEditorAt,
  treeReveal,
} from "../store/viewsStore";
import { forgeStore } from "../store/forgeStore";
import { setLoading, setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import {
  beginWorkbenchRequest,
  createPath,
  deletePath,
  failWorkbenchRequest,
  loadFileDirectory,
  loadFileTree,
  openFile,
  renamePath,
  searchFiles,
  warmFileTree,
} from "../workbench/api";
import { watchFiles, fileWatchError } from "../workbench/fileWatch";
import { ensureDiff } from "../workbench/decorations";
import { installTreeFollow } from "../workbench/treeFollow";
import { requestConfirm } from "../store/runtimeStore";
import { fileDecorations, folderCounts } from "../workbench/treeDecorations";
import { Icon, langIconUrl } from "../theme/icons";
import { themeBase } from "../theme/ThemeProvider";
import { baseIsLight } from "../theme/tokens";
import {
  ContextMenu,
  EmptyState,
  FilterHeader,
  IconButton,
  Skeleton,
  Tooltip,
  type MenuItem,
} from "../ui";
import {
  CONTENT_SEARCH_DEBOUNCE_MS,
  groupContentHits,
  shouldSearchContent,
} from "./fileContentSearch";

type FilesMode = "filter" | "content";

/* The two folder glyphs the package cannot render.
 *
 * `ICONS` are lucide *Solid components* and the package may not import Solid,
 * so a mark it draws has to be a URL. These are the same two glyphs traced out
 * to `public/icons/ui/`, placed by the package as a tint (`mask-image` over
 * `currentColor`) rather than as artwork — see that directory's README. The
 * alternative, a Solid overlay the host positions over the list, would put a
 * component on every one of the 50 000 rows the tree is budgeted for. */
const FOLDER_ICON = "/icons/ui/folder.svg";
const FOLDER_OPEN_ICON = "/icons/ui/folder-open.svg";

export function FileTreePanel() {
  let host!: HTMLDivElement;
  let explorer: ExplorerHandle | undefined;
  let leaveFiles: (() => void) | undefined;
  let filterInput: HTMLInputElement | undefined;
  const [directories, setDirectories] = createSignal<string[]>([""]);
  const [invalidated, setInvalidated] = createSignal(false);
  const [mounted, setMounted] = createSignal(false);
  const [mode, setMode] = createSignal<FilesMode>("filter");
  const [filterQuery, setFilterQuery] = createSignal("");
  const [contentQuery, setContentQuery] = createSignal("");
  /** Needle we last asked the daemon for in content mode; correlates the answer. */
  const [contentAsked, setContentAsked] = createSignal<string | null>(null);
  const [derived, setDerived] = createSignal<ExplorerDerived>({
    rows: 0,
    files: 0,
    filtered: false,
    truncated: false,
    // Seeded from the store: the explorer is only built in `onMount`, and a
    // panel that renders once with `loading: false` flashes the empty state
    // over a read that is already in flight.
    loading: workbenchStore.loading.tree,
  });
  const [menu, setMenu] = createSignal<{
    x: number;
    y: number;
    path: string;
    isFile: boolean;
  } | null>(null);
  const contentResults = createMemo(() => {
    const asked = contentAsked();
    const results = workbenchStore.search;
    if (asked === null || !results || results.query !== asked) return null;
    return results;
  });
  const contentGroups = createMemo(() => {
    const results = contentResults();
    return results ? groupContentHits(results.matches) : [];
  });
  const decorations = createMemo(() => {
    const marks = fileDecorations(workbenchStore.diff?.files ?? []);
    const result = new Map<string, FileDecoration>();
    for (const [path, mark] of marks) result.set(path, { label: mark.mark, tone: mark.tone });
    for (const [path, count] of folderCounts(marks.keys()))
      result.set(path, { label: String(count), tone: "modified" });
    return result;
  });
  /* A file wears its language mark, in the artwork's own colours, because a
     file type is not a choice and the hue is what makes the tree scannable. A
     directory wears the folder the twisty already implies, tinted, because
     structure is chrome. Both are read off the name, which is all `ListFiles`
     carries — never the contents. The resolver runs during a paint rather than
     under the effect, so the variant goes to `setState` as the revision and
     the rows repaint when the theme flips. */
  const light = createMemo(() => baseIsLight(themeBase()));
  function iconFor(row: TreeRow): RowIcon | null {
    if (!row.isFile) return { url: row.folded ? FOLDER_ICON : FOLDER_OPEN_ICON, tint: true };
    return { url: langIconUrl(row.path, untrack(light)) };
  }
  function open(path: string): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    beginWorkbenchRequest("file");
    openEditor(path);
    void openFile(workspace, path).catch((error) => failWorkbenchRequest("file", error));
  }
  function refresh(): void {
    const workspace = workbenchStore.workspace;
    if (!workspace || workbenchStore.loading.tree) return;
    beginWorkbenchRequest("tree");
    void loadFileTree(workspace).catch((error) => failWorkbenchRequest("tree", error));
  }
  onMount(() => {
    explorer = createFileExplorer(host, {
      chrome: "list",
      icon: iconFor,
      onOpen: open,
      onDerived: setDerived,
      onDirectoriesChange: setDirectories,
      onEditCommit: commitEdit,
      // Whatever closed the field — Escape, a click away, or the renamed row
      // leaving the listing because the write landed — nothing is waiting now.
      onEditCancel: () => {
        pending = null;
      },
      onFilterFocus: () => filterInput?.focus({ preventScroll: true }),
      onContextMenu: (row, point) => setMenu({ ...point, path: row.path, isFile: row.isFile }),
      onExpandOpaque: (path) => {
        const workspace = workbenchStore.workspace;
        if (!workspace) return;
        setLoading("directory", true);
        void loadFileDirectory(workspace, path).catch((error) => {
          setLoading("directory", false);
          failWorkbenchRequest("tree", error);
        });
      },
    });
    const bindings = [
      registerAction("file_tree_next", () => explorer?.action("next")),
      registerAction("file_tree_previous", () => explorer?.action("previous")),
      registerAction("file_tree_expand", () => explorer?.action("expand")),
      registerAction("file_tree_collapse", () => explorer?.action("collapse")),
      registerAction("file_tree_open", () => explorer?.action("open")),
      registerAction("file_tree_first", () => explorer?.action("first")),
      registerAction("file_tree_last", () => explorer?.action("last")),
      registerAction("file_tree_filter", () => {
        setMode("filter");
        explorer?.action("filter");
      }),
      registerAction("file_tree_refresh", refresh),
      registerAction("file_tree_rename", () => {
        const row = explorer?.selected();
        if (row) rename(row.path);
      }),
      registerAction("file_tree_delete", () => {
        const row = explorer?.selected();
        if (row) remove(row.path, row.isFile);
      }),
    ];
    setMounted(true);
    onCleanup(() => {
      explorer?.destroy();
      leaveFiles?.();
      for (const unbind of bindings) unbind();
    });
  });
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    const paths = directories();
    if (workspace) onCleanup(watchFiles(workspace, paths, () => setInvalidated(true)));
  });
  createEffect(() => {
    if (!invalidated() || workbenchStore.loading.tree) return;
    const timer = setTimeout(() => {
      setInvalidated(false);
      refresh();
    }, 250);
    onCleanup(() => clearTimeout(timer));
  });
  let previousWorkspace: string | null | undefined;
  createEffect(() => {
    if (!mounted()) return;
    const workspace = workbenchStore.workspace;
    if (workspace !== previousWorkspace) {
      previousWorkspace = workspace;
      explorer?.reset();
      setFilterQuery("");
      setContentQuery("");
      setContentAsked(null);
      setMode("filter");
      setMenu(null);
    }
    // No `error`: in `chrome: "list"` the package draws no message, and the
    // panel renders the failure and the empty states with Forge's own kit.
    const state = {
      tree: workbenchStore.tree,
      loading: workbenchStore.loading.tree,
      decorations: decorations(),
      revision: light(),
    };
    untrack(() => explorer?.setState(state));
  });
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    if (workspace) warmFileTree(workspace);
    ensureDiff();
  });
  /* The tree follows the active editor on its own rather than being pushed by
     each opener: the palette, a path link, a tab click and a close falling
     back to a neighbour all end in `active`, and a parked view restored on a
     workspace switch is not a call at all. Registered before the reveal effect
     below so an explicit "show me this path" — a breadcrumb segment, a
     changes-list row — still wins when the panel is mounting. */
  installTreeFollow({
    mounted,
    tree: () => workbenchStore.tree,
    view: () => currentViews().active,
    follow: (path) => explorer?.follow(path),
  });
  createEffect(() => {
    if (!mounted() || !workbenchStore.tree) return;
    const path = treeReveal();
    if (path) {
      setMode("filter");
      explorer?.reveal(path);
      // `reveal` drops the filter to guarantee the row is in the listing, so
      // the box the person can see has to drop it too.
      setFilterQuery("");
      clearTreeReveal();
    }
  });
  // `Mod-shift-f` may fire while this panel is unmounted; the pending flag
  // stands until we open and focus the content-search field.
  createEffect(() => {
    const asked = findInFilesPending();
    if (!asked) return;
    clearFindInFiles();
    setMode("content");
    // A request that names a symbol runs it; one that does not is the chord
    // asking for the field, and must not wipe what is already typed there.
    if (asked.query !== null) setContentQuery(asked.query);
    requestAnimationFrame(() => filterInput?.focus({ preventScroll: true }));
  });
  createEffect(() => {
    if (mode() !== "content") return;
    const workspace = workbenchStore.workspace;
    const needle = contentQuery().trim();
    if (!workspace || !shouldSearchContent(needle)) {
      setContentAsked(null);
      return;
    }
    // Depend only on mode / workspace / query — not on the shared search slot
    // or `contentAsked`. Clearing that slot before a request must not schedule
    // another grep.
    const timer = setTimeout(() => {
      setWorkbenchStore({ search: null, searchError: null });
      setContentAsked(needle);
      setLoading("search", true);
      void searchFiles(workspace, needle, "content").catch((error: unknown) => {
        setLoading("search", false);
        setWorkbenchStore("searchError", error instanceof Error ? error.message : String(error));
      });
    }, CONTENT_SEARCH_DEBOUNCE_MS);
    onCleanup(() => clearTimeout(timer));
  });
  function dirname(path: string): string {
    const cut = path.lastIndexOf("/");
    return cut < 0 ? "" : path.slice(0, cut);
  }

  function parentOf(path: string, isFile: boolean): string {
    return isFile ? dirname(path) : path;
  }

  function under(parent: string, name: string): string {
    return parent === "" ? name : `${parent}/${name}`;
  }

  function withWorkspace(run: (workspace: string) => Promise<unknown>): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    void run(workspace).catch((error) => failWorkbenchRequest("tree", error));
  }

  /* A11: create and rename are typed on the row, the way an editor does it.
     Only delete keeps a dialog — it is the one that cannot be taken back. */
  function create(path: string, isFile: boolean, directory: boolean): void {
    explorer?.edit({ kind: "create", parent: parentOf(path, isFile), directory });
  }

  function rename(path: string): void {
    explorer?.edit({ kind: "rename", path });
  }

  /**
   * The path an open field is waiting to see land.
   *
   * `create_path` / `rename_path` go to the host's workbench worker, which
   * answers with a re-listing on success and a `workbench:file_failed` event
   * on refusal — the `invoke` resolves either way, so neither outcome reaches
   * the promise. The listing arriving is not itself the answer: the watcher
   * re-reads the checkout for any write, an agent's included. The path being
   * *in* it is.
   */
  let pending: string | null = null;

  function commitEdit(request: EditRequest, name: string): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) {
      explorer?.edit(null);
      return;
    }
    const target =
      request.kind === "rename" ? under(dirname(request.path), name) : under(request.parent, name);
    setWorkbenchStore("fileError", null);
    pending = target;
    const write =
      request.kind === "rename"
        ? renamePath(workspace, request.path, target)
        : createPath(workspace, target, request.directory);
    void write.catch((error) => {
      pending = null;
      explorer?.editFailed(error instanceof Error ? error.message : String(error));
    });
  }

  createEffect(() => {
    const error = workbenchStore.fileError;
    const entries = workbenchStore.tree?.entries;
    const target = pending;
    if (!target) return;
    untrack(() => {
      if (error) {
        pending = null;
        // On the row, not in a toast: the name that was refused is the thing
        // the person is looking at, and it stays there to be corrected.
        explorer?.editFailed(error);
      } else if (entries?.some((entry) => entry.path === target)) {
        pending = null;
        explorer?.edit(null);
      }
    });
  });

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
    // panel to the filter box or a row inside it must not read as leaving.
    <div
      class="panel-body fw-host"
      onFocusIn={() => {
        leaveFiles ??= enterContext(FILES);
      }}
      onFocusOut={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
          leaveFiles?.();
          leaveFiles = undefined;
        }
      }}
    >
      <FilterHeader
        label={mode() === "filter" ? "Filter files" : "Search in files"}
        placeholder={mode() === "filter" ? "Filter files…" : "Search in files…"}
        query={mode() === "filter" ? filterQuery() : contentQuery()}
        onQuery={(value) => {
          if (mode() === "filter") {
            setFilterQuery(value);
            explorer?.setFilter(value);
          } else {
            setContentQuery(value);
          }
        }}
        rows={[
          {
            label: "Files panel mode",
            value: mode(),
            options: [
              { value: "filter" as const, label: "files" },
              { value: "content" as const, label: "text" },
            ],
            onChange: (value: FilesMode) => {
              setMode(value);
              if (value === "filter") explorer?.setFilter(filterQuery());
              else setContentAsked(null);
            },
          },
        ]}
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
      <Show when={fileWatchError()}>{(error) => <p class="panel-note">{error()}</p>}</Show>
      {/* The mount stays in the tree for the panel's life — the package owns
          that element from `onMount` on. Content mode only hides it. */}
      <div ref={host} class="fw-mount" hidden={mode() === "content" || derived().rows === 0} />
      <Show when={mode() === "content"}>
        <Show when={workbenchStore.searchError}>
          {(error) => <p class="panel-error">{error()}</p>}
        </Show>
        <Show
          when={workbenchStore.workspace !== null}
          fallback={<EmptyState message="No checkout selected." />}
        >
          <Show
            when={shouldSearchContent(contentQuery())}
            fallback={
              <p class="empty-copy">Type at least two characters to search the checkout.</p>
            }
          >
            <Show when={workbenchStore.loading.search && !contentResults()}>
              <p class="panel-note">Searching…</p>
            </Show>
            <Show when={contentResults()}>
              {(results) => (
                <>
                  <Show
                    when={results().matches.length > 0}
                    fallback={<p class="empty-copy">Nothing matches that.</p>}
                  >
                    <p class="panel-note">
                      {`${results().matches.length} match${results().matches.length === 1 ? "" : "es"}`}
                      {results().truncated ? " (truncated)" : ""}
                    </p>
                    <ul class="file-search-hits">
                      <For each={contentGroups()}>
                        {(group) => (
                          <li class="file-search-group">
                            <div class="file-search-path">{group.path}</div>
                            <ul>
                              <For each={group.matches}>
                                {(match) => (
                                  <li>
                                    <button
                                      type="button"
                                      class="forge-row file-search-hit"
                                      onClick={() => openEditorAt(match.path, match.line)}
                                    >
                                      <span class="file-search-line">{match.line}</span>
                                      <span class="file-search-text">{match.text.trim()}</span>
                                    </button>
                                  </li>
                                )}
                              </For>
                            </ul>
                          </li>
                        )}
                      </For>
                    </ul>
                  </Show>
                  <Show when={results().truncated}>
                    <p class="panel-note">
                      Search stopped at the hit budget — not every match is here.
                    </p>
                  </Show>
                </>
              )}
            </Show>
          </Show>
        </Show>
      </Show>
      <Show when={mode() === "filter"}>
        <Show
          when={workbenchStore.tree}
          fallback={
            <Show
              when={!derived().loading}
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
          <Show when={derived().rows === 0}>
            <p class="empty-copy">
              {derived().filtered ? "Nothing matches that." : "This checkout has no files."}
            </p>
          </Show>
          <Show when={derived().rows > 0}>
            <p class="panel-note">{`${derived().files} files shown`}</p>
          </Show>
          {/* A truncated listing that reads as exhaustive is worse than one that
              admits it stopped. */}
          <Show when={derived().truncated}>
            <p class="panel-note">Listing stopped at the scan budget — not every file is here.</p>
          </Show>
        </Show>
      </Show>
      <Show when={menu()}>
        {(item) => (
          <ContextMenu
            x={item().x}
            y={item().y}
            items={menuItems(item().path, item().isFile)}
            onDismiss={() => setMenu(null)}
          />
        )}
      </Show>
    </div>
  );
}
