import { For, Show, createComputed, createMemo, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { applyRailTree } from "./railTree";
import { SIDEBAR } from "../actions/actions";
import { enterContext, registerAction } from "../actions/dispatch";
import { onCleanup, onMount } from "solid-js";
import {
  addProjectFromPicker,
  createProjectGroup,
  moveProject,
  openInEditor,
  openInFileManager,
  refreshProject,
  refreshWorkspaceStatus,
  removeProjectGroup,
  renameProjectGroup,
  renameWorkspace,
  setProjectIcon,
} from "../runtime/api";
import { sessionIsAgent, sessionTitle, type Session } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { requestTextInput, runtimeStore, setRuntimeStore } from "../store/runtimeStore";
import { now } from "../runtime/clock";
import { sessionWork } from "../runtime/work";
import { sessionAttention } from "../runtime/attention";
import { setAppState } from "../runtime/api";
import { draftWithJuva } from "../workbench/api";
import { openComposeForWorkspace } from "../workbench/PrComposeView";
import { focusWorkspace } from "../store/workbenchStore";
import { focusSession, launchAgent, launchShell } from "./sessionActions";
import { Button, ContextMenu, Dialog, IconButton, Tooltip, type MenuItem } from "../ui";
import {
  buildTree,
  railCollapseTarget,
  railExpandTarget,
  railRows,
  waiting,
  type GroupNode,
  type ProjectNode,
  type SessionNode,
  type WorkspaceNode,
} from "./tree";
import { chipGroups, prLabel, prTone, rollupWork, syncLabel, type PrTone } from "./workspaceCard";
import { workspaceBranchMeta } from "./workspaceLabel";
import { seedFromAppState } from "./layout";
import { foldsPayload, parseFolds, toggleFold } from "./railFolds";
import { newWorktreeItem } from "./railMenuItems";
import { sessionMenuItems } from "./sessionMenuItems";
import { dropFromGap, gapAtY, moveTabToGap } from "./tabOrder";
import { SharedFiles } from "../settings/SharedFiles";
import { sharesStore } from "../store/sharesStore";
import { orderFromWorkspaces, parseWorkspaceOrder, type WorkspaceOrderMap } from "./workspaceOrder";

/** A DOM id for a rail row, so `aria-activedescendant` can name it. */
function railRowId(id: string): string {
  return `rail-${encodeURIComponent(id)}`;
}
import { sessionDisplayTitle } from "./sessionTree";
import { waitedFor } from "../runtime/attention";
import {
  AttentionMarker,
  BrandIcon,
  Icon,
  SessionGlyph,
  StateMarker,
  WorkMarker,
} from "../theme/icons";
import type { TextInputRequest } from "./TextInputDialog";

const COLLAPSED_KEY = "ui.sidebar.collapsed";
const WORKSPACE_ORDER_KEY = "ui.sidebar.workspace_order";
const CHECKOUT_DRAG_PX = 4;
const CHECKOUT_SCROLL_EDGE_PX = 40;

/** The palette a review decision paints the card's pull-request mark. */
const PR_ICON_CLASS: Record<PrTone, string> = {
  neutral: "forge-icon-muted",
  good: "forge-icon-green",
  warn: "forge-icon-amber",
  bad: "forge-icon-red",
};

type SidebarProps = {};

/**
 * The project rail: group → project → workspace → session.
 *
 * Folding is persisted through the daemon as one blob rather than a key per
 * row: the set is small, and a row that appears later must not arrive folded
 * because some earlier build wrote a key for it.
 */
export function Sidebar(props: SidebarProps) {
  const [sharedFiles, setSharedFiles] = createSignal<{
    projectId: string;
    workspaceId?: string;
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

  /** What a launcher row offers, mirroring the `+` menu's own list. */
  function launchItems(workspace: string): MenuItem[] {
    return forgeStore.launchables.map((launchable) => ({
      kind: "item",
      label: launchable.kind === "shell" ? "New Terminal" : `New ${launchable.label}`,
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
    }));
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
    return [
      newWorktreeItem({ id: node.id, name: node.name }),
      {
        kind: "item",
        label: "Shared files…",
        icon: "folder-open",
        run: () => setSharedFiles({ projectId: node.id, name: node.name }),
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
      { kind: "rule" },
      {
        kind: "item",
        label: "Copy path",
        icon: "copy",
        disabled: !project,
        run: () =>
          void navigator.clipboard.writeText(project?.root_path ?? "").catch(() => undefined),
      },
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
        run: () => setRuntimeStore("projectRemoval", node.id),
      },
    ];
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
    const startItems: MenuItem[] = launches.length
      ? [{ kind: "heading", label: "Start" }, ...launches, { kind: "rule" }]
      : [];
    return [
      ...startItems,
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
        label: "Open PR with Juva…",
        icon: "git-pull-request",
        run: () => {
          focusWorkspace(node.id);
          void draftWithJuva(node.id, "PullRequest").catch(() => undefined);
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
              label: "Remove worktree",
              detail: "Never deletes the branch",
              icon: "trash" as const,
              destructive: true,
              run: () => {
                setRuntimeStore("notice", null);
                setRuntimeStore("worktreeRemoval", { workspace: node.id, reason: null });
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
  createComputed(() => applyRailTree(setTree, buildTree(forgeStore, workspaceOrder())));

  function persistProjectOrder(projectId: string, ids: string[]): void {
    const next = { ...workspaceOrder(), [projectId]: ids };
    setWorkspaceOrder(next);
    void setAppState(WORKSPACE_ORDER_KEY, JSON.stringify(next)).catch(() => undefined);
  }

  /**
   * The rail, flat, for the keyboard (§4.1 U4).
   *
   * `Sidebar.tsx:447` declared `role="tree"` and bound nothing: the rail was
   * reachable by Tab and operable by nothing at all. The rows are painted by
   * four nested `For`s, so "the row after this one" is not a sibling — hence a
   * flat list beside the markup rather than a rewrite of it.
   */
  const rows = createMemo(() => railRows(tree, collapsed()));
  const [selected, setSelected] = createSignal<string | null>(null);
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

  /** `Enter` on a row: a session is selected, everything else folds or opens. */
  function activate(): void {
    const row = rows()[cursor()];
    if (!row) return;
    if (row.kind === "session") {
      // A session row is a request for that terminal, like its tab above.
      focusSession(row.target);
      return;
    }
    toggle(row.target);
  }
  const needsYou = createMemo(() => waiting(forgeStore));

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
    <aside class="sidebar" aria-label="Projects">
      <div class="rail-head">
        <span class="section-label">Projects</span>
        <IconButton
          label="Add a project"
          size="xs"
          class="tree-add"
          onClick={() => void addProjectFromPicker(null).catch(() => undefined)}
        >
          +
        </IconButton>
        <IconButton
          label="New group"
          size="xs"
          class="tree-add"
          onClick={() =>
            requestTextInput({
              title: "New group",
              label: "Group name",
              value: "",
              confirmLabel: "Create",
              allowEmpty: false,
              onSubmit: (name) => void createProjectGroup(name).catch(() => undefined),
            })
          }
        >
          ⊞
        </IconButton>
      </div>

      {/* A question that has been waiting is the one thing that must not need
          scrolling to find, so it is lifted out of the tree entirely. */}
      <Show when={needsYou().length > 0}>
        <div class="needs-you-block">
          <div class="needs-you">
            <span class="section-label">Needs you</span>
            <span class="needs-you-count">{needsYou().length}</span>
          </div>
          <For each={needsYou()}>
            {(session) => (
              <SessionRow
                session={session}
                label={sessionDisplayTitle(session)}
                wantsYou
                unread={false}
                depth={1}
                showDuration
              />
            )}
          </For>
        </div>
      </Show>

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
                    aria-selected={selected() === `group:${group.id}`}
                    classList={{
                      "wants-you": group.wantsYou && !isOpen(group.id ?? ""),
                      cursor: selected() === `group:${group.id}`,
                    }}
                    onClick={() => {
                      selectRow(`group:${group.id}`);
                      toggle(group.id ?? "");
                    }}
                    onContextMenu={(event) => openMenu(event, groupMenu(group))}
                  >
                    <Twisty open={isOpen(group.id ?? "")} />
                    <Icon
                      name={isOpen(group.id ?? "") ? "folder-open" : "folder"}
                      size={13}
                      class="forge-icon-faint"
                    />
                    <span class="tree-label">{name()}</span>
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
                        <button
                          type="button"
                          id={railRowId(`project:${project.id}`)}
                          class="forge-row tree-row project"
                          role="treeitem"
                          aria-level={group.name === null ? 1 : 2}
                          aria-expanded={isOpen(project.id)}
                          aria-selected={selected() === `project:${project.id}`}
                          classList={{
                            "wants-you": project.wantsYou && !isOpen(project.id),
                            cursor: selected() === `project:${project.id}`,
                          }}
                          onClick={() => {
                            selectRow(`project:${project.id}`);
                            toggle(project.id);
                          }}
                          onContextMenu={(event) => openMenu(event, projectMenu(project))}
                        >
                          <Twisty open={isOpen(project.id)} />
                          <span class="project-avatar" aria-hidden="true">
                            {project.icon ?? project.name.slice(0, 1).toUpperCase()}
                          </span>
                          <span class="tree-label">{project.name}</span>
                          {/* Folded, the count is the only thing left saying the
                            project has more than one checkout. */}
                          <Show when={!isOpen(project.id) && project.workspaces.length > 0}>
                            <span class="tree-note">{project.workspaces.length}</span>
                          </Show>
                        </button>
                        <Show when={isOpen(project.id)}>
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
                              // the inspector are all scoped to the checkout, and
                              // a worktree with no sessions left had no other way
                              // to be selected — the card only folded.
                              focusWorkspace(workspace.id);
                              toggle(workspace.id);
                            }}
                            onMenu={(event, workspace) => openMenu(event, workspaceMenu(workspace))}
                            onSessionMenu={(event, session) =>
                              openMenu(event, sessionMenuItems(session, sessionTitle(session)))
                            }
                          />
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
    </aside>
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
    const rail = listEl?.closest(".sidebar");
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
    <div class="ws-list" ref={listEl}>
      <For each={props.workspaces}>
        {(workspace) => (
          <WorkspaceCard
            workspace={workspace}
            level={props.level}
            open={props.isOpen(workspace.id)}
            cursor={props.selected === `workspace:${workspace.id}`}
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
 * One checkout, drawn as a card rather than a row.
 *
 * A rail of identical rows could say a worktree existed and nothing else. The
 * card answers the three questions that actually decide where the user clicks:
 * is anything running here (the marker and, when folded, the chips), is the tree
 * clean and pushed (the meta line), and is there a pull request open on it.
 */
function WorkspaceCard(props: {
  workspace: WorkspaceNode;
  /** Tree depth for a reader; `3` under a grouped project, `2` under a root. */
  level: number;
  open: boolean;
  /** The rail's keyboard cursor is on this card (§4.1 U4). */
  cursor: boolean;
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
  const chips = createMemo(() => chipGroups(props.workspace.sessions));
  // One marker for what can be five sessions: the loudest state wins, so a
  // folded card can never hide a question.
  const work = createMemo(() =>
    rollupWork(
      props.workspace.sessions.map((node) =>
        sessionWork(node.session, sessionAttention(node.session.id), now()),
      ),
    ),
  );
  const active = createMemo(() =>
    props.workspace.sessions.some((node) => node.session.id === runtimeStore.activeSession),
  );
  const sync = createMemo(() => syncLabel(props.workspace.ahead, props.workspace.behind));

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
        active: active(),
        open: props.open,
        "wants-you": props.workspace.wantsYou,
        cursor: props.cursor,
        dragging: props.dragging,
        "drop-before": props.dropBefore,
        "drop-after": props.dropAfter,
      }}
      onContextMenu={props.onMenu}
    >
      <button
        type="button"
        class="forge-row ws-head"
        onClick={props.onToggle}
        onPointerDown={props.onHandlePointerDown}
        onPointerMove={props.onHandlePointerMove}
        onPointerUp={props.onHandlePointerUp}
        onPointerCancel={props.onHandlePointerCancel}
      >
        <Show when={work()} fallback={<span class="ws-quiet-dot" aria-hidden="true" />}>
          {(state) => <WorkMarker work={state()} />}
        </Show>
        <span class="tree-label">{props.workspace.label}</span>
        <Show when={!props.workspace.worktree}>
          <span class="ws-badge">primary</span>
        </Show>
        {/* Provisioning acks when it starts (§14.2), so a checkout can be open
            before its shared files have landed. */}
        <Show when={sharesStore.applying.includes(props.workspace.id)}>
          <span class="ws-badge">setting up…</span>
        </Show>
      </button>

      <div class="ws-meta">
        <Tooltip label={props.workspace.path} contents>
          <span class="ws-branch">{workspaceBranchMeta(props.workspace)}</span>
        </Tooltip>
        <span class="ws-marks">
          {/* `measured_at: None` means "not measured", which is not the same as
              clean — so an unmeasured checkout shows no mark at all. */}
          <Show when={props.workspace.measured && props.workspace.dirty}>
            <Tooltip label="Uncommitted changes" contents>
              <span class="ws-dirty" role="img" aria-label="Uncommitted changes">
                ●
              </span>
            </Tooltip>
          </Show>
          <Show when={sync()}>
            {(label) => (
              <Tooltip label="Ahead of / behind the remote" contents>
                <span class="ws-sync">{label()}</span>
              </Tooltip>
            )}
          </Show>
          <Show when={props.workspace.worktree}>
            <Tooltip label="Git worktree" contents>
              <Icon name="git-branch" size={13} class="forge-icon-dim" />
            </Tooltip>
          </Show>
          <Show when={props.workspace.pullRequest}>
            {(pr) => (
              <Tooltip label={prLabel(pr())} contents>
                <Icon name="git-pull-request" size={13} class={PR_ICON_CLASS[prTone(pr())]} />
              </Tooltip>
            )}
          </Show>
        </span>
        <span class="ws-meta-actions">
          <IconButton
            label="Start something here"
            size="xs"
            class="tree-add"
            onClick={props.onMenu}
          >
            +
          </IconButton>
          <Show when={props.workspace.sessions.length > 0}>
            <button
              type="button"
              class="forge-row ws-expand"
              aria-label={props.open ? "Collapse this checkout" : "Expand this checkout"}
              onClick={props.onToggle}
            >
              <Twisty open={props.open} />
            </button>
          </Show>
        </span>
      </div>

      <Show when={!props.open}>
        <div class="ws-chips">
          <Show
            when={props.workspace.sessions.length > 0}
            fallback={<span class="ws-quiet">Nothing running</span>}
          >
            <For each={[chips().agents, chips().shells]}>
              {(group) => (
                <Show when={group.length > 0}>
                  <div class="ws-chip-group">
                    <For each={group}>
                      {(node) => (
                        <SessionChip
                          node={node}
                          onMenu={(event) => props.onSessionMenu(event, node.session)}
                        />
                      )}
                    </For>
                  </div>
                </Show>
              )}
            </For>
          </Show>
        </div>
      </Show>

      <Show when={props.open && props.workspace.sessions.length > 0}>
        <div class="ws-sessions">
          <For each={props.workspace.sessions}>
            {(node) => (
              <SessionRow
                session={node.session}
                label={node.label}
                wantsYou={node.wantsYou}
                unread={node.unread}
                inTree
                cursor={props.cursorSession === node.session.id}
                depth={node.depth}
                level={props.level + 1}
                onMenu={(event) => props.onSessionMenu(event, node.session)}
              />
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

/**
 * A session inside a card's chip strip.
 *
 * Its own state marker, not the card's: the roll-up says the loudest thing
 * happening here, and the chips are what a click has to distinguish between.
 */
function SessionChip(props: { node: SessionNode; onMenu: (event: MouseEvent) => void }) {
  const active = () => props.node.session.id === runtimeStore.activeSession;
  return (
    <Tooltip label={props.node.label} contents>
      <button
        type="button"
        class="forge-row ws-chip"
        classList={{
          active: active(),
          "wants-you": props.node.wantsYou,
          unread: props.node.unread && !active(),
        }}
        aria-label={props.node.label}
        onClick={() => focusSession(props.node.session.id)}
        onContextMenu={props.onMenu}
      >
        <StateMarker session={props.node.session} />
        <SessionGlyph
          providerId={props.node.session.agent_provider_id}
          session={props.node.session}
          size={13}
          emphasis={active() || props.node.wantsYou ? "full" : "dim"}
        />
      </button>
    </Tooltip>
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
  showDuration?: boolean;
  onMenu?: (event: MouseEvent) => void;
}) {
  const active = () => props.session.id === runtimeStore.activeSession;
  return (
    <button
      type="button"
      id={props.inTree ? railRowId(`session:${props.session.id}`) : undefined}
      class="forge-row tree-row session"
      // The needs-you block is deliberately *not* inside the tree — it is a
      // lift of rows out of it — so those rows carry no tree role: an orphan
      // `treeitem` is worse for a screen reader than a plain button.
      role={props.inTree ? "treeitem" : undefined}
      // A harness step is a step of the row above it, and the indent says so;
      // `level` places that same nesting under the card for a screen reader.
      aria-level={props.inTree ? (props.level ?? 1) + props.depth : undefined}
      aria-selected={props.inTree ? (props.cursor ?? active()) : undefined}
      classList={{
        active: active(),
        agent: sessionIsAgent(props.session),
        "wants-you": props.wantsYou,
        unread: props.unread && !active(),
        cursor: props.cursor === true,
      }}
      style={{ "--depth": String(props.depth) }}
      onClick={() => focusSession(props.session.id)}
      onContextMenu={props.onMenu}
    >
      <Show when={props.wantsYou} fallback={<StateMarker session={props.session} />}>
        <AttentionMarker />
      </Show>
      {/* What it runs, not just that it exists: a terminal and a Codex
          agent in the same checkout are unreadable as two identical labels. */}
      <SessionGlyph
        providerId={props.session.agent_provider_id}
        session={props.session}
        emphasis={active() || props.wantsYou ? "full" : "dim"}
      />
      <span class="tree-label">{props.label}</span>
      <Show when={props.showDuration && props.wantsYou}>
        <span class="tree-note">{waitedFor(props.session)}</span>
      </Show>
    </button>
  );
}

function Twisty(props: { open: boolean }) {
  return (
    <span class="tree-twisty" classList={{ open: props.open }} aria-hidden="true">
      ›
    </span>
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
