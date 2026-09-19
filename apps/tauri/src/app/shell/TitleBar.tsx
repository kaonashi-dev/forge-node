import { Show } from "solid-js";
import { connectionStore } from "../../state/connection";
import { Icon } from "../../theme/icons/index";
import { Button, IconButton, Tooltip } from "../../ui/index";
import { reconnect } from "../../runtime/host";
import type { Session } from "../../contracts/runtime";
import { OpenMenu } from "../../features/sessions/OpenMenu";
import { SessionTabs } from "./SessionTabs";

type TitleBarProps = {
  /**
   * Whether the settings screen is up.
   *
   * The tab strip, sidebar toggle and Open menu act on something the settings
   * layer is covering, so they hide. The bar stays — traffic lights, drag
   * region, daemon pill — and carries a back control plus the Settings title
   * after the lights instead.
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
  const connection = () => connectionStore.connection;
  const instance = () => {
    const state = connection();
    if (state.kind === "connected") {
      return state.instanceId.slice(0, 4);
    }
    return "idle";
  };

  const pillClass = () => {
    switch (connection().kind) {
      case "connected":
        return "online";
      case "connecting":
        return "idle";
      case "disconnected":
        return "error";
      default:
        return "idle";
    }
  };

  const pillLabel = () => {
    const state = connection();
    switch (state.kind) {
      case "connected":
        return `daemon ${instance()}`;
      case "connecting":
        return "connecting";
      case "disconnected":
        return state.reason;
      default:
        return "offline";
    }
  };

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
      <div class="title-bar-end">
        <Show when={!props.settingsOpen}>
          <OpenMenu />
        </Show>
        <Tooltip label={`${pillLabel()} — click to reconnect`} contents>
          <Button
            variant="ghost"
            size="sm"
            class="connection-pill"
            onClick={() => void reconnect().catch(() => undefined)}
          >
            <span class={`status-dot ${pillClass()}`} />
            <span class="connection-pill-label">{pillLabel()}</span>
          </Button>
        </Tooltip>
      </div>
    </header>
  );
}
