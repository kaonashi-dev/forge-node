import { For, Show, createMemo, type JSX } from "solid-js";
import { FileTreePanel } from "../../features/files/explorer/FileTreePanel";
import { GitPanel } from "../../features/git/GitPanel";
import { ProcessesPanel } from "../../features/project-actions/ProcessesPanel";
import { HistoryPanel } from "../../features/sessions/HistoryPanel";
import { PullRequestPanel } from "../../features/pull-requests/PullRequestPanel";
import { VIEW_ICONS } from "../../navigation/sidebarViews";
import {
  SIDEBAR_VIEWS,
  setSidebarView,
  sidebarView,
  type SidebarView,
} from "../../navigation/sidebarStore";
import { Tabs, IconButton, type TabDef } from "../../ui/index";
import { ProjectsView } from "../../features/projects/ProjectsView";
import { waitingSessions } from "../../features/sessions/waiting";
import { invokeAction } from "../../actions/dispatch";
import { Icon } from "../../theme/icons/index";
import { connectionStore } from "../../state/connection";
import { SECTION_ICONS, SETTINGS_SECTIONS, type Section } from "../../features/settings/sections";

const VIEW_LABELS: Record<SidebarView, string> = {
  Projects: "Projects",
  Files: "Files",
  History: "History",
  PR: "Pull requests",
  Git: "Git",
  Processes: "Processes",
};

type SidebarProps = {
  onHide: () => void;
  /** Settings replace the bar's content, not the window. */
  settings: boolean;
  settingsSection: Section;
  onSettingsSection: (section: Section) => void;
  onCloseSettings: () => void;
  settingsEscArmed: boolean;
};

/**
 * The window's left column, from the traffic lights down: the view switcher,
 * the view, and the daemon footer.
 *
 * `ProjectsView` stays mounted while another view is up, so its scroll,
 * keyboard cursor and open menus survive the trip. That needs a stable `tabs`
 * array: reading a signal inside an inline `SIDEBAR_VIEWS.map(…)` would turn
 * the prop into a getter and remount every panel on each badge change.
 */
export function Sidebar(props: SidebarProps) {
  const waitingCount = createMemo(() => waitingSessions().length);

  const VIEWS: Record<SidebarView, () => JSX.Element> = {
    Projects: () => <ProjectsView />,
    Files: () => <FileTreePanel />,
    History: () => <HistoryPanel />,
    PR: () => <PullRequestPanel />,
    Git: () => <GitPanel />,
    Processes: () => <ProcessesPanel />,
  };

  const tabs: TabDef[] = SIDEBAR_VIEWS.map((item) => ({
    value: item,
    label: VIEW_LABELS[item],
    icon: VIEW_ICONS[item],
    content: VIEWS[item],
    ...(item === "Projects"
      ? {
          keepMounted: true,
          contentClass: "projects-body",
          badge: () => {
            const count = waitingCount();
            return count > 0 && sidebarView() !== "Projects"
              ? { count, label: `${count} waiting on you`, tone: "attention" as const }
              : null;
          },
        }
      : item === "Files"
        ? { contentClass: "files-body" }
        : {}),
  }));

  return (
    <aside class="sidebar" aria-label="Sidebar">
      <div class="sidebar-top" data-tauri-drag-region>
        <div class="title-bar-inset" aria-hidden="true" data-tauri-drag-region />
        <div class="sidebar-top-spacer" data-tauri-drag-region />
        <Show when={!props.settings}>
          <IconButton
            label="Hide the sidebar"
            size="sm"
            shortcut="toggle_sidebar"
            onClick={props.onHide}
          >
            <Icon name="panel-left-close" class="forge-icon-muted" />
          </IconButton>
        </Show>
      </div>
      {/* Hidden, not unmounted: Settings is a trip, and ProjectsView's scroll
          and cursor must be where they were on the way back. */}
      <div class="sidebar-work" hidden={props.settings}>
        <Tabs
          class="sidebar-tabs"
          aria-label="Sidebar views"
          value={sidebarView()}
          onChange={(value) => setSidebarView(value as SidebarView)}
          listClass="sidebar-views"
          triggerClass="sidebar-view-tab"
          glide
          contentClass="sidebar-body"
          tabs={tabs}
        />
      </div>
      <Show when={props.settings}>
        <SettingsNav {...props} />
      </Show>
      <SidebarFooter />
    </aside>
  );
}

function SettingsNav(props: SidebarProps) {
  return (
    <nav class="settings-nav-list" aria-label="Settings sections">
      <button
        type="button"
        class="sidebar-jump sidebar-back"
        classList={{ armed: props.settingsEscArmed }}
        onClick={props.onCloseSettings}
      >
        <Icon name="arrow-left" size={14} class="forge-icon-muted" />
        <span class="sidebar-jump-label">
          {props.settingsEscArmed ? "Press Esc again" : "Back to work"}
        </span>
        <span class="sidebar-jump-hint">esc esc</span>
      </button>
      <span class="sidebar-group-label">Settings</span>
      <For each={SETTINGS_SECTIONS}>
        {(item) => (
          <button
            type="button"
            class="forge-row settings-nav"
            aria-current={props.settingsSection === item ? "page" : undefined}
            onClick={() => props.onSettingsSection(item)}
          >
            <Icon name={SECTION_ICONS[item]} size={15} class="settings-nav-icon" />
            {item}
          </button>
        )}
      </For>
    </nav>
  );
}

/**
 * Settings, plus the daemon's state only while it is not connected: a healthy
 * connection is the normal case and is read in Settings › General.
 */
function SidebarFooter() {
  const state = () => connectionStore.connection.kind;
  const trouble = () => {
    switch (state()) {
      case "connected":
        return null;
      case "connecting":
        return { label: "Connecting to the daemon…", tone: "idle" };
      case "disconnected":
        return { label: "Daemon disconnected", tone: "error" };
      default:
        return { label: "Daemon offline", tone: "idle" };
    }
  };
  return (
    <div class="sidebar-footer">
      <Show when={trouble()}>
        {(shown) => (
          <>
            <span class={`status-dot ${shown().tone}`} aria-hidden="true" />
            <span class="sidebar-footer-label forge-truncate" role="status">
              {shown().label}
            </span>
          </>
        )}
      </Show>
      <IconButton
        label="Settings"
        size="sm"
        shortcut="open_settings"
        class="sidebar-footer-settings"
        onClick={() => invokeAction("open_settings")}
      >
        <Icon name="settings" class="forge-icon-muted" />
      </IconButton>
    </div>
  );
}
