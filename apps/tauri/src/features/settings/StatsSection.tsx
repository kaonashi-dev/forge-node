import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { providerId, providerName, type ProviderUsage } from "../../contracts/runtime";
import { forgeStore } from "../../state/forgeStore";
import { Button, Select } from "../../ui/index";
import { Group, Page } from "./SettingsLayout";
import {
  analyticsSessions,
  analyticsTurns,
  analyticsWorkedSecs,
  type ProviderAnalytics,
  tokenTotal,
  type UsageAnalytics,
} from "../../contracts/workbench";
import {
  busiestDay,
  compactTokens,
  count,
  dailySeries,
  dayLabel,
  heatmap,
  money,
  overview,
  percentLabel,
  providerShare,
  resetsIn,
  shortDay,
  statsAccounts,
  statsProviders,
  tokenMix,
  totals,
  trackingSince,
  workedLabel,
  type HeatCell,
  type StatsProvider,
} from "./usageStats";
import { loadUsageAnalytics } from "./commands";
import { settingsStore } from "./state";
import { loading, setLoading } from "../../state/loading";

export function StatsSection() {
  const [loaded, setLoaded] = createSignal(false);

  createEffect(() => {
    if (!loaded()) {
      setLoaded(true);
      setLoading("usage", true);
      void loadUsageAnalytics(30).catch(() => undefined);
    }
  });

  const analytics = () => settingsStore.usage;
  const usageLoading = () => loading.usage;
  const error = () => settingsStore.usageError;

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
            disabled={usageLoading()}
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
        <Show when={usageLoading() && !analytics()}>
          <p class="empty-copy">Loading analytics…</p>
        </Show>
        <Show when={analytics()}>{(data) => <StatsBody analytics={data()} />}</Show>
      </Group>
    </Page>
  );
}

function StatsBody(props: { analytics: UsageAnalytics }) {
  const providers = createMemo(() => statsProviders(props.analytics, forgeStore.providers));
  const reporting = () => providers().filter((provider) => provider.analytics);
  const silent = () => providers().filter((provider) => !provider.analytics);
  const summary = () => overview(props.analytics);
  const sum = () => totals(props.analytics);
  const mix = () => tokenMix(sum());
  const [dailyProvider, setDailyProvider] = createSignal<string | null>(null);
  const daily = () => dailySeries(props.analytics, dailyProvider());
  const cells = () => heatmap(daily(), props.analytics.window_days, new Date());
  const best = () => busiestDay(daily());
  const dailyOptions = () => [
    { value: null, label: "All agents" },
    ...reporting().map((provider) => ({ value: provider.id, label: provider.name })),
  ];

  return (
    <div class="stats-body">
      <div class="stats-headline">
        <StatTile
          label="Est. cost"
          value={money(summary().costMicros)}
          note={
            summary().unpricedTurns > 0
              ? `floor · ${count(summary().unpricedTurns)} unpriced turns`
              : undefined
          }
        />
        <StatTile
          label="Tokens"
          value={compactTokens(summary().tokens)}
          note={`${summary().cachePercent}% from cache`}
        />
        <StatTile
          label="Agent runs"
          value={count(analyticsSessions(props.analytics))}
          note={`${count(analyticsTurns(props.analytics))} turns`}
        />
        <StatTile
          label="Agent time"
          value={workedLabel(analyticsWorkedSecs(props.analytics))}
          note="parallel runs add up"
        />
        <StatTile
          label="Active days"
          value={String(summary().activeDays)}
          note={`of ${props.analytics.window_days}`}
        />
      </div>

      <section class="stats-panel">
        <header class="stats-panel-head">
          <h4>Daily tokens</h4>
          <div class="stats-panel-tools">
            <Show when={best()}>
              {(day) => (
                <span class="stats-badge">
                  Peak {shortDay(day().date)} · {compactTokens(day().tokens)}
                </span>
              )}
            </Show>
            <Show when={reporting().length > 1}>
              <Select<string | null>
                aria-label="Agent shown in the daily bars"
                class="stats-daily-filter"
                value={dailyProvider()}
                options={dailyOptions()}
                onChange={setDailyProvider}
              />
            </Show>
          </div>
        </header>
        <DailyBars cells={cells()} />
      </section>

      <div class="stats-panels">
        <section class="stats-panel">
          <header class="stats-panel-head">
            <h4>Token mix</h4>
          </header>
          <div class="stats-mix" role="img" aria-label="Token mix by kind">
            <For each={mix()}>
              {(segment) => (
                <Show when={segment.tokens > 0}>
                  <span
                    class={`stats-mix-part kind-${segment.key}`}
                    style={{ "flex-grow": segment.tokens }}
                    title={`${segment.label}: ${compactTokens(segment.tokens)}`}
                  />
                </Show>
              )}
            </For>
          </div>
          <ul class="stats-legend">
            <For each={mix()}>
              {(segment) => (
                <>
                  <li>
                    <span class={`stats-swatch kind-${segment.key}`} />
                    <span class="stats-legend-label">{segment.label}</span>
                    <span class="stats-legend-value">{compactTokens(segment.tokens)}</span>
                    <span class="stats-legend-share">
                      {percentLabel(segment.tokens, summary().tokens)}
                    </span>
                  </li>
                  {/* Reasoning is already inside output, so it is a sub-row and
                      never a segment that would sum past the total. */}
                  <Show when={segment.key === "output" && sum().reasoning > 0}>
                    <li class="stats-legend-sub">
                      <span class="stats-legend-label">incl. reasoning</span>
                      <span class="stats-legend-value">{compactTokens(sum().reasoning)}</span>
                      <span class="stats-legend-share" />
                    </li>
                  </Show>
                </>
              )}
            </For>
          </ul>
        </section>

        <Show
          when={forgeStore.usage.length > 0}
          fallback={
            <Show when={forgeStore.providers.length > 0}>
              <section class="stats-panel">
                <header class="stats-panel-head">
                  <h4>Plan limits</h4>
                </header>
                <p class="settings-hint">No provider has reported a subscription allowance yet.</p>
              </section>
            </Show>
          }
        >
          <section class="stats-panel">
            <header class="stats-panel-head">
              <h4>Plan limits</h4>
            </header>
            {/* The provider's word on an allowance, a different reading from
                the counts beside it, which are what was spent. */}
            <div class="stats-meters">
              <For each={forgeStore.usage}>{(reading) => <MeterGroup reading={reading} />}</For>
            </div>
          </section>
        </Show>
      </div>

      <section class="stats-panel">
        <header class="stats-panel-head">
          <h4>By provider</h4>
        </header>
        <Show
          when={reporting().length > 0}
          fallback={<p class="empty-copy">No transcript in the window carried a usage record.</p>}
        >
          <div class="stats-providers">
            <For each={reporting()}>
              {(provider) => (
                <ProviderCard
                  provider={provider}
                  share={
                    provider.analytics ? providerShare(provider.analytics, props.analytics) : 0
                  }
                />
              )}
            </For>
          </div>
        </Show>
        <Show when={silent().length > 0}>
          <p class="settings-hint">
            No token data from{" "}
            {silent()
              .map((provider) => provider.name)
              .join(", ")}
            .
          </p>
        </Show>
      </section>

      <p class="settings-hint stats-since">
        {count(props.analytics.scanned)} transcripts read
        {props.analytics.skipped > 0 ? ` (${count(props.analytics.skipped)} skipped)` : ""} ·
        tracking since {dayLabel(trackingSince(props.analytics))} · updated{" "}
        {dayLabel(props.analytics.collected_at)}
      </p>
    </div>
  );
}

/** One bar per day of the window, quiet days drawn as a stub so gaps read as gaps. */
function DailyBars(props: { cells: HeatCell[] }) {
  const peak = () => props.cells.reduce((max, cell) => Math.max(max, cell.tokens), 0);
  const first = () => props.cells[0];
  const middle = () => props.cells[Math.floor(props.cells.length / 2)];
  const last = () => props.cells[props.cells.length - 1];

  return (
    <div class="stats-daily">
      <div class="stats-daily-bars" role="img" aria-label="Tokens per day">
        <For each={props.cells}>
          {(cell) => (
            <span
              class="stats-daily-day"
              title={`${shortDay(cell.date)} · ${compactTokens(cell.tokens)} tokens`}
            >
              <span
                class="stats-daily-bar"
                classList={{
                  quiet: cell.tokens === 0,
                  peak: cell.tokens > 0 && cell.tokens === peak(),
                }}
                style={{
                  height: cell.tokens === 0 ? undefined : `${(cell.tokens * 100) / peak()}%`,
                }}
              />
            </span>
          )}
        </For>
      </div>
      <div class="stats-daily-axis">
        <span>{first() ? shortDay(first().date) : ""}</span>
        <span>{middle() ? shortDay(middle().date) : ""}</span>
        <span>{last() ? shortDay(last().date) : ""}</span>
      </div>
    </div>
  );
}

function MeterGroup(props: { reading: ProviderUsage }) {
  return (
    <div class="stats-meter-group">
      <span class="stats-meter-account">{meterAccount(props.reading)}</span>
      <For each={props.reading.windows}>
        {(window) => (
          <div class="stats-meter">
            <span class="stats-meter-window">{window.window}</span>
            <span class="stats-meter-track">
              <span
                class="stats-meter-fill"
                classList={{
                  warn: window.used_percent >= 75 && window.used_percent < 90,
                  full: window.used_percent >= 90,
                }}
                style={{ width: `${Math.min(100, Math.max(0, window.used_percent))}%` }}
              />
            </span>
            <span class="stats-meter-value">{Math.round(window.used_percent)}%</span>
            <span class="stats-meter-reset">{resetsIn(window.resets_at, new Date()) ?? ""}</span>
          </div>
        )}
      </For>
    </div>
  );
}

function StatTile(props: { label: string; value: string; note?: string }) {
  return (
    <div class="stats-tile">
      <span class="stats-tile-label">{props.label}</span>
      <span class="stats-tile-value">{props.value}</span>
      <Show when={props.note}>{(note) => <span class="stats-tile-note">{note()}</span>}</Show>
    </div>
  );
}

function ProviderCard(props: { provider: StatsProvider; share: number }) {
  return (
    <article class="stats-provider">
      <header class="stats-provider-head">
        <span class="stats-provider-name">{props.provider.name}</span>
        <span class="stats-badge">{props.share}%</span>
      </header>
      <Show when={props.provider.analytics}>
        {(analytics) => <ProviderMetrics provider={analytics()} share={props.share} />}
      </Show>
    </article>
  );
}

function ProviderMetrics(props: { provider: ProviderAnalytics; share: number }) {
  const accounts = createMemo(() => statsAccounts(props.provider, forgeStore.agent_profiles));
  return (
    <>
      <p class="settings-hint stats-provider-model">
        {props.provider.top_model ?? "no model recorded"}
      </p>
      <div class="stats-provider-figures">
        <span class="stats-provider-tokens">
          {compactTokens(tokenTotal(props.provider.tokens))}
          <small>tokens</small>
        </span>
        <span class="stats-provider-cost">
          {money(props.provider.cost_micros)}
          <Show when={props.provider.unpriced_turns > 0}>
            <small>floor</small>
          </Show>
        </span>
      </div>
      <span class="stats-provider-bar">
        <span class="stats-provider-fill" style={{ width: `${props.share}%` }} />
      </span>
      <span class="stats-provider-facts">
        {count(props.provider.sessions)} runs · {count(props.provider.turns)} turns
      </span>
      <Show when={accounts().length > 0}>
        <ul class="stats-accounts" aria-label="By account">
          <For each={accounts()}>
            {(account) => (
              <li>
                <span class="stats-account-name">{account.name}</span>
                <span class="stats-account-tokens">
                  {compactTokens(tokenTotal(account.analytics.tokens))}
                </span>
                <span class="stats-account-cost">
                  {money(account.analytics.cost_micros)}
                  {account.analytics.unpriced_turns > 0 ? " floor" : ""}
                </span>
                <span class="stats-account-share">{account.share}%</span>
                <span class="stats-account-facts">
                  {count(account.analytics.sessions)} runs · {count(account.analytics.turns)} turns
                </span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </>
  );
}

/**
 * Which login a meter belongs to: a provider reports one reading per account
 * so the provider id alone would print the same name twice.
 */
function meterAccount(reading: ProviderUsage): string {
  const profile = reading.profile_id
    ? forgeStore.agent_profiles.find((item) => item.id === reading.profile_id)
    : null;
  const provider = forgeStore.providers.find((item) => providerId(item) === reading.provider_id);
  const name = provider ? providerName(provider) : reading.provider_id;
  return profile ? `${name} · ${profile.name}` : name;
}
