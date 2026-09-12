import { createMemo, type JSX } from "solid-js";
import { FeaturesPanel } from "../panels/FeaturesPanel";
import { FileTreePanel } from "../panels/FileTreePanel";
import { GitPanel } from "../panels/GitPanel";
import { HistoryPanel } from "../panels/HistoryPanel";
import { PullRequestPanel } from "../panels/PullRequestPanel";
import { VIEW_ICONS } from "../panels/sidebarViews";
import { forgeStore } from "../store/forgeStore";
import {
  SIDEBAR_VIEWS,
  setSidebarView,
  sidebarView,
  type SidebarView,
} from "../store/sidebarStore";
import { Tabs, type TabDef } from "../ui";
import { ProjectsView } from "./ProjectsView";
import { waiting } from "./tree";

/**
 * The one sidebar: a strip of icons above the selected view.
 *
 * `ProjectsView` stays mounted while another view is up, so its scroll,
 * keyboard cursor and open menus survive the trip. That needs a stable `tabs`
 * array: reading a signal inside an inline `SIDEBAR_VIEWS.map(…)` would turn
 * the prop into a getter and remount every panel on each badge change.
 */
export function Sidebar() {
  const waitingCount = createMemo(() => waiting(forgeStore).length);

  const VIEWS: Record<SidebarView, () => JSX.Element> = {
    Projects: () => <ProjectsView />,
    Files: () => <FileTreePanel />,
    History: () => <HistoryPanel />,
    PR: () => <PullRequestPanel />,
    Features: () => <FeaturesPanel />,
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
            return count > 0 ? { count, label: `${count} waiting on you` } : null;
          },
        }
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
