import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { forgeStore } from "../store/forgeStore";
import { terminalStore } from "../store/terminalStore";
import { LATENCY_BUDGET_MS } from "../terminal/latency";
import type { ProviderUsage, UsageWindow as UsageWindowData } from "../runtime/types";
import {
  usageIsStale,
  usagePercent,
  usageResetLabel,
  usageShortResetLabel,
  usageTone,
  usageUpdatedLabel,
} from "./usageMeters";
import { Icon } from "../theme/icons/Icon";
import { SessionGlyph } from "../theme/icons/SessionGlyph";
import { Button, IconButton, Tooltip } from "../ui";
import { refreshSnapshot } from "../runtime/api";
import { workspaceFolderLabel, workspacePathSegment } from "./workspaceLabel";
import { currentCheckout } from "./sessionActions";

type StatusBarProps = {
  onViewDetails?: () => void;
  onManageAccounts?: () => void;
};

export function StatusBar(props: StatusBarProps) {
  const [now, setNow] = createSignal(Date.now());
  const [open, setOpen] = createSignal(false);
  const [expanded, setExpanded] = createSignal<string | null>(null);
  const [mode, setMode] = createSignal<"detailed" | "compact">("detailed");

  const reportedUsage = createMemo(() =>
    forgeStore.usage.filter((reading) => reading.windows.length > 0),
  );

  onMount(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    onCleanup(() => window.clearInterval(timer));
  });

  // The checkout the window is pointed at, which is what the strip, the
  // launchers and the inspector are all scoped to. Reading the active session
  // instead would name a branch nothing else on screen is showing whenever the
  // rail is on a worktree whose terminals are closed.
  const workspace = currentCheckout;

  const branchLabel = createMemo(() => {
    const ws = workspace();
    if (!ws) return "no workspace";
    return ws.branch ?? workspaceFolderLabel(ws);
  });

  const folderSegment = createMemo(() => {
    const ws = workspace();
    return ws ? workspacePathSegment(ws) : null;
  });

  function handleRefresh(): void {
    void refreshSnapshot().catch(() => undefined);
  }

  function handleViewDetails(): void {
    setOpen(false);
    props.onViewDetails?.();
  }

  function handleManageAccounts(): void {
    setOpen(false);
    props.onManageAccounts?.();
  }

  return (
    <footer class="status-bar">
      <span class="status-branch">⑂ {branchLabel()}</span>
      <Show when={folderSegment()}>
        {(segment) => (
          <Tooltip label={workspace()?.path ?? "No workspace"} placement="top" contents>
            <span class="status-path">{segment()}</span>
          </Tooltip>
        )}
      </Show>
      <span class="status-spacer" />
      <DropdownMenu open={open()} onOpenChange={setOpen} placement="top" gutter={8}>
        <DropdownMenu.Trigger
          class="status-usage-trigger"
          aria-label="View usage details"
          disabled={reportedUsage().length === 0}
        >
          <span class="status-usage">
            <For each={reportedUsage()}>
              {(reading) => (
                <UsageReading
                  reading={reading}
                  provider={providerName(reading.provider_id)}
                  providerId={reading.provider_id}
                  now={now()}
                />
              )}
            </For>
            <Show when={reportedUsage().length === 0}>
              <span class="status-usage-empty">no usage</span>
            </Show>
          </span>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content class="usage-popover">
            <div class="usage-popover-header">
              <span class="usage-popover-title">Usage</span>
              <span class="usage-popover-subtitle">all agents</span>
              <IconButton
                size="sm"
                label="Refresh usage"
                onClick={(event) => {
                  event.stopPropagation();
                  handleRefresh();
                }}
              >
                <Icon name="refresh" size={14} class="forge-icon-muted" />
              </IconButton>
            </div>
            <div class="usage-popover-toggle" role="group" aria-label="Usage display">
              <Button
                size="xs"
                selected={mode() === "detailed"}
                onClick={() => setMode("detailed")}
              >
                Detailed
              </Button>
              <Button size="xs" selected={mode() === "compact"} onClick={() => setMode("compact")}>
                Compact
              </Button>
            </div>
            <div class="usage-popover-body">
              <Show
                when={reportedUsage().length > 0}
                fallback={<p class="usage-popover-empty">No usage reported yet.</p>}
              >
                <For each={reportedUsage()}>
                  {(reading) => (
                    <ProviderRow
                      reading={reading}
                      provider={providerName(reading.provider_id)}
                      providerId={reading.provider_id}
                      now={now()}
                      expanded={expanded() === reading.provider_id}
                      mode={mode()}
                      onToggle={() =>
                        setExpanded((current) =>
                          current === reading.provider_id ? null : reading.provider_id,
                        )
                      }
                    />
                  )}
                </For>
              </Show>
            </div>
            <div class="usage-popover-footer">
              <Button size="sm" class="usage-popover-link" onClick={handleViewDetails}>
                <span>Usage details & history</span>
                <Icon name="chevron-right" size={14} class="forge-icon-muted" />
              </Button>
              <Button size="sm" class="usage-popover-link" onClick={handleManageAccounts}>
                <span>Manage Accounts…</span>
                <Icon name="chevron-right" size={14} class="forge-icon-muted" />
              </Button>
            </div>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu>
      <IconButton
        label="Refresh usage"
        size="sm"
        class="status-usage-refresh"
        title="Refresh usage"
        onClick={handleRefresh}
      >
        <Icon name="refresh" size={14} class="forge-icon-muted" />
      </IconButton>
      <Show when={terminalStore.cols > 0 || terminalStore.latencyP95 !== null}>
        <span class="status-group">
          <Show when={terminalStore.cols > 0}>
            <span>
              {terminalStore.cols}×{terminalStore.rows}
            </span>
          </Show>
          <Show when={terminalStore.latencyP95 !== null}>
            <Tooltip
              label="Key-to-render p95 over the last 120 keystrokes"
              placement="top"
              contents
            >
              <span
                class="status-latency"
                classList={{ over: (terminalStore.latencyP95 ?? 0) > LATENCY_BUDGET_MS }}
              >
                {Math.round(terminalStore.latencyP95 ?? 0)} ms
              </span>
            </Tooltip>
          </Show>
        </span>
      </Show>
      <span class="status-group">
        <span>sessions {forgeStore.live_sessions}</span>
        <span>
          agents {forgeStore.installed_agents}/{forgeStore.provider_count}
        </span>
      </span>
      <Tooltip label="Forge Node v0.1.0" placement="top" contents>
        <span class="status-version">v0.1.0</span>
      </Tooltip>
    </footer>
  );
}

function ProviderRow(props: {
  reading: ProviderUsage;
  provider: string;
  providerId: string;
  now: number;
  expanded: boolean;
  mode: "detailed" | "compact";
  onToggle: () => void;
}) {
  const firstReset = () => {
    const windows = props.reading.windows;
    for (const window of windows) {
      const label = usageShortResetLabel(window.resets_at, props.now);
      if (label) return `Resets in ${label}`;
    }
    return null;
  };
  const stale = () => usageIsStale(props.reading.collected_at, props.now);
  const primaryWindow = () => props.reading.windows[0];

  return (
    <div class="usage-provider" classList={{ expanded: props.expanded, stale: stale() }}>
      <Button
        size="sm"
        class="usage-provider-head"
        aria-expanded={props.expanded}
        onClick={props.onToggle}
      >
        <SessionGlyph providerId={props.providerId} size={16} />
        <span class="usage-provider-name">{props.provider}</span>
        <Show when={firstReset()}>
          {(label) => <span class="usage-provider-reset">{label()}</span>}
        </Show>
        <span class="usage-provider-chevron">
          <Icon name="chevron-right" size={14} class="forge-icon-muted" />
        </span>
      </Button>
      <Show when={!props.expanded}>
        <div class="usage-provider-summary">
          <For each={props.reading.windows}>
            {(window) => {
              const percent = () => usagePercent(window.used_percent);
              const isStale = () => stale();
              return (
                <span class="usage-summary-meter" classList={{ stale: isStale() }}>
                  <span class="usage-summary-track">
                    <span
                      class={`usage-summary-fill ${usageTone(percent(), isStale())}`}
                      style={{ width: `${percent()}%` }}
                    />
                  </span>
                  <span class="usage-summary-label">
                    {window.window} {percent()}%
                  </span>
                </span>
              );
            }}
          </For>
        </div>
      </Show>
      <Show when={props.expanded}>
        <div class="usage-provider-detail">
          <span class="usage-provider-updated">
            {usageUpdatedLabel(props.reading.collected_at, props.now)}
          </span>
          <For each={props.reading.windows}>
            {(window) => (
              <UsageWindowDetail
                window={window}
                provider={props.provider}
                collectedAt={props.reading.collected_at}
                now={props.now}
                detailed={props.mode === "detailed"}
              />
            )}
          </For>
          <Show when={primaryWindow()}>
            {(window) => (
              <Show
                when={props.mode === "detailed" && window().window.toLowerCase().includes("5h")}
              >
                <div class="usage-account-row">
                  <span>{props.provider} Account</span>
                  <span class="usage-account-value">System default</span>
                  <Icon name="chevron-right" size={12} class="forge-icon-muted" />
                </div>
              </Show>
            )}
          </Show>
        </div>
      </Show>
    </div>
  );
}

function UsageWindowDetail(props: {
  window: UsageWindowData;
  provider: string;
  collectedAt: string;
  now: number;
  detailed: boolean;
}) {
  const percent = () => usagePercent(props.window.used_percent);
  const stale = () => usageIsStale(props.collectedAt, props.now);
  const reset = () => usageResetLabel(props.window.resets_at, props.now);
  const label = () =>
    props.detailed ? `${percent()}% used` : `${props.window.window} ${percent()}%`;

  return (
    <div class="usage-window-detail">
      <div class="usage-window-head">
        <span class="usage-window-name">{props.window.window}</span>
        <span class="usage-window-percent">{label()}</span>
        <Show when={reset()}>{(text) => <span class="usage-window-reset">{text()}</span>}</Show>
      </div>
      <div class="usage-window-track">
        <span
          class={`usage-window-fill ${usageTone(percent(), stale())}`}
          style={{ width: `${percent()}%` }}
        />
      </div>
    </div>
  );
}

function UsageReading(props: {
  reading: ProviderUsage;
  provider: string;
  providerId: string;
  now: number;
}) {
  return (
    <span class="status-usage-group">
      <SessionGlyph providerId={props.providerId} size={12} />
      <For each={props.reading.windows}>
        {(window) => (
          <UsageWindowMeter
            reading={window}
            provider={props.provider}
            collectedAt={props.reading.collected_at}
            now={props.now}
          />
        )}
      </For>
    </span>
  );
}

function UsageWindowMeter(props: {
  reading: UsageWindowData;
  provider: string;
  collectedAt: string;
  now: number;
}) {
  const percent = () => usagePercent(props.reading.used_percent);
  const stale = () => usageIsStale(props.collectedAt, props.now);
  const reset = () => usageResetLabel(props.reading.resets_at, props.now);
  const label = () =>
    `${percent()}% used${reset() ? ` ${usageShortResetLabel(props.reading.resets_at, props.now) ?? ""}` : ""}`;

  return (
    <Tooltip
      label={`${props.provider} · ${props.reading.window} · ${label()}`}
      placement="top"
      contents
    >
      <span class="status-usage-meter" classList={{ stale: stale() }}>
        <span class="status-usage-track">
          <span
            class={`status-usage-fill ${usageTone(percent(), stale())}`}
            style={{ width: `${percent()}%` }}
          />
        </span>
        <span class="status-usage-label">
          {props.reading.window} {label()}
        </span>
      </span>
    </Tooltip>
  );
}

function providerName(providerId: string): string {
  const provider = forgeStore.providers.find((item) => item.descriptor?.id === providerId);
  return provider?.descriptor?.display_name ?? providerId;
}
