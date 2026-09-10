import { For, Show, createEffect, createSignal } from "solid-js";
import type { ProviderUsage } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { setLoading, setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import { loadUsageAnalytics } from "../workbench/api";
import { Button } from "../ui";
import { Group, Page } from "./SettingsLayout";
import {
  analyticsSessions,
  analyticsTurns,
  analyticsWorkedSecs,
  type ProviderAnalytics,
  tokenTotal,
  type UsageAnalytics,
} from "../workbench/types";
import {
  busiestDay,
  compactTokens,
  dayLabel,
  heatmap,
  money,
  overview,
  providerShare,
  shortDay,
  tokenMix,
  totals,
  trackingSince,
  workedLabel,
  type HeatCell,
} from "./usageStats";

export function StatsSection() {
  const [loaded, setLoaded] = createSignal(false);

  createEffect(() => {
    if (!loaded()) {
      setLoaded(true);
      setLoading("usage", true);
      void loadUsageAnalytics(30).catch(() => undefined);
    }
  });

  const analytics = () => workbenchStore.usage;
  const loading = () => workbenchStore.loading.usage;
  const error = () => workbenchStore.usageError;

  return (
    <Page
      title="Stats & Usage"
      summary="Token analytics counted off agent transcripts on disk — separate from subscription meters, which are the provider's word on allowance."
    >
      <Group
        title="Last 30 days"
        aside={
          <Button
            variant="ghost"
            size="xs"
            disabled={loading()}
            onClick={() => {
              setLoading("usage", true);
              void loadUsageAnalytics(30).catch(() => undefined);
            }}
          >
            Refresh
          </Button>
        }
      >
        <Show when={error()}>{(message) => <p class="panel-error">{message()}</p>}</Show>
        <Show when={loading() && !analytics()}>
          <p class="empty-copy">Loading analytics…</p>
        </Show>
        <Show when={analytics()}>{(data) => <StatsBody analytics={data()} />}</Show>
      </Group>
    </Page>
  );
}

function StatsBody(props: { analytics: UsageAnalytics }) {
  const summary = () => overview(props.analytics);
  const mix = () => tokenMix(totals(props.analytics));
  const cells = () => heatmap(props.analytics.daily, props.analytics.window_days, new Date());
  const best = () => busiestDay(props.analytics.daily);
  const reasoning = () => totals(props.analytics).reasoning;

  return (
    <div class="stats-body">
      {/* What the scan is a scan *of*, before any number about it. */}
      <div class="stats-headline">
        <StatTile
          label="Agent runs"
          value={String(analyticsSessions(props.analytics))}
          note={`${analyticsTurns(props.analytics)} turns`}
        />
        <StatTile
          label="Time agents worked"
          value={workedLabel(analyticsWorkedSecs(props.analytics))}
          note="parallel agents counted twice"
        />
        <StatTile
          label="Transcripts read"
          value={String(props.analytics.scanned)}
          note={props.analytics.skipped > 0 ? `${props.analytics.skipped} skipped` : undefined}
        />
      </div>
      <p class="settings-hint stats-since">
        Tracking since {dayLabel(trackingSince(props.analytics))} · updated{" "}
        {dayLabel(props.analytics.collected_at)}
      </p>

      <div class="stats-overview">
        <StatTile label="Total tokens" value={compactTokens(summary().tokens)} />
        <StatTile
          label="Est. cost"
          value={money(summary().costMicros)}
          note={
            summary().unpricedTurns > 0
              ? `floor — ${summary().unpricedTurns} unpriced turns`
              : undefined
          }
        />
        <StatTile label="Active days" value={String(summary().activeDays)} />
        <StatTile label="Cache share" value={`${summary().cachePercent}%`} />
      </div>

      <div class="stats-panels">
        <section class="stats-panel">
          <header class="stats-panel-head">
            <h4>Daily intensity</h4>
            <Show when={best()}>
              {(day) => <span class="stats-badge">Best: {shortDay(day().date)}</span>}
            </Show>
          </header>
          <p class="settings-hint">Billed tokens per day, across every provider.</p>
          <Heatmap cells={cells()} />
        </section>

        <section class="stats-panel">
          <header class="stats-panel-head">
            <h4>Token mix</h4>
            <Show when={reasoning() > 0}>
              <span class="stats-badge">{compactTokens(reasoning())} reasoning</span>
            </Show>
          </header>
          {/* Reasoning is not a segment: it is already inside output, and a bar
              that shows it alongside sums past the total it is dividing. */}
          <p class="settings-hint">
            Input, output and both halves of the cache. Reasoning is counted inside output.
          </p>
          <div class="stats-mix" role="img" aria-label="Token mix by kind">
            <For each={mix()}>
              {(segment) => (
                <Show when={segment.percent > 0}>
                  <span
                    class={`stats-mix-part kind-${segment.key}`}
                    style={{ width: `${segment.percent}%` }}
                    title={`${segment.label}: ${compactTokens(segment.tokens)} (${segment.percent}%)`}
                  />
                </Show>
              )}
            </For>
          </div>
          <ul class="stats-legend">
            <For each={mix()}>
              {(segment) => (
                <li>
                  <span class={`stats-swatch kind-${segment.key}`} />
                  <span class="stats-legend-label">{segment.label}</span>
                  <span class="stats-legend-value">{compactTokens(segment.tokens)}</span>
                </li>
              )}
            </For>
          </ul>
        </section>
      </div>

      <section class="stats-panel">
        <header class="stats-panel-head">
          <h4>Providers</h4>
          <span class="stats-badge">{props.analytics.providers.length} with data</span>
        </header>
        <div class="stats-providers">
          <For
            each={props.analytics.providers}
            fallback={<p class="empty-copy">No transcript in the window carried a usage record.</p>}
          >
            {(provider) => (
              <ProviderCard provider={provider} share={providerShare(provider, props.analytics)} />
            )}
          </For>
        </div>
      </section>

      <Show when={forgeStore.usage.length > 0}>
        <section class="stats-panel">
          <header class="stats-panel-head">
            <h4>Subscription meters</h4>
          </header>
          {/* The provider's word on an allowance, which is a different reading
              from the one above: that one counts what was spent. */}
          <For each={forgeStore.usage}>
            {(reading) => (
              <div class="stats-meter-row">
                <span>{meterAccount(reading)}</span>
                <For each={reading.windows}>
                  {(window) => (
                    <span class="settings-row-note">
                      {window.window}: {window.used_percent}%
                    </span>
                  )}
                </For>
              </div>
            )}
          </For>
        </section>
      </Show>
      <Show when={forgeStore.usage.length === 0 && forgeStore.providers.length > 0}>
        <p class="settings-hint">No provider has reported a subscription allowance yet.</p>
      </Show>
    </div>
  );
}

/** A calendar of the window, one cell per day, quiet days included. */
function Heatmap(props: { cells: HeatCell[] }) {
  const first = () => props.cells[0];
  const last = () => props.cells[props.cells.length - 1];

  return (
    <div class="stats-heatmap">
      <div class="stats-heat-grid">
        <For each={props.cells}>
          {(cell) => (
            <span
              class={`stats-heat-cell level-${cell.level}`}
              title={`${shortDay(cell.date)} · ${compactTokens(cell.tokens)} tokens`}
            />
          )}
        </For>
      </div>
      <div class="stats-heat-foot">
        <span>{first() ? shortDay(first().date) : ""}</span>
        <span class="stats-heat-scale">
          Less
          <span class="stats-heat-cell level-0" />
          <span class="stats-heat-cell level-1" />
          <span class="stats-heat-cell level-2" />
          <span class="stats-heat-cell level-3" />
          <span class="stats-heat-cell level-4" />
          More
        </span>
        <span>{last() ? shortDay(last().date) : ""}</span>
      </div>
    </div>
  );
}

function StatTile(props: { label: string; value: string; note?: string }) {
  return (
    <div class="stats-tile">
      <span class="stats-tile-value">{props.value}</span>
      <span class="stats-tile-label">{props.label}</span>
      <Show when={props.note}>{(note) => <span class="stats-tile-note">{note()}</span>}</Show>
    </div>
  );
}

function ProviderCard(props: { provider: ProviderAnalytics; share: number }) {
  const tokens = () => tokenTotal(props.provider.tokens);
  return (
    <article class="stats-provider">
      <header class="stats-provider-head">
        <span class="stats-provider-name">{props.provider.provider_id}</span>
        <span class="stats-badge">{props.share}%</span>
      </header>
      <p class="settings-hint stats-provider-model">
        {props.provider.top_model ?? "no model recorded"}
      </p>
      <div class="stats-provider-facts">
        <span>{compactTokens(tokens())} tokens</span>
        <span>
          {props.provider.sessions} runs · {props.provider.turns} turns
        </span>
        <span>
          {money(props.provider.cost_micros)}
          {props.provider.unpriced_turns > 0 ? " floor" : ""}
        </span>
      </div>
      <span class="stats-provider-bar">
        <span class="stats-provider-fill" style={{ width: `${props.share}%` }} />
      </span>
    </article>
  );
}

/**
 * Which login a meter belongs to: a provider reports one reading per account
 * (§13.4), so the provider id alone would print the same name twice.
 */
function meterAccount(reading: ProviderUsage): string {
  const profile = reading.profile_id
    ? forgeStore.agent_profiles.find((item) => item.id === reading.profile_id)
    : null;
  return profile ? `${reading.provider_id} · ${profile.name}` : reading.provider_id;
}
