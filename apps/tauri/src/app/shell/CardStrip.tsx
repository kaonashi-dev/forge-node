import { Show, createMemo } from "solid-js";
import { connectionStore } from "../../state/connection";
import { Icon } from "../../theme/icons/index";
import { IconButton } from "../../ui/index";
import type { Session } from "../../contracts/runtime";
import { SessionTabs } from "./SessionTabs";
import { HandoffStatusButton } from "../../features/sessions/HandoffStatusButton";
import { SessionActionBar } from "../../features/sessions/SessionActionBar";
import { waitingSessions } from "../../features/sessions/waiting";
import { focusSession } from "../../features/sessions/sessionActions";
import { sidebarView } from "../../navigation/sidebarStore";
import type { Section } from "../../features/settings/sections";

type CardStripProps = {
  settings: boolean;
  settingsSection: Section;
  sidebarOpen: boolean;
  onShowSidebar: () => void;
  sessions: Session[];
  tabOrder: string[];
  onReorderTabs: (order: string[]) => void;
  codeOpen: boolean;
  codeActive: boolean;
  codeCount: number;
  codeHasViews: boolean;
  onSelectCode: () => void;
  onCloseCode: () => void;
};

/**
 * The card's top strip: session tabs on the left, the session's actions on the
 * right. It is also the window's drag region on this side.
 */
export function CardStrip(props: CardStripProps) {
  const waiting = createMemo(() => waitingSessions());
  /* Projects pins what is waiting above its tree; every other view, and a
     hidden sidebar, leaves the pill as the only way to see it. */
  const showPill = () =>
    waiting().length > 0 && (!props.sidebarOpen || sidebarView() !== "Projects");

  function answerNext(): void {
    const list = waiting();
    const next = list.find((session) => session.id !== connectionStore.activeSession) ?? list[0];
    if (next) focusSession(next.id);
  }

  return (
    <header class="card-strip" data-tauri-drag-region>
      <Show when={!props.sidebarOpen}>
        <div class="title-bar-inset" aria-hidden="true" data-tauri-drag-region />
        <IconButton
          label="Show the sidebar"
          size="md"
          shortcut="toggle_sidebar"
          onClick={props.onShowSidebar}
        >
          <Icon name="panel-left-open" class="forge-icon-muted" />
        </IconButton>
      </Show>
      <Show
        when={!props.settings}
        fallback={
          <>
            <nav class="card-crumbs" aria-label="Location">
              <span class="card-crumb">Settings</span>
              <span class="card-crumb-sep" aria-hidden="true">
                /
              </span>
              <h1 class="card-crumb-current">{props.settingsSection}</h1>
            </nav>
            <div class="tab-strip-spacer" data-tauri-drag-region />
          </>
        }
      >
        <SessionTabs
          sessions={props.sessions}
          activeId={connectionStore.activeSession}
          tabOrder={props.tabOrder}
          onReorderTabs={props.onReorderTabs}
          codeOpen={props.codeOpen}
          codeActive={props.codeActive}
          codeCount={props.codeCount}
          codeHasViews={props.codeHasViews}
          onSelectCode={props.onSelectCode}
          onCloseCode={props.onCloseCode}
        />
        <div class="card-strip-trailing">
          <Show when={showPill()}>
            <button type="button" class="waiting-pill" onClick={answerNext}>
              <span class="waiting-pill-dot" aria-hidden="true" />
              {waiting().length} waiting
            </button>
          </Show>
          <HandoffStatusButton />
          <SessionActionBar />
        </div>
      </Show>
    </header>
  );
}
