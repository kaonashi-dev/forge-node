import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { APP } from "../actions/actions";
import { enterContext, installKeymap, invokeAction, registerAction } from "../actions/dispatch";
import { RightPanel } from "../panels/RightPanel";
import { CommandPalette } from "../palette/CommandPalette";
import type { PaletteEntry, PaletteScope } from "../palette/entries";
import {
  addProjectFromPicker,
  closeSession,
  connect,
  scrollTerminal,
  setAppState,
} from "../runtime/api";
import { applyConnected, bindRuntimeEvents } from "../runtime/events";
import { restoreInspectorTab } from "../store/inspectorStore";
import { forgeStore } from "../store/forgeStore";
import {
  requestConfirm,
  requestNewWorktree,
  runtimeStore,
  setRuntimeStore,
} from "../store/runtimeStore";
import { listenForUpdates, requestUpdateCheck } from "../store/updateStore";
import { sessionIsActive, sessionTitle } from "../runtime/types";
import { terminalStore } from "../store/terminalStore";
import { focusWorkspace, setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import {
  centerMode,
  close as closeView,
  closeCode,
  closeOtherViews,
  closeViewsToRight,
  currentViews,
  openEditor,
  openFeatureCompose,
  reopenClosed,
  showCode,
  showSession,
  stepCodeView,
} from "../store/viewsStore";
import { openFile, warmFileTree as warmTree } from "../workbench/api";
import {
  PANEL_OPEN_KEY,
  PANEL_RANGE,
  PANEL_WIDTH_KEY,
  RAIL_OPEN_KEY,
  RAIL_RANGE,
  RAIL_WIDTH_KEY,
  DENSITY_KEY,
  THEME_BASE_KEY,
  readChoice,
  readFlag,
  readWidth,
  writeFlag,
  writeWidth,
} from "./layout";
import { CenterStack } from "./CenterStack";
import type { Section } from "../settings/SettingsRoute";
import { defaultAgentFrom, resolveDefaultAgent } from "../settings/defaultAgent";
import { applyThemeBase, type ThemePreference } from "../theme/ThemeProvider";
import { DENSITIES, applyDensity } from "../theme/density";
import { ResizeHandle } from "./ResizeHandle";
import { Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { TitleBar } from "./TitleBar";
import { BranchPicker } from "./BranchPicker";
import { JuvaDraftDialog } from "./JuvaDraftDialog";
import { ConfirmDialog } from "./ConfirmDialog";
import { RemoveProjectDialog } from "./RemoveProjectDialog";
import { RemoveWorktreeDialog } from "./RemoveWorktreeDialog";
import { TextInputDialog, type TextInputRequest } from "./TextInputDialog";
import { HandoffDialog } from "./HandoffDialog";
import { TabSwitcher } from "./SwitchTab";
import { liveIdsByActivity } from "./tabMru";
import { bindTabSwitcherCommit, stepTabSwitcher, tabSwitcherView } from "./tabSwitcher";
import {
  currentWorkspace as sharedWorkspace,
  focusSession,
  launchAgent,
  launchShell,
  restoreWorkspace,
  openCheckoutReview,
  startHandoff,
  toggleSessionChanges,
} from "./sessionActions";
import { listen } from "@tauri-apps/api/event";
import { openSessions as orderedOpenSessions } from "./tabOrder";
import {
  parseTabOrder,
  projectOfWorkspace,
  sessionsInWorkspace,
  type TabOrderMap,
} from "./sessionScope";
import { closeTarget } from "./closeTarget";
import { inspectorTab, setInspectorTab, type InspectorTab } from "../store/inspectorStore";
import { IconButton, ToastRegion } from "../ui";

const TAB_ORDER_KEY = "ui.tab_order";

export function AppShell() {
  const [railOpen, setRailOpen] = createSignal(true);
  const [panelOpen, setPanelOpen] = createSignal(true);
  const [railWidth, setRailWidth] = createSignal(RAIL_RANGE.fallback);
  const [panelWidth, setPanelWidth] = createSignal(PANEL_RANGE.fallback);
  const [palette, setPalette] = createSignal<PaletteScope | null>(null);
  const [settings, setSettings] = createSignal(false);
  const [settingsSection, setSettingsSection] = createSignal<Section | undefined>();
  const [tabOrder, setTabOrder] = createSignal<TabOrderMap>(readTabOrder());

  // The daemon answers with the stored layout some milliseconds after the
  // window is already on screen. Seeding once — rather than tracking — keeps a
  // later snapshot from yanking a panel the user has since dragged.
  let seeded = false;
  createEffect(() => {
    if (seeded || Object.keys(forgeStore.app_state).length === 0) return;
    seeded = true;
    setRailOpen(readFlag(RAIL_OPEN_KEY, true));
    setPanelOpen(readFlag(PANEL_OPEN_KEY, true));
    setRailWidth(readWidth(RAIL_WIDTH_KEY, RAIL_RANGE));
    setPanelWidth(readWidth(PANEL_WIDTH_KEY, PANEL_RANGE));
    setTabOrder(readTabOrder());
    const base = forgeStore.app_state[THEME_BASE_KEY] as ThemePreference | undefined;
    if (base) applyThemeBase(base);
    // Density is written over `tokens.css` as an inline style, so it has to be
    // re-applied whenever the stored preference lands — the same round trip
    // the theme takes.
    applyDensity(readChoice(DENSITY_KEY, DENSITIES, "default"));
  });

  function toggleRail(): void {
    setRailOpen((open) => !open);
    writeFlag(RAIL_OPEN_KEY, railOpen());
  }

  function togglePanel(): void {
    setPanelOpen((open) => !open);
    writeFlag(PANEL_OPEN_KEY, panelOpen());
  }

  /**
   * Show an inspector tab, opening the panel if it is collapsed.
   *
   * Pressing the chord for a tab that is already up closes the panel — the
   * same toggle every other panel chord has, so the shortcut is a switch
   * rather than a one-way door.
   */
  function revealInspector(tab: InspectorTab): void {
    if (panelOpen() && inspectorTab() === tab) {
      togglePanel();
      return;
    }
    setInspectorTab(tab);
    if (!panelOpen()) togglePanel();
  }

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
    const session = forgeStore.sessions.find((item) => item.id === runtimeStore.activeSession);
    const workspace = session?.workspace_id ?? workbenchStore.workspace;
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
      openSessions().map((session) => session.id),
      runtimeStore.activeSession,
      liveIdsByActivity(forgeStore.sessions),
    );
  }

  function cycleTab(delta: number): void {
    const sessions = openSessions();
    if (sessions.length === 0) return;
    const current = sessions.findIndex((session) => session.id === runtimeStore.activeSession);
    const index =
      ((((current < 0 ? 0 : current) + delta) % sessions.length) + sessions.length) %
      sessions.length;
    focusSession(sessions[index].id);
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
    const target = closeTarget(centerMode(), currentViews());
    if (target.kind === "view") {
      closeView(target.view);
      return;
    }
    const active = runtimeStore.activeSession;
    if (!active) return;
    const session = forgeStore.sessions.find((item) => item.id === active);
    // The daemon refuses to close a live session, so the confirmation is not a
    // courtesy: it is the choice to kill it first (§7.3).
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
        const workspace = workbenchStore.workspace;
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
    let unlisten: (() => void) | undefined;
    let menuUnlisten: (() => void) | undefined;
    let updateUnlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await bindRuntimeEvents();
        const snapshot = await connect();
        if (snapshot) {
          applyConnected(snapshot);
          // Preferences live in the snapshot's `app_state`, so anything read
          // from it has to wait for the snapshot rather than for the module.
          restoreInspectorTab();
          restoreWorkspace();
        }
      } catch {
        // Vite-only preview has no Tauri host.
      }
    })();

    onCleanup(enterContext(APP));
    onCleanup(installKeymap());
    // Releasing Control commits, from the module's own key listener rather
    // than from a chord: there is no keymap entry for "let go".
    onCleanup(bindTabSwitcherCommit(focusSession));

    const bound = [
      registerAction("toggle_sidebar", toggleRail),
      registerAction("toggle_right_panel", togglePanel),
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
      registerAction("close_session", closeActive),
      registerAction("next_session", () => cycleTab(1)),
      registerAction("previous_session", () => cycleTab(-1)),
      registerAction("switch_tab_next", () => stepSwitcher(1)),
      registerAction("switch_tab_previous", () => stepSwitcher(-1)),
      registerAction("focus_session", (index) => {
        const sessions = openSessions();
        const target = sessions[(index ?? 1) - 1];
        if (!target) return;
        focusSession(target.id);
      }),
      // Registered here, not in `RightPanel`: the panel unmounts when it is
      // collapsed, and a shortcut that names a tab has to be able to open the
      // panel it lives in rather than going quiet.
      registerAction("toggle_files", () => revealInspector("Files")),
      registerAction("toggle_pull_requests", () => revealInspector("PR")),
      registerAction("toggle_features", () => revealInspector("Features")),
      registerAction("toggle_lieutenant", () => revealInspector("Lieutenant")),
      registerAction("toggle_git", () => revealInspector("Git")),
      registerAction("toggle_history", () => revealInspector("History")),
      // The same three functions the overlay over the terminal calls, so a
      // palette entry cannot drift from the button beside it.
      registerAction("session_handoff", startHandoff),
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
      // Registering a feature is the harness's entry point, so it is reachable
      // from the palette as well as the `+` menu and the Features panel.
      registerAction("new_feature", () => {
        // The settings layer covers the centre, so a draft opened from
        // Settings → Harness would land underneath it.
        setSettings(false);
        revealInspector("Features");
        openFeatureCompose();
      }),
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
      registerAction("scroll_up", () => void scrollTerminal(pageLines()).catch(() => undefined)),
      registerAction("scroll_down", () => void scrollTerminal(-pageLines()).catch(() => undefined)),
    ];
    onCleanup(() => {
      for (const unbind of bound) unbind();
      unlisten?.();
      menuUnlisten?.();
      updateUnlisten?.();
    });

    void listenForUpdates()
      .then((fn) => {
        updateUnlisten = fn;
      })
      .catch(() => undefined);

    void listen<string>("shell:menu", (event) => {
      invokeAction(event.payload as import("../actions/actions").ActionId);
    })
      .then((fn) => {
        menuUnlisten = fn;
      })
      .catch(() => undefined);
  });

  return (
    <main class="app-shell">
      <TitleBar
        settingsOpen={settings()}
        railOpen={railOpen()}
        panelOpen={panelOpen()}
        onToggleRail={toggleRail}
        onTogglePanel={togglePanel}
        sessions={openSessions()}
        tabOrder={currentTabOrder()}
        onReorderTabs={persistTabOrder}
        codeOpen={openViewCount() > 0}
        codeActive={centerMode() === "code" && openViewCount() > 0}
        codeCount={openViewCount()}
        onSelectCode={showCode}
        onCloseCode={closeCode}
      />
      <div
        class="workspace-grid"
        classList={{ "rail-collapsed": !railOpen(), "panel-collapsed": !panelOpen() }}
        style={{
          "--rail-w": `${railWidth()}px`,
          "--panel-w": `${panelWidth()}px`,
        }}
      >
        <Show when={railOpen()}>
          <Sidebar />
          <ResizeHandle
            side="left"
            label="Resize the project rail"
            width={railWidth()}
            min={RAIL_RANGE.min}
            max={RAIL_RANGE.max}
            fallback={RAIL_RANGE.fallback}
            onResize={setRailWidth}
            onCommit={(width) => writeWidth(RAIL_WIDTH_KEY, width)}
          />
        </Show>
        <CenterStack
          settings={settings()}
          settingsSection={settingsSection()}
          onCloseSettings={() => setSettings(false)}
        />
        <Show when={panelOpen()}>
          <ResizeHandle
            side="right"
            label="Resize the inspector"
            width={panelWidth()}
            min={PANEL_RANGE.min}
            max={PANEL_RANGE.max}
            fallback={PANEL_RANGE.fallback}
            onResize={setPanelWidth}
            onCommit={(width) => writeWidth(PANEL_WIDTH_KEY, width)}
          />
          <RightPanel />
        </Show>
      </div>
      <StatusBar
        onViewDetails={() => {
          setSettingsSection("Stats & Usage");
          setSettings(true);
        }}
        onManageAccounts={() => {
          setSettingsSection("Agents");
          setSettings(true);
        }}
      />
      {/* What the host refused, and why. Dismissed by hand rather than on a
          timer: a message that vanishes before it is read is not a message. */}
      <Show when={runtimeStore.notice}>
        {(reason) => (
          <div class="notice" role="status">
            <span>{reason()}</span>
            <IconButton
              label="Dismiss"
              size="sm"
              class="notice-close"
              onClick={() => setRuntimeStore("notice", null)}
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
      <Show when={runtimeStore.branchPicker}>
        {(project) => (
          <BranchPicker
            projectId={project().id}
            projectName={project().name}
            onDismiss={() => setRuntimeStore("branchPicker", null)}
          />
        )}
      </Show>
      <Show when={workbenchStore.juvaDraft && workbenchStore.workspace}>
        <JuvaDraftDialog
          workspace={workbenchStore.workspace!}
          draft={workbenchStore.juvaDraft!}
          onDismiss={() => setWorkbenchStore("juvaDraft", null)}
        />
      </Show>
      <Show when={runtimeStore.projectRemoval}>
        {(project) => (
          <RemoveProjectDialog
            project={project()}
            onDismiss={() => setRuntimeStore("projectRemoval", null)}
          />
        )}
      </Show>
      <Show when={runtimeStore.worktreeRemoval}>
        {(removal) => (
          <RemoveWorktreeDialog
            workspace={removal().workspace}
            reason={removal().reason}
            onDismiss={() => setRuntimeStore("worktreeRemoval", null)}
          />
        )}
      </Show>
      {/* §4.2 U16. Mounted once, at the shell root: `toast()` is called from
          stores and API wrappers that have no component to render into. */}
      <ToastRegion />
      <Show when={runtimeStore.confirm}>{(request) => <ConfirmDialog request={request()} />}</Show>
      <Show when={runtimeStore.textInput}>
        {(request) => (
          <TextInputDialog
            request={request()}
            onDismiss={() => setRuntimeStore("textInput", null)}
          />
        )}
      </Show>
      <Show when={runtimeStore.handoff}>
        {(request) => (
          <HandoffDialog request={request()} onDismiss={() => setRuntimeStore("handoff", null)} />
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
