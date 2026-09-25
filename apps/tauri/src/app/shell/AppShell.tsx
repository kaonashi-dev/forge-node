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
import { APP } from "../../actions/actions";
import { enterContext, installKeymap, invokeAction, registerAction } from "../../actions/dispatch";
import { CommandPalette } from "../palette/CommandPalette";
import type { PaletteEntry, PaletteScope } from "../palette/entries";
import { addProjectFromPicker } from "../../features/projects/commands";
import { closeSession } from "../../features/sessions/commands";
import { setAppState } from "../../features/settings/commands";
import { scrollTerminal } from "../../features/terminal/commands";
import { focusedTerminalId } from "../../features/terminal/focus";
import { startAppRuntime } from "../lifecycle/index";
import { startGitSync } from "../../features/git/gitSync";
import { startCheckoutWatch } from "../integrations/checkoutWatch";
import { forgeStore } from "../../state/forgeStore";
import { dialogsStore, requestConfirm, setDialogsStore } from "../../state/dialogs";
import { connectionStore, setNotice } from "../../state/connection";
import {
  projectDialogsStore,
  requestNewWorktree,
  setProjectDialogsStore,
} from "../../features/projects/dialogs";
import { sessionDialogsStore, setSessionDialogsStore } from "../../features/sessions/dialogs";
import { listenForUpdates, requestUpdateCheck } from "../../features/settings/updateStore";
import { sessionIsActive, sessionTitle } from "../../contracts/runtime";
import { terminalStore } from "../../features/terminal/terminalStore";
import {
  centerMode,
  codeOpen,
  currentViews,
  focus as focusCodeView,
  reopenClosed,
  requestFindInFiles,
  showCode,
  showSession,
  stepCodeView,
} from "../../navigation/viewsStore";
import {
  close as closeView,
  closeCode,
  closeOtherViews,
  closeViewsToRight,
} from "../../features/editor/tabs";
import { centerSplit } from "../../navigation/centerSplitStore";
import { openEditor } from "../../features/editor/open";
import {
  DENSITY_KEY,
  REDUCE_MOTION_KEY,
  EDITOR_FONT_SIZE_KEY,
  EDITOR_FONT_SIZE_RANGE,
  SIDEBAR_RANGE,
  SIDEBAR_WIDTH_KEY,
  THEME_BASE_KEY,
  UI_FONT_SIZE_KEY,
  bumpScale,
  readChoice,
  readScale,
  readWidth,
  resetScale,
  writeWidth,
} from "../../state/preferences";
import { CenterStack } from "./CenterStack";
import type { Section } from "../../features/settings/SettingsRoute";
import { ESC_AGAIN_MS, isSecondEsc } from "../../features/settings/escAgain";
import { defaultAgentFrom, resolveDefaultAgent } from "../../features/settings/defaultAgent";
import { applyThemeBase, type ThemePreference } from "../../theme/ThemeProvider";
import { applyDensity, readDensity } from "../../theme/density";
import { applyReduceMotion } from "../../theme/motion";
import { UI_FONT_SIZE_RANGE, applyUiFont } from "../../theme/uiFont";
import { ResizeHandle } from "./ResizeHandle";
import { Sidebar } from "./Sidebar";
import { TitleBar } from "./TitleBar";
import { BranchPicker } from "../../features/projects/BranchPicker";
import { JuvaDraftDialog } from "../../features/git/JuvaDraftDialog";
import { ConfirmDialog } from "./dialogs/ConfirmDialog";
import { RemoveProjectDialog } from "../../features/projects/RemoveProjectDialog";
import { RemoveWorktreeDialog } from "../../features/projects/RemoveWorktreeDialog";
import { TextInputDialog } from "./dialogs/TextInputDialog";
import { HandoffDialog } from "../../features/sessions/HandoffDialog";
import { HandoffProgressDialog } from "../../features/sessions/HandoffProgressDialog";
import { SendContextDialog } from "../../features/sessions/SendContextDialog";
import { SpawnChildDialog } from "../../features/sessions/SpawnChildDialog";
import { TabSwitcher } from "./tabs/SwitchTab";
import { trackQuestions } from "../../features/sessions/waiting";
import { liveIdsByActivity } from "../../navigation/tabMru";
import type { SwitchTarget } from "../../navigation/tabTargets";
import { activeSwitcherKey, switcherTargets, trackTabFocus } from "../../navigation/switcherRing";
import {
  bindTabSwitcherCommit,
  stepTabSwitcher,
  tabSwitcherView,
} from "../../navigation/tabSwitcher";
import {
  currentWorkspace as sharedWorkspace,
  focusSession,
  joinPanes,
  launchAgent,
  launchShell,
  splitPane,
  restoreWorkspace,
  openCheckoutReview,
  startHandoff,
  startSendContext,
  startSpawnChild,
  toggleSessionChanges,
} from "../../features/sessions/sessionActions";
import { listen } from "@tauri-apps/api/event";
import {
  openSessions as orderedOpenSessions,
  stripIndex,
  stripItems,
  type StripItem,
} from "../../navigation/tabOrder";
import {
  parseTabOrder,
  projectOfWorkspace,
  sessionsInWorkspace,
  type TabOrderMap,
} from "../../features/sessions/sessionScope";
import { closeTarget } from "./tabs/closeTarget";
import {
  cycleSidebarView,
  restoreSidebar,
  sidebarOpen,
  toggleSidebar,
  toggleView,
} from "../../navigation/sidebarStore";
import { IconButton, ToastRegion } from "../../ui/index";
import { openFile, warmFileTree as warmTree } from "../../features/files/commands";
import { gitStore, setGitStore } from "../../features/git/state";
import { activeWorkspace, focusWorkspace } from "../../state/workspace";

const TAB_ORDER_KEY = "ui.tab_order";

const NOTICE_DISMISS_MS = 6000;

export function AppShell() {
  const [sidebarWidth, setSidebarWidth] = createSignal(SIDEBAR_RANGE.fallback);
  const [palette, setPalette] = createSignal<PaletteScope | null>(null);
  const [settings, setSettings] = createSignal(false);
  const [settingsSection, setSettingsSection] = createSignal<Section | undefined>();
  const [settingsEscArmed, setSettingsEscArmed] = createSignal(false);
  const [tabOrder, setTabOrder] = createSignal<TabOrderMap>(readTabOrder());

  // The daemon answers with the stored layout some milliseconds after the
  // window is already on screen. Seeding once — rather than tracking — keeps a
  // later snapshot from yanking a panel the user has since dragged.
  let seeded = false;
  createEffect(() => {
    if (seeded || Object.keys(forgeStore.app_state).length === 0) return;
    seeded = true;
    restoreSidebar();
    setSidebarWidth(readWidth(SIDEBAR_WIDTH_KEY, SIDEBAR_RANGE));
    setTabOrder(readTabOrder());
    const base = forgeStore.app_state[THEME_BASE_KEY] as ThemePreference | undefined;
    if (base) applyThemeBase(base);
    // Density is written over `tokens.css` as an inline style, so it has to be
    // re-applied whenever the stored preference lands — the same round trip
    // the theme takes.
    applyDensity(readDensity(forgeStore.app_state[DENSITY_KEY]));
    applyReduceMotion(forgeStore.app_state[REDUCE_MOTION_KEY] === "true");
    applyUiFont(
      readScale(
        UI_FONT_SIZE_KEY,
        UI_FONT_SIZE_RANGE.min,
        UI_FONT_SIZE_RANGE.max,
        UI_FONT_SIZE_RANGE.fallback,
      ),
    );
  });

  /*
   * The active session says which checkout the sidebar views answer about.
   * Switching sessions must move them rather than leave the previous
   * checkout's diff under a new branch name.
   *
   * `on`, tracking the session's workspace alone. As a bare effect this also
   * depended on what it wrote: `focusWorkspace` reads `activeWorkspace()`
   * to decide whether anything changed, so anyone else focusing a checkout — the
   * palette, the file palette, the Git panel with no session open — woke this
   * effect, which saw no active session and immediately cleared it again.
   *
   * Do not guess from the workspace list while the runtime is still settling:
   * that list may contain retained worktrees whose directories no longer exist,
   * so the active session stays the authoritative selection.
   */
  createEffect(
    on(
      () =>
        forgeStore.sessions.find((item) => item.id === connectionStore.activeSession)?.workspace_id,
      (workspace) => {
        // A new tab must not steal the Code view: while it is up the checkout
        // on screen belongs to what is being read, not to the session that
        // just started in the background.
        if (centerMode() === "code") return;
        focusWorkspace(workspace ?? null);
      },
    ),
  );

  trackTabFocus();
  trackQuestions();

  /** How many files, diffs and PRs are parked in the Code tab. */
  const openViewCount = createMemo(() => currentViews().open.length);

  /**
   * The checkout the window is in — the scope of the tab strip.
   *
   * A memo rather than a plain call: it is read by the strip, by every tab
   * shortcut, by the persisted order and by every launcher, and all of them
   * have to agree within one frame or a drag lands in a different worktree
   * than the one it started in.
   */
  const currentWorkspace = createMemo(sharedWorkspace);

  /**
   * The tabs, in the order the strip shows them.
   *
   * Scoped to the current checkout: a tab is a way back into a terminal, and a
   * strip that keeps offering the terminals of a worktree the window has
   * walked away from is one click from typing into the wrong branch.
   */
  function openSessions() {
    return orderedOpenSessions(
      sessionsInWorkspace(forgeStore.sessions, currentWorkspace()),
      currentTabOrder(),
    );
  }

  /** The stored order of this checkout's tabs; the strip drags against it. */
  function currentTabOrder(): string[] {
    const workspace = currentWorkspace();
    return (workspace && tabOrder()[workspace]) || [];
  }

  function persistTabOrder(order: string[]): void {
    const workspace = currentWorkspace();
    if (!workspace) return;
    // Pruned to the checkouts that still exist on the way out. Keyed per
    // checkout, this map would otherwise keep a row for every worktree ever
    // removed, and app state is written back to the daemon on every drag.
    const live = new Set(forgeStore.workspaces.map((item) => item.id));
    const next: TabOrderMap = { [workspace]: order };
    for (const [id, ids] of Object.entries(tabOrder())) {
      if (id !== workspace && live.has(id)) next[id] = ids;
    }
    setTabOrder(next);
    void setAppState(TAB_ORDER_KEY, JSON.stringify(next)).catch(() => undefined);
  }

  function activeProject(): { id: string; name: string } | null {
    const id = projectOfWorkspace(forgeStore.workspaces, currentWorkspace());
    const project = forgeStore.projects.find((item) => item.id === id);
    return project ? { id: project.id, name: project.name } : null;
  }

  /**
   * Point the workbench at the active session's checkout and read its tree, so
   * a palette that lists files has files to list.
   *
   * Cheap enough to do on the way into any palette that admits them: the tree
   * is cached per workspace, and the alternative is `⌘⇧O` quietly showing no
   * files at all until something else has opened the Files panel first.
   */
  function warmFileTree(): void {
    const session = forgeStore.sessions.find((item) => item.id === connectionStore.activeSession);
    const workspace = session?.workspace_id ?? activeWorkspace();
    if (!workspace) return;
    focusWorkspace(workspace);
    warmTree(workspace);
  }

  function openPalette(scope: PaletteScope): void {
    if (scope !== "commands" && scope !== "launch") warmFileTree();
    setPalette(scope);
  }

  function openBranchPicker(): void {
    const project = activeProject();
    if (project) requestNewWorktree(project);
  }

  /**
   * One Ctrl+Tab: the focus ring, plus a few live sessions from other
   * checkouts so the hold list can jump projects without leaving the strip.
   */
  function stepSwitcher(delta: number): void {
    stepTabSwitcher(
      delta,
      switcherTargets(openSessions().map((session) => session.id)),
      activeSwitcherKey(),
      liveIdsByActivity(forgeStore.sessions),
    );
  }

  /** Commit a row: a terminal is a session, a file is a tab inside Code. */
  function focusSwitcherTarget(target: SwitchTarget): void {
    if (target.kind === "session") {
      focusSession(target.id);
      return;
    }
    // The checkout the row was built for, not whichever one the window drifted
    // to while Control was held: the views are parked per checkout, and
    // focusing one into another checkout's strip finds nothing to raise.
    focusWorkspace(target.workspace);
    if (target.kind === "code") showCode();
    else focusCodeView(target.view);
  }

  function windowTabs() {
    return stripItems(openSessions(), codeOpen());
  }

  function activateTab(item: StripItem): void {
    if (item.kind === "code") showCode();
    else focusSession(item.id);
  }

  function cycleTab(delta: number): void {
    const items = windowTabs();
    if (items.length === 0) return;
    const current = stripIndex(items, centerMode() === "code", connectionStore.activeSession);
    const index =
      ((((current < 0 ? 0 : current) + delta) % items.length) + items.length) % items.length;
    activateTab(items[index]);
  }

  /**
   * `MOD-W` closes whatever the centre column is actually showing.
   *
   * Reading a file and pressing it is a request to close *that file*, not to
   * kill the session the file was opened from — the two are different weights
   * of action, and the tab strip on screen is what says which one is meant.
   * With the Code tab up it closes the active view (and the tab itself with
   * the last one); on the terminal it closes the session.
   */
  function closeActive(): void {
    const target = closeTarget(centerMode(), currentViews(), centerSplit(), settings());
    if (target.kind === "none") return;
    if (target.kind === "unsplit") {
      joinPanes();
      return;
    }
    if (target.kind === "view") {
      closeView(target.view);
      return;
    }
    const active = connectionStore.activeSession;
    if (!active) return;
    const session = forgeStore.sessions.find((item) => item.id === active);
    // The daemon refuses to close a live session, so the confirmation is not a
    // courtesy: it is the choice to kill it first.
    if (session && sessionIsActive(session.state)) {
      const title = sessionTitle(session);
      requestConfirm({
        title: `${title} is still running.`,
        description: "Closing it stops the process. Anything it has not written is lost.",
        confirmLabel: "Close session",
        destructive: true,
        onConfirm: () => void closeSession(active).catch(() => undefined),
      });
      return;
    }
    void closeSession(active).catch(() => undefined);
  }

  /**
   * ⌘⇧A. With no default agent configured this opens the palette already
   * narrowed to the launchers — the whole "New agent" menu without a second
   * widget for it; with one, it launches it directly (Settings → Agents).
   *
   * A preference pointing at something uninstalled or deleted resolves back to
   * the palette rather than to silence: it is stale, not a reason to make the
   * shortcut dead.
   */
  function launchDefaultAgent(): void {
    const target = resolveDefaultAgent(
      defaultAgentFrom(forgeStore.app_state),
      forgeStore.launchables,
    );
    switch (target.kind) {
      case "shell":
        void launchShell(currentWorkspace()).catch(() => undefined);
        break;
      case "agent":
        void launchAgent(target.provider, target.profile, currentWorkspace()).catch(
          () => undefined,
        );
        break;
      default:
        setPalette("launch");
    }
  }

  function runChoice(entry: PaletteEntry): void {
    setPalette(null);
    switch (entry.choice.kind) {
      case "new_shell":
        void launchShell(currentWorkspace()).catch(() => undefined);
        break;
      case "new_agent":
        void launchAgent(entry.choice.provider, entry.choice.profile, currentWorkspace()).catch(
          () => undefined,
        );
        break;
      case "focus_session":
        focusSession(entry.choice.session);
        break;
      case "focus_workspace": {
        // A workspace is reached through a session in it, or by opening one.
        const workspace = entry.choice.workspace;
        const session = forgeStore.sessions.find(
          (item) => item.workspace_id === workspace && item.terminal_id !== null,
        );
        if (session) {
          focusSession(session.id);
          break;
        }
        // Pointed at before the launch, not after: the strip has to be showing
        // this checkout by the time its first tab lands in it.
        focusWorkspace(workspace);
        void launchShell(workspace).catch(() => undefined);
        break;
      }
      case "action":
        // Back through the dispatcher rather than a second switch here: the
        // palette is a second way in, not a second implementation.
        invokeAction(entry.choice.action);
        break;
      case "open_file": {
        const workspace = activeWorkspace();
        if (!workspace) break;
        const path = entry.choice.path;
        void openFile(workspace, path)
          .then(() => openEditor(path))
          .catch(() => undefined);
        break;
      }
    }
  }

  onMount(() => {
    let disposeRuntime: (() => void) | undefined;
    let menuUnlisten: (() => void) | undefined;
    let updateUnlisten: (() => void) | undefined;
    void (async () => {
      try {
        // Preferences live in the snapshot's `app_state`, so anything read
        // from it has to wait for the snapshot rather than for the module.
        disposeRuntime = await startAppRuntime({ onConnected: restoreWorkspace });
      } catch {
        // Vite-only preview has no Tauri host.
      }
    })();

    onCleanup(enterContext(APP));
    onCleanup(installKeymap());
    // The rail's branch name is a persisted column: without this a
    // `git checkout` typed into a shell never reaches it.
    onCleanup(startGitSync());
    // The Files panel and the diff are guarded by presence, so a `git pull`
    // that leaves the branch name alone would never reach them.
    onCleanup(startCheckoutWatch());
    // Releasing Control commits, from the module's own key listener rather
    // than from a chord: there is no keymap entry for "let go".
    onCleanup(bindTabSwitcherCommit(focusSwitcherTarget));

    const bound = [
      registerAction("toggle_sidebar", toggleSidebar),
      registerAction(
        "new_terminal",
        // Named rather than left to the daemon's default. With the strip
        // scoped to a checkout, a worktree whose terminals are all closed
        // shows an empty strip, and the daemon's fallback — the workspace of
        // whatever terminal is still attached — would start the shell in the
        // checkout the user just left.
        () => void launchShell(currentWorkspace()).catch(() => undefined),
      ),
      registerAction("new_agent", launchDefaultAgent),
      registerAction("split_pane", splitPane),
      registerAction("join_panes", joinPanes),
      registerAction("close_session", closeActive),
      registerAction("next_session", () => cycleTab(1)),
      registerAction("previous_session", () => cycleTab(-1)),
      registerAction("switch_tab_next", () => stepSwitcher(1)),
      registerAction("switch_tab_previous", () => stepSwitcher(-1)),
      registerAction("focus_session", (index) => {
        const target = windowTabs()[(index ?? 1) - 1];
        if (!target) return;
        activateTab(target);
      }),
      // Registered here, not in the container: the sidebar unmounts when it is
      // collapsed, and a shortcut that names a view has to be able to open the
      // bar it lives in rather than going quiet.
      registerAction("toggle_projects", () => toggleView("Projects")),
      registerAction("toggle_files", () => toggleView("Files")),
      registerAction("find_in_project", () => requestFindInFiles()),
      registerAction("toggle_pull_requests", () => toggleView("PR")),
      registerAction("toggle_git", () => toggleView("Git")),
      registerAction("toggle_history", () => toggleView("History")),
      registerAction("cycle_sidebar_views", cycleSidebarView),
      // The same three functions the overlay over the terminal calls, so a
      // palette entry cannot drift from the button beside it.
      registerAction("session_handoff", startHandoff),
      registerAction("session_spawn_child", startSpawnChild),
      registerAction("session_send_context", startSendContext),
      registerAction("toggle_session_changes", toggleSessionChanges),
      registerAction("review_checkout", openCheckoutReview),
      // U10, on the Code strip. All three are no-ops with nothing open, which
      // is why they check rather than assume: a chord that throws is worse
      // than one that does nothing.
      registerAction("reopen_closed_tab", reopenClosed),
      registerAction("close_other_views", closeOtherViews),
      registerAction("close_views_to_right", closeViewsToRight),
      registerAction("previous_code_view", () => stepCodeView(-1)),
      registerAction("next_code_view", () => stepCodeView(1)),
      registerAction("open_settings", () => {
        setSettingsSection(undefined);
        setSettings((open) => !open);
      }),
      // The menu bar has "About Forge Node" in two places and the palette lists it;
      // without a handler all three were dead. It is the General group — the
      // version, the daemon and what it is running — so it opens that.
      registerAction("about", () => {
        setSettingsSection("General");
        setSettings(true);
      }),
      registerAction("check_for_update", () => {
        void requestUpdateCheck();
      }),
      registerAction("open_command_palette", () => openPalette("everything")),
      registerAction("go_to", () => openPalette("places")),
      registerAction("find_command", () => openPalette("commands")),
      registerAction("open_file_palette", () => openPalette("files")),
      registerAction("new_worktree", openBranchPicker),
      registerAction("add_project", () => {
        void addProjectFromPicker(null).catch(() => undefined);
      }),
      registerAction("scroll_up", () => {
        const id = focusedTerminalId();
        void scrollTerminal(pageLines(), id === "main" ? undefined : id).catch(() => undefined);
      }),
      registerAction("scroll_down", () => {
        const id = focusedTerminalId();
        void scrollTerminal(-pageLines(), id === "main" ? undefined : id).catch(() => undefined);
      }),
      registerAction("editor_zoom_in", () =>
        bumpScale(EDITOR_FONT_SIZE_KEY, 1, EDITOR_FONT_SIZE_RANGE),
      ),
      registerAction("editor_zoom_out", () =>
        bumpScale(EDITOR_FONT_SIZE_KEY, -1, EDITOR_FONT_SIZE_RANGE),
      ),
      registerAction("editor_zoom_reset", () =>
        resetScale(EDITOR_FONT_SIZE_KEY, EDITOR_FONT_SIZE_RANGE),
      ),
    ];
    onCleanup(() => {
      for (const unbind of bound) unbind();
      disposeRuntime?.();
      menuUnlisten?.();
      updateUnlisten?.();
    });

    void listenForUpdates()
      .then((fn) => {
        updateUnlisten = fn;
      })
      .catch(() => undefined);

    void listen<string>("shell:menu", (event) => {
      invokeAction(event.payload as import("../../actions/actions").ActionId);
    })
      .then((fn) => {
        menuUnlisten = fn;
      })
      .catch(() => undefined);
  });

  /*
   * Bubble, not the capture-phase keymap: a bound Escape would steal dismissals
   * from a dialog, a select, and the keyboard-capture row. Two presses inside
   * the window close; a nested overlay that already handled the key never
   * reaches us (`defaultPrevented` / `stopPropagation`).
   */
  createEffect(() => {
    if (!settings()) {
      setSettingsEscArmed(false);
      return;
    }
    let firstAt: number | null = null;
    let armTimer = 0;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.repeat) return;
      if (event.defaultPrevented) return;
      if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
      const now = Date.now();
      if (isSecondEsc(now, firstAt)) {
        event.preventDefault();
        event.stopPropagation();
        firstAt = null;
        window.clearTimeout(armTimer);
        setSettingsEscArmed(false);
        setSettings(false);
        return;
      }
      firstAt = now;
      setSettingsEscArmed(true);
      window.clearTimeout(armTimer);
      armTimer = window.setTimeout(() => {
        if (firstAt !== null && Date.now() - firstAt >= ESC_AGAIN_MS) {
          firstAt = null;
          setSettingsEscArmed(false);
        }
      }, ESC_AGAIN_MS);
    };
    window.addEventListener("keydown", onKeyDown);
    onCleanup(() => {
      window.removeEventListener("keydown", onKeyDown);
      window.clearTimeout(armTimer);
      setSettingsEscArmed(false);
    });
  });

  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  function releaseNotice(): void {
    clearTimeout(noticeTimer);
    if (connectionStore.notice) noticeTimer = setTimeout(() => setNotice(null), NOTICE_DISMISS_MS);
  }
  function holdNotice(): void {
    clearTimeout(noticeTimer);
  }
  createEffect(() => {
    if (connectionStore.notice) releaseNotice();
    else holdNotice();
  });
  onCleanup(holdNotice);
  return (
    <main class="app-shell">
      <TitleBar
        settingsOpen={settings()}
        onCloseSettings={() => setSettings(false)}
        settingsEscArmed={settingsEscArmed()}
        sidebarOpen={sidebarOpen()}
        onToggleSidebar={toggleSidebar}
        sessions={openSessions()}
        tabOrder={currentTabOrder()}
        onReorderTabs={persistTabOrder}
        codeOpen={codeOpen()}
        codeActive={centerMode() === "code" && codeOpen()}
        codeCount={openViewCount()}
        onSelectCode={showCode}
        onCloseCode={closeCode}
      />
      <div
        class="workspace-grid"
        classList={{ "sidebar-collapsed": !sidebarOpen() }}
        style={{ "--sidebar-w": `${sidebarWidth()}px` }}
      >
        <Show when={sidebarOpen()}>
          <Sidebar />
          <ResizeHandle
            side="left"
            label="Resize the sidebar"
            width={sidebarWidth()}
            min={SIDEBAR_RANGE.min}
            max={SIDEBAR_RANGE.max}
            fallback={SIDEBAR_RANGE.fallback}
            onResize={setSidebarWidth}
            onCommit={(width) => writeWidth(SIDEBAR_WIDTH_KEY, width)}
          />
        </Show>
        <CenterStack settings={settings()} settingsSection={settingsSection()} />
      </div>
      {/* What the host refused, and why. It leaves on its own, but never while
          the pointer or focus is on it, so it cannot vanish mid-read. */}
      <Show when={connectionStore.notice}>
        {(reason) => (
          <div
            class="notice"
            role="status"
            onPointerEnter={holdNotice}
            onPointerLeave={releaseNotice}
            onFocusIn={holdNotice}
            onFocusOut={releaseNotice}
          >
            <span>{reason()}</span>
            <IconButton
              label="Dismiss"
              size="sm"
              class="notice-close"
              onClick={() => setNotice(null)}
            >
              ×
            </IconButton>
          </div>
        )}
      </Show>
      <Show when={palette()}>
        {(scope) => (
          <CommandPalette scope={scope()} onChoose={runChoice} onDismiss={() => setPalette(null)} />
        )}
      </Show>
      <Show when={projectDialogsStore.branchPicker}>
        {(project) => (
          <BranchPicker
            projectId={project().id}
            projectName={project().name}
            onDismiss={() => setProjectDialogsStore("branchPicker", null)}
          />
        )}
      </Show>
      <Show when={gitStore.juvaDraft && activeWorkspace()}>
        <JuvaDraftDialog
          workspace={activeWorkspace()!}
          draft={gitStore.juvaDraft!}
          onDismiss={() => setGitStore("juvaDraft", null)}
        />
      </Show>
      <Show when={projectDialogsStore.projectRemoval}>
        {(project) => (
          <RemoveProjectDialog
            project={project()}
            onDismiss={() => setProjectDialogsStore("projectRemoval", null)}
          />
        )}
      </Show>
      <Show when={projectDialogsStore.worktreeRemoval}>
        {(removal) => (
          <RemoveWorktreeDialog
            workspace={removal().workspace}
            reason={removal().reason}
            onDismiss={() => setProjectDialogsStore("worktreeRemoval", null)}
          />
        )}
      </Show>
      {/* Mounted once at the shell root: `toast()` is called from
          stores and API wrappers that have no component to render into. */}
      <ToastRegion />
      <Show when={dialogsStore.confirm}>{(request) => <ConfirmDialog request={request()} />}</Show>
      <Show when={dialogsStore.textInput}>
        {(request) => (
          <TextInputDialog
            request={request()}
            onDismiss={() => setDialogsStore("textInput", null)}
          />
        )}
      </Show>
      <Show when={sessionDialogsStore.handoff}>
        {(request) => (
          <HandoffDialog
            request={request()}
            onDismiss={() => setSessionDialogsStore("handoff", null)}
          />
        )}
      </Show>
      <HandoffProgressDialog />
      <Show when={sessionDialogsStore.spawnChild}>
        {(request) => (
          <SpawnChildDialog
            request={request()}
            onDismiss={() => setSessionDialogsStore("spawnChild", null)}
          />
        )}
      </Show>
      <Show when={sessionDialogsStore.sendContext}>
        {(request) => (
          <SendContextDialog
            request={request()}
            onDismiss={() => setSessionDialogsStore("sendContext", null)}
          />
        )}
      </Show>
      <Show when={tabSwitcherView()}>
        {(view) => (
          <TabSwitcher
            view={view()}
            sessions={forgeStore.sessions}
            workspaces={forgeStore.workspaces}
            projects={forgeStore.projects}
          />
        )}
      </Show>
    </main>
  );
}

function readTabOrder(): TabOrderMap {
  return parseTabOrder(forgeStore.app_state[TAB_ORDER_KEY]);
}

/**
 * One page of scrollback, minus a line of overlap.
 *
 * Read off the pane rather than assumed: `⌘↑` has to move the distance the
 * viewport actually shows, whatever the window has been resized to, and the
 * kept line is what lets a reader stitch two pages together.
 */
function pageLines(): number {
  return Math.max(1, (terminalStore.rows || 24) - 1);
}
