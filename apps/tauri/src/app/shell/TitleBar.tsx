import { Show, createMemo } from "solid-js";
import { connectionStore } from "../../state/connection";
import { Icon } from "../../theme/icons/index";
import { IconButton } from "../../ui/index";
import type { Session } from "../../contracts/runtime";
import { SessionTabs } from "./SessionTabs";
import { HandoffStatusButton } from "../../features/sessions/HandoffStatusButton";
import { waitingSessions } from "../../features/sessions/waiting";
import { focusSession } from "../../features/sessions/sessionActions";
import { invokeAction } from "../../actions/dispatch";

type TitleBarProps = {
  /**
   * Whether the settings screen is up.
   *
   * The tab strip and sidebar toggle act on something the settings layer is
   * covering, so they hide. The bar stays — traffic lights and drag region —
   * and carries a back control plus the Settings title after the lights
   * instead.
   */
  settingsOpen: boolean;
  onCloseSettings: () => void;
  /** First Escape has been seen; the next one inside the window will close. */
  settingsEscArmed: boolean;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  sessions: Session[];
  tabOrder: string[];
  onReorderTabs: (order: string[]) => void;
  codeOpen: boolean;
  codeActive: boolean;
  codeCount: number;
  onSelectCode: () => void;
  onCloseCode: () => void;
};

export function TitleBar(props: TitleBarProps) {
  const waiting = createMemo(() => waitingSessions());

  function answerNext(): void {
    const list = waiting();
    const next = list.find((session) => session.id !== connectionStore.activeSession) ?? list[0];
    if (next) focusSession(next.id);
  }

  return (
    <header class="title-bar" data-tauri-drag-region>
      <div class="title-bar-inset" aria-hidden="true" data-tauri-drag-region />
      <Show
        when={!props.settingsOpen}
        fallback={
          <>
            <IconButton
              label="Back"
              title={props.settingsEscArmed ? "Press Esc again to close" : "Back · Esc Esc"}
              selected={props.settingsEscArmed}
              onClick={props.onCloseSettings}
            >
              <Icon name="arrow-left" class="forge-icon-muted" />
            </IconButton>
            <h1 class="title-bar-title">Settings</h1>
            <div class="title-bar-spacer" data-tauri-drag-region />
          </>
        }
      >
        <IconButton label="Toggle sidebar" onClick={props.onToggleSidebar}>
          <Icon
            name={props.sidebarOpen ? "panel-left-close" : "panel-left-open"}
            class="forge-icon-muted"
          />
        </IconButton>
        <SessionTabs
          sessions={props.sessions}
          activeId={connectionStore.activeSession}
          tabOrder={props.tabOrder}
          onReorderTabs={props.onReorderTabs}
          codeOpen={props.codeOpen}
          codeActive={props.codeActive}
          codeCount={props.codeCount}
          onSelectCode={props.onSelectCode}
          onCloseCode={props.onCloseCode}
        />
      </Show>
      <div class="title-bar-trailing">
        <Show when={!props.settingsOpen && waiting().length > 0}>
          <button type="button" class="waiting-chip" onClick={answerNext}>
            <span class="forge-attention-dot" aria-hidden="true" />
            {waiting().length} waiting on you
          </button>
        </Show>
        <HandoffStatusButton />
        <Show when={!props.settingsOpen}>
          <IconButton label="Command palette" onClick={() => invokeAction("open_command_palette")}>
            <Icon name="search" class="forge-icon-muted" />
          </IconButton>
        </Show>
      </div>
    </header>
  );
}
