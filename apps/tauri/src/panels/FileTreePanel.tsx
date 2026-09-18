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
import { enterContext, invokeAction, registerAction } from "../actions/dispatch";
import { clearTreeReveal, currentViews, openEditor, treeReveal } from "../store/viewsStore";
import { forgeStore } from "../store/forgeStore";
import { workbenchStore } from "../store/workbenchStore";
import {
  beginWorkbenchRequest,
  createPath,
  deletePath,
  ensureDirectory,
  failWorkbenchRequest,
  invalidateDirectories,
  openFile,
  renamePath,
  setDirectoryInterests,
} from "../workbench/api";
import {
  directoryError,
  directoryLoading,
  directoryStatus,
  directoryTree,
} from "../workbench/directoryState";
import { parentPath, validatePath, validateRename } from "../workbench/pathOperations";
import { subscribePathOperations } from "../workbench/operations";
import { watchFiles, fileWatchError, rearmFileWatches } from "../workbench/fileWatch";
import { ensureDiff } from "../workbench/decorations";
import { installTreeFollow } from "../workbench/treeFollow";
import { requestConfirm, requestTextInput } from "../store/runtimeStore";
import { fileDecorations, folderCounts } from "../workbench/treeDecorations";
import { Icon, langIconUrl } from "../theme/icons";
import { themeBase } from "../theme/ThemeProvider";
import { baseIsLight } from "../theme/tokens";
import { ContextMenu, EmptyState, Skeleton, type MenuItem } from "../ui";
import { createFileDrag } from "../workbench/fileDrag";
import { sendTargetedPaste } from "../runtime/api";
import { runtimeStore } from "../store/runtimeStore";

const FOLDER_ICON = "/icons/ui/folder.svg";
const FOLDER_OPEN_ICON = "/icons/ui/folder-open.svg";
type MenuTarget = { kind: "root" } | { kind: "file" | "directory"; path: string };

export function FileTreePanel() {
  let host!: HTMLDivElement;
  let explorer: ExplorerHandle | undefined;
  let drag: ReturnType<typeof createFileDrag> | undefined;
  let leaveFiles: (() => void) | undefined;
  let pending: object | null = null;
  let disposed = false;
  let interactionEpoch = 0;
  const [directories, setDirectories] = createSignal<string[]>([""]);
  const [mounted, setMounted] = createSignal(false);
  const [operationError, setOperationError] = createSignal<string | null>(null);
  const [derived, setDerived] = createSignal<ExplorerDerived>({
    rows: 0,
    files: 0,
    filtered: false,
    truncated: false,
    loading: directoryLoading(),
  });
  const [menu, setMenu] = createSignal<{ x: number; y: number; target: MenuTarget } | null>(null);
  const listing = createMemo(() => {
    const tree = directoryTree();
    return tree ? { ...tree } : null;
  });
  const workspaceInfo = createMemo(() =>
    forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace),
  );
  const decorations = createMemo(() => {
    const marks = fileDecorations(workbenchStore.diff?.files ?? []);
    const result = new Map<string, FileDecoration>();
    for (const [path, mark] of marks) result.set(path, { label: mark.mark, tone: mark.tone });
    for (const [path, count] of folderCounts(marks.keys()))
      result.set(path, { label: String(count), tone: "modified" });
    return result;
  });
  const directoryFailures = createMemo(() =>
    directories().flatMap((path) => {
      const error = path ? directoryStatus(path)?.error : null;
      return error ? [{ path, error }] : [];
    }),
  );
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
    if (workspace) {
      invalidateDirectories(workspace);
      rearmFileWatches(workspace, "");
    }
  }
  function search(): void {
    invokeAction("open_file_palette");
  }
  onMount(() => {
    drag = createFileDrag(host, {
      workspace: () => {
        const workspace = workspaceInfo();
        return workspace ? { id: workspace.id, path: workspace.path } : null;
      },
      connected: () => runtimeStore.connection.kind === "connected",
      editing: () => explorer?.isEditing() ?? false,
      expand: (path) => explorer?.expand(path),
      reveal: (path) => explorer?.reveal(path),
      move: renamePath,
      paste: sendTargetedPaste,
      error: setOperationError,
    });
    explorer = createFileExplorer(host, {
      chrome: "list",
      icon: iconFor,
      onOpen: open,
      onPointerDown: (event, row) => {
        const workspaceId = workbenchStore.workspace;
        if (workspaceId) drag?.down(event, { workspaceId, path: row.path, isFile: row.isFile });
      },
      onDerived: setDerived,
      onDirectoriesChange: setDirectories,
      onEditCommit: commitEdit,
      onEditCancel: () => {
        pending = null;
      },
      onFilterFocus: search,
      onContextMenu: (row, point) =>
        setMenu({
          ...point,
          target: { kind: row.isFile ? "file" : "directory", path: row.path },
        }),
      onExpandDirectory: (path) => {
        const workspace = workbenchStore.workspace;
        if (workspace) ensureDirectory(workspace, path);
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
      registerAction("file_tree_filter", search),
      registerAction("file_tree_refresh", refresh),
      registerAction("file_tree_rename", () => {
        const row = explorer?.selected();
        if (row) explorer?.edit({ kind: "rename", path: row.path });
      }),
      registerAction("file_tree_delete", () => {
        const row = explorer?.selected();
        if (row) remove(row.path, row.isFile);
      }),
      subscribePathOperations((result) => {
        if (
          result.workspace !== workbenchStore.workspace ||
          result.kind !== "rename" ||
          pending ||
          explorer?.isEditing()
        )
          return;
        if (result.from && result.to) explorer?.retarget(result.from, result.to);
      }),
    ];
    setMounted(true);
    onCleanup(() => {
      disposed = true;
      drag?.destroy();
      pending = null;
      explorer?.destroy();
      leaveFiles?.();
      for (const unbind of bindings) unbind();
    });
  });
  createEffect(() => {
    runtimeStore.connection;
    runtimeStore.connection.kind;
    runtimeStore.connectionGeneration;
    runtimeStore.activeSession;
    runtimeStore.activeTerminal;
    workbenchStore.workspace;
    drag?.cancel();
  });
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    const paths = directories();
    if (!workspace) return;
    untrack(() => {
      setDirectoryInterests(workspace, paths);
      onCleanup(watchFiles(workspace, paths, () => undefined));
    });
  });
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    if (workspace) onCleanup(() => setDirectoryInterests(workspace, []));
  });
  let previousWorkspace: string | null | undefined;
  createEffect(() => {
    if (!mounted()) return;
    const workspace = workbenchStore.workspace;
    if (workspace !== previousWorkspace) {
      interactionEpoch++;
      previousWorkspace = workspace;
      pending = null;
      explorer?.reset();
      setOperationError(null);
      setMenu(null);
    }
    const state = {
      tree: listing(),
      loading: directoryLoading(),
      decorations: decorations(),
      revision: light(),
    };
    untrack(() => explorer?.setState(state));
  });
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    if (workspace) untrack(() => ensureDirectory(workspace));
    ensureDiff();
  });
  installTreeFollow({
    mounted,
    tree: listing,
    view: () => currentViews().active,
    follow: (path) => explorer?.follow(path),
  });
  createEffect(() => {
    if (!mounted() || !workbenchStore.workspace) return;
    const path = treeReveal();
    if (path) {
      untrack(() => explorer?.reveal(path));
      clearTreeReveal();
    }
  });

  function under(parent: string, name: string): string {
    return parent === "" ? name : `${parent}/${name}`;
  }
  function create(target: MenuTarget, directory: boolean): void {
    if (!workbenchStore.workspace) return;
    const parent =
      target.kind === "root" ? "" : target.kind === "file" ? parentPath(target.path) : target.path;
    explorer?.edit({ kind: "create", parent, directory });
  }
  function commitEdit(request: EditRequest, name: string): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) {
      explorer?.editFailed("Select a checkout first.");
      return;
    }
    const target = under(
      request.kind === "rename" ? parentPath(request.path) : request.parent,
      name,
    );
    const error =
      name.trim() === ""
        ? "Enter a name."
        : request.kind === "rename" && /[/\\]/.test(name)
          ? "Enter a name without separators. Use Move to… to choose another folder."
          : (validatePath(name) ??
            (request.kind === "rename"
              ? validateRename(request.path, target)
              : validatePath(target)));
    if (error) {
      explorer?.editFailed(error);
      return;
    }
    const attempt = {};
    pending = attempt;
    setOperationError(null);
    const write =
      request.kind === "rename"
        ? renamePath(workspace, request.path, target)
        : createPath(workspace, target, request.directory);
    void write
      .then(() => {
        if (disposed || pending !== attempt || workbenchStore.workspace !== workspace) return;
        pending = null;
        explorer?.edit(null);
        explorer?.reveal(target);
        if (request.kind === "create" && !request.directory) open(target);
      })
      .catch((error) => {
        if (disposed || pending !== attempt || workbenchStore.workspace !== workspace) return;
        pending = null;
        explorer?.editFailed(error instanceof Error ? error.message : String(error));
      });
  }
  function move(path: string, value = parentPath(path), error?: string): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    const epoch = interactionEpoch;
    const current = () =>
      !disposed && workspace === workbenchStore.workspace && epoch === interactionEpoch;
    let submitting = false;
    requestTextInput({
      title: `Move ${path}`,
      label: error ?? "Destination folder relative to checkout (empty for root)",
      value,
      placeholder: "e.g. src/components",
      confirmLabel: "Move",
      allowEmpty: true,
      onSubmit: (parent) => {
        if (submitting || !current()) return;
        submitting = true;
        const target = under(parent, path.slice(path.lastIndexOf("/") + 1));
        const invalid = (parent ? validatePath(parent) : null) ?? validateRename(path, target);
        // The shared prompt dismisses synchronously after submit; reopen after that dismissal.
        if (invalid) {
          queueMicrotask(() => {
            if (current()) move(path, parent, invalid);
          });
          return;
        }
        void renamePath(workspace, path, target)
          .then(() => {
            if (current()) explorer?.reveal(target);
          })
          .catch((failure) => {
            if (!current()) return;
            move(path, parent, failure instanceof Error ? failure.message : String(failure));
          });
      },
    });
  }
  function remove(path: string, isFile: boolean): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    const epoch = interactionEpoch;
    let submitting = false;
    requestConfirm({
      title: `Delete ${path}?`,
      description: isFile
        ? "The file is removed from the checkout. Git is the only way back."
        : "The folder and everything in it are removed from the checkout. Git is the only way back.",
      confirmLabel: "Delete",
      destructive: true,
      onConfirm: () => {
        if (
          submitting ||
          disposed ||
          epoch !== interactionEpoch ||
          workspace !== workbenchStore.workspace
        )
          return;
        submitting = true;
        void deletePath(workspace, path).catch((error) => {
          if (!disposed && epoch === interactionEpoch && workbenchStore.workspace === workspace)
            setOperationError(error instanceof Error ? error.message : String(error));
        });
      },
    });
  }
  function menuItems(target: MenuTarget): MenuItem[] {
    const workspace = workbenchStore.workspace;
    const path = target.kind === "root" ? "" : target.path;
    const root = workspaceInfo()?.path;
    const absolute = root ? (path ? `${root.replace(/\/$/, "")}/${path}` : root) : path;
    const createItems: MenuItem[] = [
      {
        kind: "item",
        label: "New file…",
        icon: "file",
        disabled: !workspace,
        run: () => create(target, false),
      },
      {
        kind: "item",
        label: "New folder…",
        icon: "folder",
        disabled: !workspace,
        run: () => create(target, true),
      },
    ];
    const copy: MenuItem = {
      kind: "item",
      label: target.kind === "root" ? "Copy workspace path" : "Copy path",
      icon: "copy",
      disabled: !root,
      run: () => void navigator.clipboard?.writeText(absolute).catch(() => undefined),
    };
    if (target.kind === "root")
      return [
        ...createItems,
        { kind: "rule" },
        { kind: "item", label: "Refresh", icon: "refresh", disabled: !workspace, run: refresh },
        {
          kind: "item",
          label: "Collapse folders",
          icon: "folder",
          run: () => explorer?.collapseAll(),
        },
        copy,
      ];
    return [
      ...(target.kind === "file"
        ? [
            { kind: "item" as const, label: "Open", icon: "file" as const, run: () => open(path) },
            {
              kind: "item" as const,
              label: "Insert reference in terminal",
              icon: "square-terminal" as const,
              run: () =>
                host.dispatchEvent(
                  new CustomEvent("forge:file-reference", {
                    bubbles: true,
                    detail: { workspaceId: workspace, path },
                  }),
                ),
            },
          ]
        : []),
      copy,
      {
        kind: "item",
        label: "Copy relative path",
        icon: "copy",
        run: () => void navigator.clipboard?.writeText(path).catch(() => undefined),
      },
      { kind: "rule" },
      ...createItems,
      {
        kind: "item",
        label: "Rename…",
        icon: "edit",
        run: () => explorer?.edit({ kind: "rename", path }),
      },
      { kind: "item", label: "Move to…", icon: "folder", run: () => move(path) },
      {
        kind: "item",
        label: "Delete",
        icon: "trash",
        destructive: true,
        run: () => remove(path, target.kind === "file"),
      },
    ];
  }
  function textTarget(target: EventTarget | null): boolean {
    return target instanceof Element && !!target.closest("input, textarea, [contenteditable=true]");
  }
  return (
    <div
      class="panel-body fw-host"
      tabIndex={0}
      onContextMenu={(event) => {
        if (textTarget(event.target)) return;
        event.preventDefault();
        setMenu({ x: event.clientX, y: event.clientY, target: { kind: "root" } });
      }}
      onKeyDown={(event) => {
        if (textTarget(event.target)) return;
        if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
        event.preventDefault();
        event.stopPropagation();
        const rect = event.currentTarget.getBoundingClientRect();
        setMenu({ x: rect.left + 16, y: rect.top + 24, target: { kind: "root" } });
      }}
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
      <button
        type="button"
        class="fw-root-identity"
        title={workspaceInfo()?.path}
        onClick={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          setMenu({ x: rect.left, y: rect.bottom, target: { kind: "root" } });
        }}
      >
        <Icon name="folder-open" size={14} />
        <span>{workspaceInfo()?.path.split("/").filter(Boolean).at(-1) ?? "Files"}</span>
      </button>
      <Show when={directoryError()}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      <For each={directoryFailures()}>
        {(failure) => (
          <p class="panel-error">
            {failure.path}: {failure.error}{" "}
            <button
              type="button"
              onClick={() => {
                const workspace = workbenchStore.workspace;
                if (workspace) ensureDirectory(workspace, failure.path, true);
              }}
            >
              Retry
            </button>
          </p>
        )}
      </For>
      <Show when={operationError()}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      <Show when={fileWatchError()}>{(error) => <p class="panel-note">{error()}</p>}</Show>
      <div ref={host} class="fw-mount" />
      <Show when={!directoryTree()?.loadedDirectories?.includes("") && derived().rows === 0}>
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
          />
        </Show>
      </Show>
      <Show
        when={directoryTree() && derived().rows === 0 && !derived().loading && !directoryError()}
      >
        <p class="empty-copy">This checkout has no files. Use the folder menu to create one.</p>
      </Show>
      <Show when={derived().rows > 0}>
        <p class="panel-note">{`${derived().files} files shown`}</p>
      </Show>
      <Show when={derived().truncated}>
        <p class="panel-note">Listing stopped at the scan budget — not every entry is here.</p>
      </Show>
      <Show when={menu()}>
        {(item) => (
          <ContextMenu
            x={item().x}
            y={item().y}
            items={menuItems(item().target)}
            onDismiss={() => setMenu(null)}
          />
        )}
      </Show>
    </div>
  );
}
