import { For, Show, createMemo } from "solid-js";
import {
  SIDEBAR_VIEWS,
  setSidebarView,
  sidebarView,
  type SidebarView,
} from "../../navigation/sidebarStore";
import { VIEW_ICONS, sidebarPanelId, sidebarTabId } from "../../navigation/sidebarViews";
import { Icon } from "../../theme/icons/index";
import { Tooltip } from "../../ui/index";
import { waitingSessions } from "../../features/sessions/waiting";

type Badge = { count: number; label: string };

/**
 * The sidebar's views, as a tablist in the title bar above the sidebar column.
 *
 * Not the kit's `Tabs`: its list and panels share one Kobalte root, and here
 * they live in different subtrees, so roving focus is done by hand.
 */
export function SidebarSwitcher() {
  let list: HTMLDivElement | undefined;
  const waiting = createMemo(() => waitingSessions().length);

  const badge = (view: SidebarView): Badge | null => {
    if (view !== "Projects") return null;
    const count = waiting();
    return count > 0 ? { count, label: `${count} waiting on you` } : null;
  };

  function onKeyDown(event: KeyboardEvent): void {
    const index = SIDEBAR_VIEWS.indexOf(sidebarView());
    const last = SIDEBAR_VIEWS.length - 1;
    const next =
      event.key === "ArrowRight"
        ? (index + 1) % SIDEBAR_VIEWS.length
        : event.key === "ArrowLeft"
          ? (index + last) % SIDEBAR_VIEWS.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : null;
    if (next === null) return;
    event.preventDefault();
    const view = SIDEBAR_VIEWS[next];
    setSidebarView(view);
    list?.querySelector<HTMLElement>(`#${sidebarTabId(view)}`)?.focus();
  }

  return (
    <div
      ref={list}
      class="sidebar-switcher"
      role="tablist"
      aria-label="Sidebar views"
      onKeyDown={onKeyDown}
    >
      <For each={SIDEBAR_VIEWS}>
        {(view) => {
          const selected = () => sidebarView() === view;
          const name = () => {
            const current = badge(view);
            return current ? `${view} — ${current.label}` : view;
          };
          return (
            <Tooltip label={name()} contents>
              <button
                type="button"
                role="tab"
                id={sidebarTabId(view)}
                class="sidebar-view-tab"
                aria-controls={sidebarPanelId(view)}
                aria-selected={selected()}
                aria-label={name()}
                tabIndex={selected() ? 0 : -1}
                data-selected={selected() ? "" : undefined}
                onClick={() => setSidebarView(view)}
              >
                <Icon name={VIEW_ICONS[view]} size={16} />
                <Show when={badge(view)}>
                  {(current) => (
                    <span class="forge-tab-badge" data-tone="attention" aria-hidden="true">
                      {current().count}
                    </span>
                  )}
                </Show>
              </button>
            </Tooltip>
          );
        }}
      </For>
    </div>
  );
}
