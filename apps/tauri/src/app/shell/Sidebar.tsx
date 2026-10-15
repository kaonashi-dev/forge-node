import { createMemo, type JSX } from "solid-js";
import { FileTreePanel } from "../../features/files/explorer/FileTreePanel";
import { GitPanel } from "../../features/git/GitPanel";
import { HistoryPanel } from "../../features/sessions/HistoryPanel";
import { PullRequestPanel } from "../../features/pull-requests/PullRequestPanel";
import { VIEW_ICONS } from "../../navigation/sidebarViews";
import { forgeStore } from "../../state/forgeStore";
import {
  SIDEBAR_VIEWS,
  setSidebarView,
  sidebarView,
  type SidebarView,
} from "../../navigation/sidebarStore";
import { Tabs, type TabDef } from "../../ui/index";
import { ProjectsView } from "../../features/projects/ProjectsView";
import { waitingSessions } from "../../features/sessions/waiting";

/**
 * The one sidebar: a strip of icons above the selected view.
 *
 * `ProjectsView` stays mounted while another view is up, so its scroll,
 * keyboard cursor and open menus survive the trip. That needs a stable `tabs`
 * array: reading a signal inside an inline `SIDEBAR_VIEWS.map(…)` would turn
 * the prop into a getter and remount every panel on each badge change.
 */
export function Sidebar() {
  const waitingCount = createMemo(() => waitingSessions().length);

  const VIEWS: Record<SidebarView, () => JSX.Element> = {
    Projects: () => <ProjectsView />,
    Files: () => <FileTreePanel />,
    History: () => <HistoryPanel />,
    PR: () => <PullRequestPanel />,
    Git: () => <GitPanel />,
  };

  const tabs: TabDef[] = SIDEBAR_VIEWS.map((item) => ({
    value: item,
    label: item,
    icon: VIEW_ICONS[item],
    content: VIEWS[item],
    ...(item === "Projects"
      ? {
          keepMounted: true,
          contentClass: "projects-body",
          badge: () => {
            const count = waitingCount();
            return count > 0
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
      <Tabs
        class="sidebar-tabs"
        aria-label="Sidebar views"
        value={sidebarView()}
        onChange={(value) => setSidebarView(value as SidebarView)}
        listClass="sidebar-views"
        triggerClass="sidebar-view-tab"
        contentClass="sidebar-body"
        tabs={tabs}
      />
    </aside>
  );
}
