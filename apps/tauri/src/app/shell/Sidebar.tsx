import { For, Show, type JSX } from "solid-js";
import { FileTreePanel } from "../../features/files/explorer/FileTreePanel";
import { GitPanel } from "../../features/git/GitPanel";
import { HistoryPanel } from "../../features/sessions/HistoryPanel";
import { PullRequestPanel } from "../../features/pull-requests/PullRequestPanel";
import { sidebarPanelId, sidebarTabId } from "../../navigation/sidebarViews";
import { SIDEBAR_VIEWS, sidebarView, type SidebarView } from "../../navigation/sidebarStore";
import { ProjectsView } from "../../features/projects/ProjectsView";

type Panel = { content: () => JSX.Element; keepMounted?: boolean; class?: string };

/**
 * The selected view's panel; its switcher is `SidebarSwitcher` in the title bar.
 *
 * `ProjectsView` stays mounted while another view is up, so its scroll,
 * keyboard cursor and open menus survive the trip.
 */
export function Sidebar() {
  const PANELS: Record<SidebarView, Panel> = {
    Projects: { content: () => <ProjectsView />, keepMounted: true, class: "projects-body" },
    Files: { content: () => <FileTreePanel />, class: "files-body" },
    History: { content: () => <HistoryPanel /> },
    PR: { content: () => <PullRequestPanel /> },
    Git: { content: () => <GitPanel /> },
  };

  return (
    <aside class="sidebar" aria-label="Sidebar">
      <For each={SIDEBAR_VIEWS}>
        {(view) => {
          const panel = PANELS[view];
          const selected = () => sidebarView() === view;
          return (
            <Show when={panel.keepMounted || selected()}>
              <div
                role="tabpanel"
                id={sidebarPanelId(view)}
                aria-labelledby={sidebarTabId(view)}
                class={`sidebar-body ${panel.class ?? ""}`}
                hidden={!selected()}
              >
                {panel.content()}
              </div>
            </Show>
          );
        }}
      </For>
    </aside>
  );
}
