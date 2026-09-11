import { Show } from "solid-js";
import { runtimeStore } from "../store/runtimeStore";
import { Icon } from "../theme/icons";
import { Button, IconButton, Tooltip } from "../ui";
import { reconnect } from "../runtime/api";
import type { Session } from "../runtime/types";
import { OpenMenu } from "./OpenMenu";
import { SessionTabs } from "./SessionTabs";
import type { TextInputRequest } from "./TextInputDialog";

type TitleBarProps = {
  /**
   * Whether the settings screen is up.
   *
   * Everything in this bar except the window controls and the daemon pill acts
   * on something the settings layer is covering: the tab strip switches to a
   * terminal nobody can see, and the sidebar toggle folds a bar that is not on
   * screen. The bar stays — it carries the traffic lights and the drag region —
   * but it empties out.
   */
  settingsOpen: boolean;
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
  const connection = () => runtimeStore.connection;
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
        fallback={<div class="title-bar-spacer" data-tauri-drag-region />}
      >
        <IconButton label="Toggle sidebar" onClick={props.onToggleSidebar}>
          <Icon
            name={props.sidebarOpen ? "panel-left-close" : "panel-left-open"}
            class="forge-icon-muted"
          />
        </IconButton>
        <SessionTabs
          sessions={props.sessions}
          activeId={runtimeStore.activeSession}
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
