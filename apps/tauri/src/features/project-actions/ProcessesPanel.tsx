import { createMemo, For, Show } from "solid-js";
import { openUrl } from "../../runtime/host";
import { now } from "../../runtime/clock";
import { forgeStore } from "../../state/forgeStore";
import { Icon } from "../../theme/icons";
import { EmptyState, IconButton } from "../../ui/index";
import { killSession, restartSession } from "../sessions/commands";
import { focusSession } from "../sessions/sessionActions";
import { browserUrl, isActive, urlBadge, type ChipState } from "./actionState";
import { groupProcesses, type ProcessRow } from "./processes";

const DOT: Record<ChipState["phase"], string> = {
  idle: "forge-icon-faint",
  waiting: "forge-icon-muted",
  probing: "forge-icon-green",
  running: "forge-icon-green",
  ready: "forge-icon-green",
  timed_out: "forge-icon-amber",
  port_in_use: "forge-icon-amber",
  exited: "forge-icon-faint",
  failed: "forge-icon-red",
};

function elapsed(since: string, at: number): string {
  const seconds = Math.max(0, Math.round((at - Date.parse(since)) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h`;
}

/** What the right side of a row says about its run. */
function status(row: ProcessRow, at: number): string {
  const state = row.state;
  switch (state.phase) {
    case "ready":
      return urlBadge(state.url, row.session.action?.port ?? null) ?? "ready";
    case "waiting":
      return "waiting";
    case "port_in_use":
      return `:${state.port} taken`;
    case "timed_out":
      return "not ready";
    case "exited":
      return state.code === null ? "stopped" : `exit ${state.code}`;
    case "failed":
      return "failed";
    default:
      return elapsed(row.session.created_at, at);
  }
}

/** Every project's action runs, live first. Derived from the store alone. */
export function ProcessesPanel() {
  const groups = createMemo(() =>
    groupProcesses(forgeStore.sessions, forgeStore.workspaces, forgeStore.projects),
  );

  return (
    <Show
      when={groups().length > 0}
      fallback={
        <EmptyState
          message="Nothing running. Run an action from a checkout's row or the command palette."
          actions={[{ action: "configure_actions", label: "Configure actions", icon: "edit" }]}
        />
      }
    >
      <div class="processes">
        <For each={groups()}>
          {(group) => (
            <section class="processes-group" aria-label={group.projectName}>
              <h3>{group.projectName}</h3>
              <For each={group.rows}>{(row) => <ProcessLine row={row} />}</For>
            </section>
          )}
        </For>
      </div>
    </Show>
  );
}

function ProcessLine(props: { row: ProcessRow }) {
  const state = () => props.row.state;
  const url = () => browserUrl(props.row.session);
  return (
    <div class="process-row" data-phase={state().phase}>
      <Icon name={props.row.live ? "activity" : "exited"} size={12} class={DOT[state().phase]} />
      <button
        type="button"
        class="process-row-main"
        onClick={() => focusSession(props.row.session.id)}
        title={state().phase === "failed" ? (state() as { reason: string }).reason : undefined}
      >
        <span class="forge-truncate">{props.row.label}</span>
        <span class="process-row-where forge-truncate">{props.row.workspaceLabel}</span>
      </button>
      <span class="process-row-side">
        <Show when={url()} fallback={<span>{status(props.row, now())}</span>}>
          {(href) => (
            <button
              type="button"
              class="process-link"
              aria-label={`Open ${props.row.label} in the browser`}
              onClick={() => void openUrl(href()).catch(() => undefined)}
            >
              {status(props.row, now())}
              <Icon name="globe" size={11} />
            </button>
          )}
        </Show>
        <Show
          when={isActive(state())}
          fallback={
            <IconButton
              label={`Run ${props.row.label} again`}
              size="xs"
              onClick={() => void restartSession(props.row.session.id).catch(() => undefined)}
            >
              <Icon name="refresh" size={12} />
            </IconButton>
          }
        >
          <IconButton
            label={`Restart ${props.row.label}`}
            size="xs"
            onClick={() => void restartSession(props.row.session.id).catch(() => undefined)}
          >
            <Icon name="refresh" size={12} />
          </IconButton>
          <IconButton
            label={`Stop ${props.row.label}`}
            size="xs"
            onClick={() => void killSession(props.row.session.id).catch(() => undefined)}
          >
            <Icon name="stop" size={12} />
          </IconButton>
        </Show>
      </span>
    </div>
  );
}
