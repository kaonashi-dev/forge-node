import { For, Show, createComputed, createMemo, createSelector, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { applyRailTree } from "./railTree";
import { SIDEBAR } from "../../actions/actions";
import { enterContext, registerAction } from "../../actions/dispatch";
import { onCleanup, onMount } from "solid-js";
import {
  addProjectFromPicker,
  createProjectGroup,
  moveProject,
  refreshProject,
  removeProjectGroup,
  renameProjectGroup,
  renameWorkspace,
  setProjectIcon,
  setWorktreeIgnores,
} from "./commands";
import { openInEditor, openInFileManager } from "../../runtime/host";
import { sessionTitle, type Session } from "../../contracts/runtime";
import { forgeStore } from "../../state/forgeStore";
import { requestTextInput } from "../../state/dialogs";
import { connectionStore, setNotice } from "../../state/connection";
import { projectDialogsStore, setProjectDialogsStore } from "./dialogs";
import { now } from "../../runtime/clock";
import { sessionWork } from "../sessions/work";
import { setAppState } from "../settings/commands";
import { refreshWorkspaceStatus } from "../sessions/commands";
import { openComposeForWorkspace } from "../pull-requests/PrComposeView";
import { centerMode, openCompare, showCode, viewsStore } from "../../navigation/viewsStore";
import { openFileCount } from "../../navigation/views";
import { focusSession, launchAgent, launchShell } from "../sessions/sessionActions";
import {
  Badge,
  Button,
  ContextMenu,
  Dialog,
  IconButton,
  Menu,
  Tooltip,
  buttonClass,
  type MenuItem,
} from "../../ui/index";
import {
  buildTree,
  railCollapseTarget,
  railExpandTarget,
  railRows,
  soleCheckout,
  type GroupNode,
  type ProjectNode,
  type WorkspaceNode,
} from "./tree";
import { prLabel, prTone, rollupWork, syncLabel, type PrTone } from "./workspaceCard";
import { seedFromAppState } from "../../state/preferences";
import { foldsPayload, parseFolds, toggleFold } from "./railFolds";
import { newWorktreeItem } from "./railMenuItems";
import { sessionMenuItems } from "../sessions/sessionMenuItems";
import { dropFromGap, gapAtY, moveTabToGap } from "../../navigation/tabOrder";
import { SharedFiles } from "./SharedFiles";
import { WorktreeIgnoresDialog } from "./WorktreeIgnoresDialog";
import { sharesStore } from "./sharesStore";
import { orderFromWorkspaces, parseWorkspaceOrder, type WorkspaceOrderMap } from "./workspaceOrder";
import { tabOrder } from "../sessions/tabOrderState";
/** A DOM id for a rail row, so `aria-activedescendant` can name it. */
function railRowId(id: string): string {
  return `rail-${encodeURIComponent(id)}`;
}
import { sessionDisplayTitle } from "../sessions/sessionTree";
import { waitedFor } from "../sessions/attention";
import { hasQuestion, waitingSessions } from "../sessions/waiting";
import { gitStore } from "../git/state";
import { BrandIcon, Icon } from "../../theme/icons/index";
import { AttentionMarker, StateMarker, WorkMarker } from "../sessions/markers";
import { SessionGlyph } from "../sessions/SessionGlyph";
import { draftWithJuva, loadBranchCompare } from "../git/commands";
import { activeWorkspace, focusWorkspace } from "../../state/workspace";
import { projectHue } from "../../theme/projectHue";

const COLLAPSED_KEY = "ui.sidebar.collapsed";
const WORKSPACE_ORDER_KEY = "ui.sidebar.workspace_order";
const CHECKOUT_DRAG_PX = 4;
const CHECKOUT_SCROLL_EDGE_PX = 40;

function focusCode(workspace: string): void {
  focusWorkspace(workspace);
  showCode();
}

/** Files a checkout has open in Code. */
function filesOpen(workspace: string): number {
  const views = viewsStore.byWorkspace[workspace];
  return views ? openFileCount(views) : 0;
}

/** The rail lists Code while the checkout has anything open there. */
function codeShown(workspace: string): boolean {
  return (
    (viewsStore.byWorkspace[workspace]?.open.length ?? 0) > 0 ||
    (centerMode() === "code" && activeWorkspace() === workspace)
  );
}

/** The loudest state anywhere under a folded project: one dot instead of a count. */
function projectWork(project: ProjectNode) {
  return rollupWork(
    project.workspaces.flatMap((workspace) =>
      workspace.sessions.map((node) =>
        sessionWork(
          node.session,
          { wants_you: node.wantsYou || hasQuestion(node.session.id) },
          now(),
        ),
      ),
    ),
  );
}

/** The palette a review decision paints the card's pull-request mark. */
const PR_ICON_CLASS: Record<PrTone, string> = {
  neutral: "forge-icon-muted",
  good: "forge-icon-green",
  warn: "forge-icon-amber",
  bad: "forge-icon-red",
};

/**
 * The project rail: group → project → workspace → session.
 *
 * Folding is persisted through the daemon as one blob rather than a key per
 * row: the set is small, and a row that appears later must not arrive folded
 * because some earlier build wrote a key for it.
 */
export function ProjectsView() {
  const [sharedFiles, setSharedFiles] = createSignal<{
    projectId: string;
    workspaceId?: string;
    name: string;
  } | null>(null);
  const [ignoredWorktrees, setIgnoredWorktrees] = createSignal<{
    projectId: string;
    name: string;
  } | null>(null);

  const [collapsed, setCollapsed] = createSignal<Set<string>>(readCollapsed());
  const [workspaceOrder, setWorkspaceOrder] = createSignal<WorkspaceOrderMap>(readWorkspaceOrder());
  const [menu, setMenu] = createSignal<{ x: number; y: number; items: MenuItem[] } | null>(null);

  /*
   * Same one-shot seed as the tab strip: the snapshot lands after first paint,
   * so both lines above read an empty `app_state` at creation. The order got
   * its correction here; the fold set did not, and so every relaunch opened
   * the rail fully expanded — the stored set was written on every fold and
   * read by nothing.
   *
   * Seeded once rather than tracked, for the reason `seedFromAppState`
   * documents: a later write to `app_state` must not re-fold a row the person
   * has since opened. `folded` covers the other end of the same race — a fold
   * made in the milliseconds before the snapshot lands is a choice, and the
   * stored set must not overwrite it.
   */
  let folded = false;
  seedFromAppState(() => {
    setWorkspaceOrder(readWorkspaceOrder());
    if (!folded) setCollapsed(readCollapsed());
  });

  function openMenu(event: MouseEvent, items: MenuItem[]): void {
    event.preventDefault();
    event.stopPropagation();
    setMenu({ x: event.clientX, y: event.clientY, items });
  }

  function launchItems(workspace: string): MenuItem[] {
    const shells: MenuItem[] = [];
    const agents: MenuItem[] = [];
    for (const launchable of forgeStore.launchables) {
      const items = launchable.kind === "shell" ? shells : agents;
      items.push({
        kind: "item",
        label: launchable.kind === "shell" ? "New Terminal" : launchable.label,
        ...(launchable.kind === "shell"
          ? { icon: "square-terminal" as const }
          : { glyph: <SessionGlyph providerId={launchable.provider ?? null} size={14} /> }),
        // Not installed stays listed and disabled, so the menu also answers
        // "what could run here?".
        detail: launchable.detail ?? undefined,
        disabled: !launchable.enabled,
        run: () =>
          launchable.kind === "shell"
            ? void launchShell(workspace).catch(() => undefined)
            : void launchAgent(launchable.provider ?? "", launchable.profile, workspace).catch(
                () => undefined,
              ),
      });
    }
    return [...shells, ...(agents.length ? [{ kind: "rule" } as const, ...agents] : [])];
  }

  /**
   * A project's own menu.
   *
   * Everything here acks and then broadcasts, so the rail redraws from the
   * snapshot the daemon sends rather than from a guess about what the click
   * did — which is why a refused move leaves the row exactly where it was.
   */
  function projectMenu(node: ProjectNode): MenuItem[] {
    const project = forgeStore.projects.find((item) => item.id === node.id);
    const groups = forgeStore.project_groups;
    const sole = soleCheckout(node);
    // A project with one checkout answers for it too; its rows already carry
    // the new worktree, shared files and the path.
    const own: MenuItem[] = sole
      ? []
      : [
          newWorktreeItem({ id: node.id, name: node.name }),
          {
            kind: "item",
            label: "Shared files…",
            icon: "folder-open",
            run: () => setSharedFiles({ projectId: node.id, name: node.name }),
          },
        ];
    const items: MenuItem[] = [
      ...own,
      {
        kind: "item",
        label: "Ignored worktrees…",
        detail: "Undo a folder Forge was told to stop adopting",
        icon: "filter",
        run: () => setIgnoredWorktrees({ projectId: node.id, name: node.name }),
      },
      {
        kind: "item",
        label: "Refresh project",
        detail: "Re-detect the git root, branches and worktrees",
        icon: "refresh",
        run: () => void refreshProject(node.id).catch(() => undefined),
      },
      {
        kind: "item",
        label: "Set icon…",
        icon: "appearance",
        run: () => {
          requestTextInput({
            title: "Set project icon",
            label: "Icon (one or two characters)",
            value: node.icon ?? "",
            placeholder: "🦀",
            confirmLabel: "Apply",
            allowEmpty: true,
            onSubmit: (icon) =>
              // An empty answer clears it, so the rail falls back to the initials
              // it derives from the name — there is no separate "clear" row.
              void setProjectIcon(node.id, icon || null).catch(() => undefined),
          });
        },
      },
      ...(groups.length > 0 || project?.project_group_id
        ? [
            { kind: "rule" as const },
            ...groups
              .filter((group) => group.id !== project?.project_group_id)
              .map((group) => ({
                kind: "item" as const,
                label: `Move to ${group.name}`,
                icon: "folder-open" as const,
                run: () => void moveProject(node.id, group.id).catch(() => undefined),
              })),
            ...(project?.project_group_id
              ? [
                  {
                    kind: "item" as const,
                    label: "Move out of the group",
                    icon: "folder-open" as const,
                    run: () => void moveProject(node.id, null).catch(() => undefined),
                  },
                ]
              : []),
          ]
        : []),
      ...(sole
        ? []
        : [
            { kind: "rule" as const },
            {
              kind: "item" as const,
              label: "Copy path",
              icon: "copy" as const,
              disabled: !project,
              run: () =>
                void navigator.clipboard.writeText(project?.root_path ?? "").catch(() => undefined),
            },
          ]),
      { kind: "rule" },
      {
        kind: "item",
        label: "Remove project…",
        detail: "Stop tracking it here; the dialog says what happens on disk",
        icon: "trash",
        destructive: true,
        disabled: !project,
        // The dialog, never the removal: this is the one item in the menu that
        // can kill a running session and delete a directory, and the policy
        // that decides which is a question only a person can answer.
        run: () => setProjectDialogsStore("projectRemoval", node.id),
      },
    ];
    return sole
      ? [...workspaceMenu(sole), { kind: "rule" }, { kind: "heading", label: "Project" }, ...items]
      : items;
  }

  /** A group's menu. Removing one never touches the project directories. */
  function groupMenu(node: GroupNode): MenuItem[] {
    if (node.id === null) {
      // The implicit group of projects that belong to none: there is nothing
      // to rename or remove, only somewhere to add to.
      return [
        {
          kind: "item",
          label: "Add project…",
          icon: "folder-open",
          run: () => void addProjectFromPicker(null).catch(() => undefined),
        },
      ];
    }
    const id = node.id;
    return [
      {
        kind: "item",
        label: "Add project…",
        icon: "folder-open",
        run: () => void addProjectFromPicker(id).catch(() => undefined),
      },
      { kind: "rule" },
      {
        kind: "item",
        label: "Rename group…",
        icon: "edit",
        run: () => {
          requestTextInput({
            title: "Rename group",
            label: "Group name",
            value: node.name ?? "",
            confirmLabel: "Rename",
            allowEmpty: false,
            onSubmit: (name) => void renameProjectGroup(id, name).catch(() => undefined),
          });
        },
      },
      {
        kind: "item",
        label: "Remove group",
        detail: "The projects stay; only the grouping goes",
        icon: "trash",
        destructive: true,
        run: () => void removeProjectGroup(id).catch(() => undefined),
      },
    ];
  }

  function workspaceMenu(node: WorkspaceNode): MenuItem[] {
    const project = forgeStore.projects.find((item) =>
      forgeStore.workspaces.some((ws) => ws.id === node.id && ws.project_id === item.id),
    );
    const projectPath = project?.root_path ?? node.path;
    const launches = launchItems(node.id);
    return [
      ...launches,
      { kind: "rule" },
      { kind: "heading", label: "Juva & Git" },
      {
        kind: "item",
        label: "Commit with Juva…",
        icon: "agent",
        run: () => {
          focusWorkspace(node.id);
          void draftWithJuva(node.id, "CommitMessage").catch(() => undefined);
        },
      },
      {
        kind: "item",
        label: "Preview PR",
        icon: "git-pull-request",
        run: () => {
          focusWorkspace(node.id);
          openCompare(node.id);
          void loadBranchCompare(node.id).catch(() => undefined);
        },
      },
      {
        kind: "item",
        label: "Open PR with agent…",
        icon: "agent",
        run: () => {
          focusWorkspace(node.id);
          openComposeForWorkspace(node.id);
        },
      },
      { kind: "rule" },
      { kind: "heading", label: "Workspace" },
      {
        kind: "item",
        label: "Shared files…",
        icon: "folder-open",
        disabled: !project,
        run: () => {
          if (project)
            setSharedFiles({ projectId: project.id, workspaceId: node.id, name: node.label });
        },
      },
      newWorktreeItem(project ? { id: project.id, name: project.name } : undefined),
      {
        kind: "item",
        label: "Refresh status",
        icon: "refresh",
        run: () => void refreshWorkspaceStatus(node.id).catch(() => undefined),
      },
      {
        kind: "item",
        label: "Copy path",
        icon: "copy",
        run: () => void navigator.clipboard.writeText(node.path).catch(() => undefined),
      },
      {
        kind: "item",
        label: "Rename…",
        detail: "An empty name falls back to the branch",
        icon: "edit",
        run: () => {
          requestTextInput({
            title: "Rename checkout",
            label: "Checkout name",
            value: node.label,
            confirmLabel: "Rename",
            allowEmpty: true,
            onSubmit: (name) => void renameWorkspace(node.id, name || null).catch(() => undefined),
          });
        },
      },
      ...(isWorktree(node.id)
        ? [
            {
              kind: "item" as const,
              label: "Ignore worktrees in this folder",
              detail: "Forge stops adopting anything under it",
              icon: "filter" as const,
              disabled: !project,
              run: () => {
                if (!project) return;
                // The gesture names the containing folder, not the worktree:
                // the next agent run gets a different directory name.
                const folder =
                  node.path.replace(/\/+$/, "").split("/").slice(0, -1).join("/") || "/";
                const kept = forgeStore.worktree_ignores.filter(
                  (rule) => rule.project_id === project.id && rule.path !== folder,
                );
                void setWorktreeIgnores(project.id, [
                  ...kept,
                  {
                    project_id: project.id,
                    path: folder,
                    scope: "subtree" as const,
                    created_at: new Date().toISOString(),
                  },
                ]).catch(() => undefined);
              },
            },
            {
              kind: "item" as const,
              label: "Remove worktree",
              detail: "Never deletes the branch",
              icon: "trash" as const,
              destructive: true,
              run: () => {
                setNotice(null);
                setProjectDialogsStore("worktreeRemoval", { workspace: node.id, reason: null });
              },
            },
          ]
        : []),
      ...(project
        ? [
            { kind: "rule" as const },
            {
              kind: "submenu" as const,
              label: "Open in",
              icon: "folder-open" as const,
              items: [
                {
                  kind: "item" as const,
                  label: "Zed",
                  glyph: <BrandIcon brand="editor-zed" size={14} class="forge-icon-muted" />,
                  run: () => void openInEditor("zed", projectPath).catch(() => undefined),
                },
                {
                  kind: "item" as const,
                  label: "Cursor",
                  glyph: <BrandIcon brand="cursor" size={14} class="forge-icon-muted" />,
                  run: () => void openInEditor("cursor", projectPath).catch(() => undefined),
                },
                { kind: "rule" as const },
                {
                  kind: "item" as const,
                  label: "Finder",
                  icon: "folder-open" as const,
                  run: () => void openInFileManager(projectPath).catch(() => undefined),
                },
              ],
            },
          ]
        : []),
    ];
  }

  /** Worktrees can be removed; external ones are only forgotten by Forge. */
  function isWorktree(workspace: string): boolean {
    return forgeStore.workspaces.some(
      (item) => item.id === workspace && item.kind === "GitWorktree",
    );
  }

  /*
   * A store rather than a memo: `buildTree` allocates new wrappers every time,
   * and `<For>` keys by identity, so a memo remounted every card on any OSC
   * title change and replayed `ws-list-in`. `applyRailTree` reconciles by `id`.
   */
  const [tree, setTree] = createStore<GroupNode[]>([]);
  createComputed(() => applyRailTree(setTree, buildTree(forgeStore, workspaceOrder(), tabOrder())));

  function persistProjectOrder(projectId: string, ids: string[]): void {
    const next = { ...workspaceOrder(), [projectId]: ids };
    setWorkspaceOrder(next);
    void setAppState(WORKSPACE_ORDER_KEY, JSON.stringify(next)).catch(() => undefined);
  }

  /**
   * Flattened rail rows for keyboard navigation.
   *
   * `Sidebar.tsx:447` declared `role="tree"` and bound nothing: the rail was
   * reachable by Tab and operable by nothing at all. The rows are painted by
   * four nested `For`s, so "the row after this one" is not a sibling — hence a
   * flat list beside the markup rather than a rewrite of it.
   */
  const rows = createMemo(() => railRows(tree, collapsed(), codeShown));
  const [selected, setSelected] = createSignal<string | null>(null);
  const isSelected = createSelector(selected);
  const cursor = createMemo(() => {
    const id = selected();
    const found = rows().findIndex((row) => row.id === id);
    return found < 0 ? 0 : found;
  });

  function selectRow(id: string): void {
    setSelected(id);
    queueMicrotask(() => {
      document.getElementById(railRowId(id))?.scrollIntoView({ block: "nearest" });
    });
  }

  function step(delta: number): void {
    const list = rows();
    if (list.length === 0) return;
    const next = Math.min(Math.max(cursor() + delta, 0), list.length - 1);
    selectRow(list[next].id);
  }

  function activate(): void {
    const row = rows()[cursor()];
    if (!row) return;
    if (row.kind === "code") {
      focusCode(row.target);
      return;
    }
    if (row.kind === "session") {
      // A session row is a request for that terminal, like its tab above.
      focusSession(row.target);
      return;
    }
    toggle(row.target);
  }
  const needsYou = createMemo(() => waitingSessions());

  function toggle(id: string): void {
    folded = true;
    const next = toggleFold(collapsed(), id);
    setCollapsed(next);
    void setAppState(COLLAPSED_KEY, foldsPayload(next)).catch(() => undefined);
  }

  const isOpen = (id: string) => !collapsed().has(id);

  /**
   * The rail claims its bare-letter chords only while it has the keyboard.
   *
   * Entered on focus and not on mount, for the same reason the file tree does
   * it that way: `j`, `k`, `h` and `l` bound for as long as the rail is merely
   * *visible* would take four letters away from every terminal beside it.
   */
  let leaveSidebar: (() => void) | undefined;

  function claimKeyboard(): void {
    leaveSidebar ??= enterContext(SIDEBAR);
  }

  function releaseKeyboard(): void {
    leaveSidebar?.();
    leaveSidebar = undefined;
  }

  onCleanup(releaseKeyboard);

  /*
   * This view stays mounted while hidden — behind another sidebar view or
   * behind Settings — so neither cleanup nor a `focusout` is guaranteed to
   * release SIDEBAR when it goes: not every engine sends one for a focused
   * element that becomes `display: none`, and a live context would hand
   * `MOD-Enter` to this tree instead of the terminal.
   *
   * WKWebView also drops the scroll offset of a `display: none` box, so the
   * offset is kept from scroll events while the view has a height and put back
   * when it has one again. A resize observer sees both ways of hiding.
   */
  let viewEl: HTMLDivElement | undefined;
  let keptScroll = 0;

  onMount(() => {
    if (!viewEl || typeof ResizeObserver === "undefined") return;
    const el = viewEl;
    const observer = new ResizeObserver(() => {
      if (el.clientHeight === 0) releaseKeyboard();
      else if (el.scrollTop !== keptScroll) el.scrollTop = keptScroll;
    });
    observer.observe(el);
    onCleanup(() => observer.disconnect());
  });

  onMount(() => {
    const bound = [
      registerAction("sidebar_next", () => step(1)),
      registerAction("sidebar_previous", () => step(-1)),
      registerAction("sidebar_first", () => rows()[0] && selectRow(rows()[0].id)),
      registerAction("sidebar_last", () => {
        const last = rows().at(-1);
        if (last) selectRow(last.id);
      }),
      registerAction("sidebar_collapse", () => {
        const target = railCollapseTarget(rows()[cursor()]);
        if (!target) return;
        if ("fold" in target) toggle(rows()[cursor()].target);
        else selectRow(target.select);
      }),
      registerAction("sidebar_expand", () => {
        const row = rows()[cursor()];
        const target = railExpandTarget(row, rows(), cursor());
        if (!target) return;
        if ("unfold" in target) toggle(row.target);
        else selectRow(target.select);
      }),
      registerAction("sidebar_open", activate),
      registerAction("sidebar_new_terminal", () => {
        // The checkout a new terminal belongs to: the selected workspace, or
        // the one the selected session already lives in.
        const row = rows()[cursor()];
        if (!row) return;
        const workspace =
          row.kind === "workspace"
            ? row.target
            : row.parent?.startsWith("workspace:")
              ? row.parent.slice("workspace:".length)
              : null;
        if (workspace) void launchShell(workspace).catch(() => undefined);
      }),
    ];
    onCleanup(() => {
      for (const unbind of bound) unbind();
    });
  });

  return (
    <div
      class="projects-view"
      ref={viewEl}
      onScroll={(event) => {
        if (event.currentTarget.clientHeight > 0) keptScroll = event.currentTarget.scrollTop;
      }}
    >
      {/* A question that has been waiting is the one thing that must not need
          scrolling to find, so it is lifted out of the tree and above it. */}
      <Show when={needsYou().length > 0}>
        <section id="needs-you" class="needs-you-block" aria-label="Needs you">
          <For each={needsYou()}>{(session) => <NeedsYouCard session={session} />}</For>
        </section>
      </Show>

      <div class="rail-head">
        <span class="forge-section-label rail-head-label">Projects</span>
        <Menu
          triggerLabel="Add a project or a group"
          triggerClass={buttonClass("xs", "ghost", true)}
          trigger={<Icon name="plus" size={14} class="forge-icon-muted" />}
          items={[
            {
              kind: "item",
              label: "Add a project…",
              icon: "folder-open",
              run: () => void addProjectFromPicker(null).catch(() => undefined),
            },
            {
              kind: "item",
              label: "New group…",
              icon: "folder",
              run: () =>
                requestTextInput({
                  title: "New group",
                  label: "Group name",
                  value: "",
                  confirmLabel: "Create",
                  allowEmpty: false,
                  onSubmit: (name) => void createProjectGroup(name).catch(() => undefined),
                }),
            },
          ]}
        />
      </div>

      <Show when={sharedFiles()}>
        {(target) => (
          <Dialog
            title={`Shared files · ${target().name}`}
            size="lg"
            onDismiss={() => setSharedFiles(null)}
            footer={
              <Button variant="secondary" onClick={() => setSharedFiles(null)}>
                Done
              </Button>
            }
          >
            <SharedFiles projectId={target().projectId} workspaceId={target().workspaceId} />
          </Dialog>
        )}
      </Show>
      <Show when={ignoredWorktrees()}>
        {(target) => (
          <WorktreeIgnoresDialog
            projectId={target().projectId}
            name={target().name}
            onDismiss={() => setIgnoredWorktrees(null)}
          />
        )}
      </Show>
      <Show when={menu()}>
        {(open) => (
          <ContextMenu
            x={open().x}
            y={open().y}
            items={open().items}
            onDismiss={() => setMenu(null)}
          />
        )}
      </Show>

      {/* U4/U5: one tab stop, `aria-activedescendant` naming the row the
          chords act on, and the context entered on focus rather than on mount.
          `focusin`/`focusout` so focus moving between rows is not "leaving". */}
      <div
        role="tree"
        aria-label="Projects, checkouts and sessions"
        aria-activedescendant={rows()[cursor()] ? railRowId(rows()[cursor()].id) : undefined}
        tabIndex={0}
        onFocusIn={claimKeyboard}
        onFocusOut={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) releaseKeyboard();
        }}
      >
        {/* An empty rail was a sentence and nothing else. The one thing there is
          to do here is the button, rather than a line of prose about a menu
          somewhere else. */}
        <For
          each={tree}
          fallback={
            <div class="rail-empty">
              <p class="empty-copy">No projects yet.</p>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => void addProjectFromPicker(null).catch(() => undefined)}
              >
                Add a project
              </Button>
            </div>
          }
        >
          {(group) => (
            <div class="tree-group" role="group">
              <Show when={group.name}>
                {(name) => (
                  <button
                    type="button"
                    id={railRowId(`group:${group.id}`)}
                    class="forge-row tree-row group"
                    role="treeitem"
                    aria-level={1}
                    aria-expanded={isOpen(group.id ?? "")}
                    aria-selected={isSelected(`group:${group.id}`)}
                    classList={{
                      cursor: isSelected(`group:${group.id}`),
                    }}
                    onClick={() => {
                      selectRow(`group:${group.id}`);
                      toggle(group.id ?? "");
                    }}
                    onContextMenu={(event) => openMenu(event, groupMenu(group))}
                  >
                    <Icon
                      name={isOpen(group.id ?? "") ? "folder-open" : "folder"}
                      size={12}
                      class="forge-icon-faint"
                    />
                    <span class="tree-label forge-truncate">{name()}</span>
                    <Show when={group.wantsYou && !isOpen(group.id ?? "")}>
                      <FoldedQuestion />
                    </Show>
                  </button>
                )}
              </Show>
              <Show when={!group.name || isOpen(group.id ?? "")}>
                {/* A group's projects are its children, and the indent is the
                    only thing on screen that says so. Ungrouped projects are
                    roots and stay flush, which is why this is a class and not
                    a rule on `.tree-row.project`. */}
                <div class="tree-children" classList={{ nested: group.name !== null }} role="group">
                  <For each={group.projects}>
                    {(project) => (
                      <>
                        <div class="project-line">
                          <button
                            type="button"
                            id={railRowId(`project:${project.id}`)}
                            class="forge-row tree-row project"
                            role="treeitem"
                            aria-level={group.name === null ? 1 : 2}
                            aria-expanded={isOpen(project.id)}
                            aria-selected={isSelected(`project:${project.id}`)}
                            classList={{
                              cursor: isSelected(`project:${project.id}`),
                            }}
                            onClick={() => {
                              selectRow(`project:${project.id}`);
                              const sole = soleCheckout(project);
                              if (sole) focusWorkspace(sole.id);
                              toggle(project.id);
                            }}
                            onContextMenu={(event) => openMenu(event, projectMenu(project))}
                          >
                            <span
                              class="project-avatar hued"
                              style={{ "--project-hue": projectHue(project.id) }}
                              aria-hidden="true"
                            >
                              {project.icon ?? project.name.slice(0, 1).toUpperCase()}
                            </span>
                            <span class="tree-label forge-truncate">{project.name}</span>
                            <Show
                              when={isOpen(project.id) ? null : projectWork(project)}
                              fallback={
                                <Show when={isOpen(project.id) && soleCheckout(project)}>
                                  {(sole) => (
                                    <span class="tree-meta">{sole().branch ?? sole().label}</span>
                                  )}
                                </Show>
                              }
                            >
                              {(work) => <WorkMarker work={work()} />}
                            </Show>
                          </button>
                          <Show when={soleCheckout(project)}>
                            {(sole) => (
                              <CheckoutMarks
                                workspace={sole()}
                                work={null}
                                onStart={(event) => openMenu(event, projectMenu(project))}
                              />
                            )}
                          </Show>
                        </div>
                        <Show when={isOpen(project.id) && soleCheckout(project)}>
                          {(sole) => (
                            <div
                              class="tree-guide hued"
                              style={{ "--project-hue": projectHue(project.id) }}
                            >
                              <CheckoutChildren
                                workspace={sole()}
                                level={group.name === null ? 2 : 3}
                                selected={selected()}
                                onSessionMenu={(event, session) =>
                                  openMenu(event, sessionMenuItems(session, sessionTitle(session)))
                                }
                              />
                            </div>
                          )}
                        </Show>
                        <Show when={isOpen(project.id) && !soleCheckout(project)}>
                          <div
                            class="tree-guide hued"
                            style={{ "--project-hue": projectHue(project.id) }}
                          >
                            <WorkspaceList
                              workspaces={project.workspaces}
                              level={group.name === null ? 2 : 3}
                              order={workspaceOrder()[project.id] ?? []}
                              onReorder={(ids) => persistProjectOrder(project.id, ids)}
                              isOpen={isOpen}
                              selected={selected()}
                              onToggle={(workspace) => {
                                selectRow(`workspace:${workspace.id}`);
                                // Picking a checkout points the window at it, not
                                // just the rail: the tab strip, the launchers and
                                // the sidebar views are all scoped to the checkout,
                                // and a worktree with no sessions left had no other
                                // way to be selected — the card only folded.
                                focusWorkspace(workspace.id);
                                toggle(workspace.id);
                              }}
                              onMenu={(event, workspace) =>
                                openMenu(event, workspaceMenu(workspace))
                              }
                              onSessionMenu={(event, session) =>
                                openMenu(event, sessionMenuItems(session, sessionTitle(session)))
                              }
                            />
                          </div>
                        </Show>
                      </>
                    )}
                  </For>
                </div>
              </Show>
            </div>
          )}
        </For>
      </div>
    </div>
  );
}

/**
 * The checkouts under one project, in the order the person left them.
 *
 * Pointer capture rather than HTML5 drag: the same path the tab strip uses,
 * so a title-bar app-region never applies and WKWebView actually starts the
 * gesture. Reorder stays inside this project — a worktree cannot be dragged
 * onto another repository.
 */
function WorkspaceList(props: {
  workspaces: WorkspaceNode[];
  /** `aria-level` for a card here: one below the project it hangs off. */
  level: number;
  order: string[];
  onReorder: (ids: string[]) => void;
  isOpen: (id: string) => boolean;
  selected: string | null;
  onToggle: (workspace: WorkspaceNode) => void;
  onMenu: (event: MouseEvent, workspace: WorkspaceNode) => void;
  onSessionMenu: (event: MouseEvent, session: Session) => void;
}) {
  let listEl: HTMLDivElement | undefined;
  const [dragging, setDragging] = createSignal<string | null>(null);
  const [dropTarget, setDropTarget] = createSignal<{ id: string; after: boolean } | null>(null);
  let drag: {
    id: string;
    pointerId: number;
    startX: number;
    startY: number;
    started: boolean;
  } | null = null;
  let lastDragAt = 0;

  function finishDrag(): void {
    drag = null;
    setDragging(null);
    setDropTarget(null);
  }
  onCleanup(finishDrag);

  function cardBoxes(): { top: number; height: number }[] {
    if (!listEl) return [];
    const nodes = listEl.querySelectorAll<HTMLElement>(":scope > .ws-card");
    return Array.from(nodes, (node) => {
      const rect = node.getBoundingClientRect();
      return { top: rect.top, height: rect.height };
    });
  }

  function scrollRailToward(y: number): void {
    const rail = listEl?.closest(".projects-view");
    if (!(rail instanceof HTMLElement)) return;
    const rect = rail.getBoundingClientRect();
    if (y < rect.top + CHECKOUT_SCROLL_EDGE_PX) {
      rail.scrollBy({ top: -16 });
    } else if (y > rect.bottom - CHECKOUT_SCROLL_EDGE_PX) {
      rail.scrollBy({ top: 16 });
    }
  }

  function onHandlePointerDown(workspaceId: string, event: PointerEvent): void {
    if (event.button !== 0) return;
    const target = event.currentTarget;
    if (!(target instanceof HTMLElement)) return;
    drag = {
      id: workspaceId,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      started: false,
    };
    target.setPointerCapture(event.pointerId);
  }

  function onHandlePointerMove(workspaceId: string, event: PointerEvent): void {
    if (!drag || drag.id !== workspaceId || drag.pointerId !== event.pointerId) return;
    if (!drag.started) {
      if (Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < CHECKOUT_DRAG_PX) {
        return;
      }
      drag.started = true;
      setDragging(workspaceId);
    }
    event.preventDefault();
    scrollRailToward(event.clientY);
    const ids = props.workspaces.map((workspace) => workspace.id);
    setDropTarget(dropFromGap(ids, gapAtY(cardBoxes(), event.clientY), workspaceId));
  }

  function onHandlePointerUp(workspaceId: string, event: PointerEvent): void {
    if (!drag || drag.id !== workspaceId || drag.pointerId !== event.pointerId) return;
    if (drag.started) {
      lastDragAt = performance.now();
      const base = orderFromWorkspaces(props.workspaces, props.order);
      props.onReorder(moveTabToGap(base, drag.id, gapAtY(cardBoxes(), event.clientY)));
    }
    finishDrag();
  }

  return (
    <div ref={listEl}>
      <For each={props.workspaces}>
        {(workspace) => (
          <WorkspaceCard
            workspace={workspace}
            level={props.level}
            open={props.isOpen(workspace.id)}
            cursor={props.selected === `workspace:${workspace.id}`}
            cursorCode={props.selected === `code:${workspace.id}`}
            cursorSession={
              props.selected?.startsWith("session:")
                ? props.selected.slice("session:".length)
                : null
            }
            dragging={dragging() === workspace.id}
            dropBefore={dropTarget()?.id === workspace.id && !dropTarget()?.after}
            dropAfter={dropTarget()?.id === workspace.id && dropTarget()?.after === true}
            onToggle={() => {
              if (performance.now() - lastDragAt < 400) return;
              props.onToggle(workspace);
            }}
            onMenu={(event) => props.onMenu(event, workspace)}
            onSessionMenu={props.onSessionMenu}
            onHandlePointerDown={(event) => onHandlePointerDown(workspace.id, event)}
            onHandlePointerMove={(event) => onHandlePointerMove(workspace.id, event)}
            onHandlePointerUp={(event) => onHandlePointerUp(workspace.id, event)}
            onHandlePointerCancel={finishDrag}
          />
        )}
      </For>
    </div>
  );
}

/**
 * One checkout as a tree row: branch, marks and, folded, the loudest state
 * under it — a folded row must never be able to hide a question.
 */
function WorkspaceCard(props: {
  workspace: WorkspaceNode;
  /** Tree depth for a reader; `3` under a grouped project, `2` under a root. */
  level: number;
  open: boolean;
  cursor: boolean;
  cursorCode: boolean;
  /** Which session id the cursor is on, if it is on one inside this card. */
  cursorSession: string | null;
  dragging: boolean;
  dropBefore: boolean;
  dropAfter: boolean;
  onToggle: () => void;
  onMenu: (event: MouseEvent) => void;
  onSessionMenu: (event: MouseEvent, session: Session) => void;
  onHandlePointerDown: (event: PointerEvent) => void;
  onHandlePointerMove: (event: PointerEvent) => void;
  onHandlePointerUp: (event: PointerEvent) => void;
  onHandlePointerCancel: () => void;
}) {
  const work = createMemo(() =>
    rollupWork(
      props.workspace.sessions.map((node) =>
        sessionWork(
          node.session,
          { wants_you: node.wantsYou || hasQuestion(node.session.id) },
          now(),
        ),
      ),
    ),
  );
  const codeActive = () => centerMode() === "code" && activeWorkspace() === props.workspace.id;
  const current = createMemo(
    () =>
      codeActive() ||
      (centerMode() === "session" &&
        props.workspace.sessions.some((node) => node.session.id === connectionStore.activeSession)),
  );

  return (
    <div
      id={railRowId(`workspace:${props.workspace.id}`)}
      class="ws-card"
      role="treeitem"
      aria-level={props.level}
      aria-expanded={props.open}
      aria-selected={props.cursor}
      aria-label={props.workspace.label}
      classList={{
        current: current(),
        open: props.open,
        cursor: props.cursor,
        dragging: props.dragging,
        "drop-before": props.dropBefore,
        "drop-after": props.dropAfter,
      }}
      onContextMenu={props.onMenu}
    >
      <div class="ws-row">
        <button
          type="button"
          class="forge-row ws-head"
          aria-label={`${props.workspace.label}, ${props.open ? "collapse" : "expand"}`}
          onClick={props.onToggle}
          onPointerDown={props.onHandlePointerDown}
          onPointerMove={props.onHandlePointerMove}
          onPointerUp={props.onHandlePointerUp}
          onPointerCancel={props.onHandlePointerCancel}
        >
          <Twisty open={props.open} />
          <Icon
            name={props.workspace.worktree ? "worktree" : "git-branch"}
            size={12}
            class={current() ? "forge-icon-accent" : "forge-icon-muted"}
          />
          <span class="tree-label ws-label forge-truncate">{props.workspace.label}</span>
          <Show when={!props.workspace.worktree}>
            <span class="tree-meta">primary</span>
          </Show>
        </button>
        <CheckoutMarks
          workspace={props.workspace}
          work={props.open ? null : work()}
          onStart={props.onMenu}
        />
      </div>

      <Show when={props.open}>
        <CheckoutChildren
          workspace={props.workspace}
          level={props.level + 1}
          selected={
            props.cursorCode
              ? `code:${props.workspace.id}`
              : props.cursorSession
                ? `session:${props.cursorSession}`
                : null
          }
          onSessionMenu={props.onSessionMenu}
        />
      </Show>
    </div>
  );
}

/** A checkout's trailing marks: work, changes, sync, pull request and its launcher. */
function CheckoutMarks(props: {
  workspace: WorkspaceNode;
  work: ReturnType<typeof rollupWork>;
  onStart: (event: MouseEvent) => void;
}) {
  const sync = createMemo(() => syncLabel(props.workspace.ahead, props.workspace.behind));
  const changed = () =>
    gitStore.diff?.workspace_id === props.workspace.id ? gitStore.diff.files.length : null;
  return (
    <span class="ws-marks">
      {/* Provisioning acks when it starts, so a checkout can be open
          before its shared files have landed. */}
      <Show when={sharesStore.applying.includes(props.workspace.id)}>
        <Badge>setting up…</Badge>
      </Show>
      <Show when={props.work}>{(state) => <WorkMarker work={state()} />}</Show>
      {/* `measured_at: None` means "not measured", which is not the same as
          clean — so an unmeasured checkout shows no mark at all. */}
      <Show when={props.workspace.measured && props.workspace.dirty}>
        <Tooltip label="Uncommitted changes" contents>
          <span
            class="ws-dirty"
            role="img"
            aria-label={
              changed() !== null ? `${changed()} uncommitted changes` : "Uncommitted changes"
            }
          >
            ●{changed() ?? ""}
          </span>
        </Tooltip>
      </Show>
      <Show when={sync()}>
        {(label) => (
          <span class="ws-sync" aria-label="Ahead of or behind the remote">
            {label()}
          </span>
        )}
      </Show>
      <Show when={props.workspace.pullRequest}>
        {(pr) => (
          <Tooltip label={prLabel(pr())} contents>
            <Icon name="git-pull-request" size={13} class={PR_ICON_CLASS[prTone(pr())]} />
          </Tooltip>
        )}
      </Show>
      <IconButton label="Start something here" size="xs" class="ws-start" onClick={props.onStart}>
        <Icon name="plus" size={13} />
      </IconButton>
    </span>
  );
}

/** What a checkout holds: its Code row while files are open, then its sessions. */
function CheckoutChildren(props: {
  workspace: WorkspaceNode;
  level: number;
  selected: string | null;
  onSessionMenu: (event: MouseEvent, session: Session) => void;
}) {
  const codeActive = () => centerMode() === "code" && activeWorkspace() === props.workspace.id;
  return (
    <div class="ws-sessions">
      <Show when={codeShown(props.workspace.id)}>
        <button
          type="button"
          id={railRowId(`code:${props.workspace.id}`)}
          class="forge-row tree-row"
          role="treeitem"
          aria-level={props.level}
          aria-selected={props.selected === `code:${props.workspace.id}` || codeActive()}
          aria-current={codeActive() || undefined}
          classList={{ cursor: props.selected === `code:${props.workspace.id}` }}
          onClick={() => focusCode(props.workspace.id)}
          onContextMenu={(event) => event.stopPropagation()}
        >
          <Icon
            name="file-code"
            size={14}
            class={codeActive() ? "forge-icon-accent" : "forge-icon-muted"}
          />
          <span class="tree-label forge-truncate">Code</span>
          <Show when={filesOpen(props.workspace.id) > 0}>
            <span class="tree-meta">{filesOpen(props.workspace.id)} open</span>
          </Show>
        </button>
      </Show>
      <For each={props.workspace.sessions}>
        {(node) => (
          <SessionRow
            session={node.session}
            label={node.label}
            wantsYou={node.wantsYou || hasQuestion(node.session.id)}
            unread={node.unread}
            inTree
            cursor={props.selected === `session:${node.session.id}`}
            depth={node.depth}
            level={props.level}
            onMenu={(event) => props.onSessionMenu(event, node.session)}
          />
        )}
      </For>
    </div>
  );
}

/** A waiting session lifted above the tree: what it is, where, and for how long. */
function NeedsYouCard(props: { session: Session }) {
  const place = createMemo(() => {
    const workspace = forgeStore.workspaces.find((item) => item.id === props.session.workspace_id);
    const project = forgeStore.projects.find((item) => item.id === workspace?.project_id);
    return project?.name ?? workspace?.branch ?? "";
  });
  const label = () => sessionDisplayTitle(props.session);
  return (
    <button
      type="button"
      class="needs-you-card"
      aria-label={`${label()}, waiting on you for ${waitedFor(props.session, now())}`}
      onClick={() => focusSession(props.session.id)}
    >
      <span class="needs-you-line">
        <span class="forge-attention-dot" aria-hidden="true" />
        <span class="needs-you-title forge-truncate">{label()}</span>
        <span class="needs-you-waited">{waitedFor(props.session, now())}</span>
      </span>
      <span class="needs-you-place forge-truncate">{place()} · waiting on you</span>
    </button>
  );
}

function SessionRow(props: {
  session: Session;
  /** Pre-computed so the rail and the feature tab name a session identically. */
  label: string;
  wantsYou: boolean;
  unread: boolean;
  /** Indent, in rungs of the card's own left edge. */
  depth: number;
  /** Tree level for a reader, which the indent no longer implies. */
  level?: number;
  /** Whether this row sits inside the rail's tree, or was lifted out of it. */
  inTree?: boolean;
  /** The rail's keyboard cursor is on this row. */
  cursor?: boolean;
  onMenu?: (event: MouseEvent) => void;
}) {
  const active = () =>
    centerMode() === "session" && props.session.id === connectionStore.activeSession;
  const waited = () => waitedFor(props.session, now());
  return (
    <button
      type="button"
      id={props.inTree ? railRowId(`session:${props.session.id}`) : undefined}
      class="forge-row tree-row"
      // The needs-you block is deliberately *not* inside the tree — it is a
      // lift of rows out of it — so those rows carry no tree role: an orphan
      // `treeitem` is worse for a screen reader than a plain button.
      role={props.inTree ? "treeitem" : undefined}
      // A child sits under the session that started it, and the indent says so;
      // `level` places that same nesting under the card for a screen reader.
      aria-level={props.inTree ? (props.level ?? 1) + props.depth : undefined}
      aria-selected={props.inTree ? (props.cursor ?? active()) : undefined}
      aria-label={props.wantsYou ? `${props.label}, waiting on you for ${waited()}` : undefined}
      aria-current={active() || undefined}
      classList={{
        unread: props.unread && !active(),
        cursor: props.cursor === true,
      }}
      style={{ "--tree-depth": String(props.depth) }}
      onClick={() => focusSession(props.session.id)}
      onContextMenu={props.onMenu}
    >
      {/* What it runs, not just that it exists: a terminal and a Codex
          agent in the same checkout are unreadable as two identical labels. */}
      <SessionGlyph
        providerId={props.session.agent_provider_id}
        session={props.session}
        emphasis={active() || props.wantsYou ? "full" : "dim"}
      />
      <SessionLabel label={props.label} />
      <Show when={props.wantsYou} fallback={<StateMarker session={props.session} />}>
        <AttentionMarker />
      </Show>
    </button>
  );
}

/** A shell's `Shell ~/path` title sets its path in the code face, so names line up. */
function SessionLabel(props: { label: string }) {
  const split = () => {
    const at = props.label.indexOf(" ~/");
    return at > 0 ? [props.label.slice(0, at), props.label.slice(at + 1)] : null;
  };
  return (
    <Show when={split()} fallback={<span class="tree-label forge-truncate">{props.label}</span>}>
      {(parts) => (
        <span class="tree-label forge-truncate">
          {parts()[0]} <span class="tree-path">{parts()[1]}</span>
        </span>
      )}
    </Show>
  );
}

function Twisty(props: { open: boolean }) {
  return (
    <span class="tree-twisty" classList={{ open: props.open }} aria-hidden="true">
      <Icon name="chevron-right" size={11} />
    </span>
  );
}

function FoldedQuestion() {
  return (
    <span class="forge-attention-dot" role="img" aria-label="A session inside is waiting on you" />
  );
}

/**
 * The folded rows from the last snapshot.
 *
 * Read at creation and again once the snapshot lands, then never: the daemon
 * is authoritative for the first paint, but a snapshot arriving mid-session
 * must not re-fold a row the user just opened.
 */
function readCollapsed(): Set<string> {
  return parseFolds(forgeStore.app_state[COLLAPSED_KEY]);
}

function readWorkspaceOrder(): WorkspaceOrderMap {
  return parseWorkspaceOrder(forgeStore.app_state[WORKSPACE_ORDER_KEY]);
}
