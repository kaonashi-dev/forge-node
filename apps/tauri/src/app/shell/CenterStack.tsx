import { For, Show, createEffect, createSignal, lazy, onCleanup } from "solid-js";
import type { Section } from "../../features/settings/SettingsRoute";
import { centerMode, codeOpen, currentViews, focus } from "../../navigation/viewsStore";
import { close, closeCode, closeOtherViews, closeViewsToRight } from "../../features/editor/tabs";
import { EmptyCenter } from "./EmptyCenter";
import { forgeStore } from "../../state/forgeStore";
import { reconnect } from "../../runtime/host";
import { connectionStore } from "../../state/connection";
import { activeWorkspaceId, sessionsInWorkspace } from "../../features/sessions/sessionScope";
import { PrDetailView } from "../../features/pull-requests/PrDetailView";
import { PrComposeView } from "../../features/pull-requests/PrComposeView";
import { Icon } from "../../theme/icons/index";
import {
  Button,
  ContextMenu,
  EmptyState,
  IconButton,
  Tooltip,
  type MenuItem,
} from "../../ui/index";
import {
  sameView,
  strip,
  viewKey,
  viewLabel,
  viewTitle,
  TERMINAL_VIEW,
  type WorkbenchView,
} from "../../navigation/views";
import { TerminalPane } from "../../features/terminal/TerminalPane";
import { ResizeHandle } from "./ResizeHandle";
import { ViewGlyph } from "./tabs/ViewGlyph";
import { SessionActionBar } from "../../features/sessions/SessionActionBar";
import { SessionChangesPanel } from "../../features/git/SessionChangesPanel";
import {
  QUIET_MS,
  beginSessionChanges,
  mayAutoRefresh,
  splitEntry,
  splitOpen,
} from "../../features/git/sessionChangesStore";
import {
  SESSION_SPLIT_OPEN_KEY,
  SESSION_SPLIT_RANGE,
  SESSION_SPLIT_WIDTH_KEY,
  readFlag,
  readWidth,
  seedFromAppState,
  writeWidth,
} from "../../state/preferences";
import { loadSessionChanges } from "../../features/git/commands";
import { activeWorkspace } from "../../state/workspace";

/**
 * P11: the three heaviest surfaces load when they are first opened.
 *
 * Editor, Diff and Settings stay out of the initial shell so a window that
 * only ever shows a terminal never downloads them. `lazy` at module scope,
 * not inside the render: a component created per render is a component
 * remounted per render, which would throw away the editor's undo history
 * every time the strip re-drew.
 */
const PreviewView = lazy(() =>
  import("../../features/files/preview/PreviewView").then((module) => ({
    default: module.PreviewView,
  })),
);
const EditorView = lazy(() =>
  import("../../features/editor/dom/EditorView").then((module) => ({
    default: module.EditorView,
  })),
);
const EditorTerminalPane = lazy(() =>
  import("../../features/editor/cells/EditorTerminalPane").then((module) => ({
    default: module.EditorTerminalPane,
  })),
);
const DiffView = lazy(() =>
  import("../../features/git/DiffView").then((module) => ({ default: module.DiffView })),
);
const SettingsRoute = lazy(() =>
  import("../../features/settings/SettingsRoute").then((module) => ({
    default: module.SettingsRoute,
  })),
);
const ProjectSearchView = lazy(() =>
  import("../../features/files/search/ProjectSearchView").then((module) => ({
    default: module.ProjectSearchView,
  })),
);
const ReviewView = lazy(() =>
  import("../../features/git/ReviewView").then((module) => ({ default: module.ReviewView })),
);
const PrReviewView = lazy(() =>
  import("../../features/pull-requests/PrReviewView").then((module) => ({
    default: module.PrReviewView,
  })),
);

/**
 * The centre column: the active session's terminal, or the Code tab.
 *
 * The terminal is never unmounted. Taking it off screen with `visibility`
 * rather than `Show` is deliberate: unmounting it would resize the PTY down to
 * nothing and cost a full resync on the way back, so a glance at a diff would
 * reflow every program running in it.
 */
export function CenterStack(props: { settings: boolean; settingsSection?: Section }) {
  const views = () => currentViews();
  const onCode = () => !props.settings && centerMode() === "code" && codeOpen();
  /* The daemon's word, taken at the handshake: before a connection there is no
     editor session either, so the fallback never renders against a live one. */
  const domSurface = () =>
    connectionStore.connection.kind === "connected" &&
    connectionStore.connection.editorSurface === "dom";
  const active = () => views().active;
  /*
   * The grid keeps painting the last frame it was sent, so the window whose
   * last session just closed went on showing that session's scrollback — a
   * dead terminal that reads exactly like a live one. With nothing open the
   * centre column says so, and offers the three ways back in.
   *
   * Counted within the current checkout, like the strip above it: a worktree
   * whose terminals are all closed has nothing to show even while another
   * worktree still has some, and the grid would otherwise go on painting that
   * other worktree's scrollback under this one's empty tab strip.
   */
  const noSessions = () =>
    sessionsInWorkspace(
      forgeStore.sessions,
      activeWorkspaceId(
        forgeStore.sessions,
        connectionStore.activeSession,
        activeWorkspace(),
        forgeStore.workspaces[0]?.id ?? null,
      ),
    ).length === 0;

  const [tabMenu, setTabMenu] = createSignal<{ x: number; y: number; view: WorkbenchView } | null>(
    null,
  );
  const [splitWidth, setSplitWidth] = createSignal(
    readWidth(SESSION_SPLIT_WIDTH_KEY, SESSION_SPLIT_RANGE),
  );
  // Read once above, before the daemon has answered, so the stored width would
  // never come back on its own. The split is behind a `Show`, but the signal
  // is not: it is created with the stack, on the frame the window paints.
  seedFromAppState(() => setSplitWidth(readWidth(SESSION_SPLIT_WIDTH_KEY, SESSION_SPLIT_RANGE)));

  const activeSession = () =>
    forgeStore.sessions.find((item) => item.id === connectionStore.activeSession) ?? null;
  const splitSession = () => {
    const session = activeSession();
    if (!session) return null;
    return splitOpen(session.id, readFlag(SESSION_SPLIT_OPEN_KEY, false)) ? session.id : null;
  };

  function read(session: string): void {
    beginSessionChanges(session);
    void loadSessionChanges(session).catch(() => undefined);
  }

  /*
   * Re-read when the session goes quiet, never on a timer.
   *
   * `last_activity_at` is already broadcast (coalesced to ≤1/s by the PTY
   * thread), so "quiet" is derivable here without a second subscription. The
   * store's floor is what stops a session that bursts and pauses from turning
   * four git subprocesses into a loop; this only decides *when* to ask.
   *
   * Opening the split reads straight away rather than waiting out the quiet
   * window: a panel that says "not read yet" for two seconds after you open it
   * reads as a button that did nothing.
   */
  createEffect(() => {
    const session = splitSession();
    if (!session) return;
    if (splitEntry(session).readAt === null && mayAutoRefresh(session)) {
      read(session);
      return;
    }
    // Read for the dependency, not for the value: every bump re-runs this
    // effect, which cancels the pending timer and arms a fresh one, so the
    // read only happens once the bumps stop.
    activeSession()?.last_activity_at;
    const timer = window.setTimeout(() => {
      if (mayAutoRefresh(session)) read(session);
    }, QUIET_MS);
    onCleanup(() => window.clearTimeout(timer));
  });

  /**
   * Bulk close actions target the context-menu tab.
   *
   * Each one focuses the tab first. "Close others" from a tab's own menu is a
   * statement about *that* tab, and running it against whichever tab happened
   * to be active would close the one the menu is attached to.
   */
  function tabMenuItems(view: WorkbenchView): MenuItem[] {
    const index = strip(views()).findIndex((item) => sameView(item, view));
    const others = strip(views()).length > 1;
    const toRight = index >= 0 && index < strip(views()).length - 1;
    return [
      { kind: "item", label: "Close", icon: "close", run: () => close(view) },
      {
        kind: "item",
        label: "Close Others",
        icon: "close",
        disabled: !others,
        run: () => {
          focus(view);
          closeOtherViews();
        },
      },
      {
        kind: "item",
        label: "Close to the Right",
        icon: "close",
        disabled: !toRight,
        run: () => {
          focus(view);
          closeViewsToRight();
        },
      },
      { kind: "rule" },
      { kind: "item", label: "Close All", icon: "trash", destructive: true, run: closeCode },
    ];
  }

  return (
    <div class="center-stack">
      <Show when={onCode() && strip(views()).length > 0}>
        <div class="workbench-strip" role="tablist" aria-label="Open files">
          <For each={strip(views())}>
            {(view) => (
              <div
                class="workbench-tab"
                classList={{
                  active: sameView(active(), view),
                  dirty:
                    view.kind === "editor-terminal" &&
                    forgeStore.sessions.find((session) => session.id === view.session)?.editor
                      ?.dirty === true,
                }}
                role="tab"
                aria-selected={sameView(active(), view)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setTabMenu({ x: event.clientX, y: event.clientY, view });
                }}
                // Middle-click closes, the way it does in every browser. On
                // `auxclick` and not `mouseup`: `mouseup` fires for the right
                // button too, and would close the tab a context menu is
                // opening on.
                onAuxClick={(event) => {
                  if (event.button !== 1) return;
                  event.preventDefault();
                  close(view);
                }}
              >
                <Tooltip label={viewTitle(view)} contents>
                  <button
                    type="button"
                    class="forge-row workbench-tab-label"
                    onClick={() => focus(view)}
                  >
                    <ViewGlyph view={view} />
                    <span class="workbench-tab-text">{viewLabel(view)}</span>
                  </button>
                </Tooltip>
                <IconButton
                  label={`Close ${viewLabel(view)}`}
                  size="xs"
                  class="workbench-tab-close"
                  onClick={() => close(view)}
                >
                  <span class="workbench-tab-dirty" aria-label="Unsaved changes" />
                  <Icon name="close" class="workbench-tab-close-icon" size={12} />
                </IconButton>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={tabMenu()}>
        {(open) => (
          <ContextMenu
            x={open().x}
            y={open().y}
            items={tabMenuItems(open().view)}
            onDismiss={() => setTabMenu(null)}
          />
        )}
      </Show>

      {/*
        The connection is gone, over the grid it belonged to.
        Not a toast and not only the title-bar pill: the grid keeps painting
        the last frame it received, so a dead terminal looks exactly like an
        idle one, and typing into it is silently lost.
      */}
      <Show when={connectionStore.connection.kind === "disconnected"}>
        <div class="disconnected-banner" role="alert">
          <span class="disconnected-reason">
            {(connectionStore.connection as { reason: string }).reason}
          </span>
          <span class="history-spacer" />
          <span class="panel-note">Retrying…</span>
          <Button
            variant="secondary"
            size="xs"
            onClick={() => void reconnect().catch(() => undefined)}
          >
            Reconnect now
          </Button>
        </div>
      </Show>

      <div
        class="center-slot"
        classList={{
          hidden: props.settings || onCode() || noSessions(),
          split: splitSession() !== null,
        }}
        style={{ "--session-split-w": `${splitWidth()}px` }}
      >
        <div class="terminal-slot">
          <TerminalPane active={!props.settings && centerMode() === "session"} />
          <SessionActionBar />
        </div>
        <Show when={splitSession()}>
          {(session) => (
            <>
              <ResizeHandle
                side="right"
                label="Resize the session changes"
                width={splitWidth()}
                min={SESSION_SPLIT_RANGE.min}
                max={SESSION_SPLIT_RANGE.max}
                fallback={SESSION_SPLIT_RANGE.fallback}
                onResize={setSplitWidth}
                onCommit={(width) => writeWidth(SESSION_SPLIT_WIDTH_KEY, width)}
              />
              <SessionChangesPanel session={session()} />
            </>
          )}
        </Show>
      </div>

      <Show when={!props.settings && !onCode() && noSessions()}>
        <div class="center-view">
          <EmptyCenter />
        </div>
      </Show>

      <Show when={onCode() && strip(views()).length === 0}>
        <div class="center-view">
          <EmptyState
            message="No files open. Open a file to start working in Code."
            actions={[{ action: "open_file_palette", label: "Open file", icon: "folder-open" }]}
          />
        </div>
      </Show>

      <Show when={onCode() && active().kind === "diff"}>
        <div class="center-view">
          <DiffView />
        </div>
      </Show>
      <Show when={onCode() && active().kind === "review"}>
        <div class="center-view">
          <ReviewView workspace={(active() as { workspace: string }).workspace} />
        </div>
      </Show>
      {/* Two surfaces for one editor session, chosen by the daemon and never
          by the tab: `[editor] surface` decides whether the host paints ANSI
          into a cell grid or publishes windows into the DOM. */}
      <Show when={onCode() && active().kind === "editor-terminal"}>
        <div class="center-view">
          <Show
            when={domSurface()}
            fallback={
              <EditorTerminalPane
                session={(active() as { session: string }).session}
                path={(active() as { path: string }).path}
              />
            }
          >
            <EditorView
              session={(active() as { session: string }).session}
              path={(active() as { path: string }).path}
            />
          </Show>
        </div>
      </Show>
      <Show when={onCode() && active().kind === "preview"}>
        <div class="center-view">
          <PreviewView
            workspace={activeWorkspace() ?? ""}
            path={(active() as { path: string }).path}
          />
        </div>
      </Show>
      <Show when={onCode() && active().kind === "pr_detail"}>
        <div class="center-view">
          <PrDetailView prKey={(active() as { key: string }).key} />
        </div>
      </Show>
      <Show when={onCode() && active().kind === "pr_review"}>
        <div class="center-view">
          <PrReviewView prKey={(active() as { key: string }).key} />
        </div>
      </Show>
      <Show when={onCode() && active().kind === "search"}>
        <div class="center-view">
          <ProjectSearchView />
        </div>
      </Show>
      <Show when={onCode() && active().kind === "pr_compose"}>
        <div class="center-view">
          <PrComposeView />
        </div>
      </Show>

      <Show when={props.settings}>
        <SettingsRoute section={props.settingsSection} />
      </Show>
    </div>
  );
}

export { TERMINAL_VIEW, viewKey };
