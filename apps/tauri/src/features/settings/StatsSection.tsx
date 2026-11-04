import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { type UsageWindow } from "../../contracts/runtime";
import { forgeStore } from "../../state/forgeStore";
import { Badge, Button, RadioGroup, Select } from "../../ui/index";
import { Group, Page } from "./SettingsLayout";
import { tokenTotal, type ProviderAnalytics, type UsageAnalytics } from "../../contracts/workbench";
import {
  accountName,
  accountScopes,
  chartPeak,
  chartValue,
  compactMoney,
  compactTokens,
  count,
  dayLabel,
  elapsedPercent,
  limitGroups,
  money,
  paceOf,
  parseScopeKey,
  percentLabel,
  providerColorSlots,
  providerShare,
  providerShareLabel,
  resetsIn,
  scopedSummary,
  seriesColor,
  shortDay,
  statsAccounts,
  statsChart,
  statsProviders,
  tokenMix,
  trackingSince,
  workedLabel,
  type LimitPace,
  type StatsMetric,
  type StatsProvider,
} from "./usageStats";
import { UsageChart } from "./UsageChart";
import { loadUsageAnalytics } from "./commands";
import { settingsStore } from "./state";
import { loading, setLoading } from "../../state/loading";

const WINDOWS = [
  { value: "7", label: "7d" },
  { value: "30", label: "30d" },
  { value: "90", label: "90d" },
];

const METRICS = [
  { value: "tokens", label: "Tokens" },
  { value: "cost", label: "Cost" },
];

export function StatsSection() {
  const [loaded, setLoaded] = createSignal(false);
  const [days, setDays] = createSignal(30);
  const [metric, setMetric] = createSignal<StatsMetric>("tokens");
  const [scope, setScope] = createSignal("all");

  const analytics = () => settingsStore.usage;
  const usageLoading = () => loading.usage;
  const error = () => settingsStore.usageError;

  function reload(windowDays: number) {
    setLoading("usage", true);
    void loadUsageAnalytics(windowDays).catch(() => undefined);
  }

  createEffect(() => {
    if (!loaded()) {
      setLoaded(true);
      reload(days());
    }
  });

  const scopeOptions = createMemo(() => {
    const options = [{ value: "all", label: "All agents" }];
    const data = analytics();
    if (!data) return options;
    const providers = statsProviders(data, forgeStore.providers);
    const reporting = providers.filter((provider) => provider.analytics);
    for (const provider of reporting) {
      options.push({ value: `provider:${provider.id}`, label: provider.name });
    }
    const accounts = accountScopes(providers, forgeStore.agent_profiles).filter((account) => {
      const provider = reporting.find((row) => row.id === account.providerId);
      return (provider?.analytics?.accounts?.length ?? 0) > 1;
    });
    for (const account of accounts) {
      options.push({
        value: account.key,
        label: `${account.providerName} · ${account.name}`,
      });
    }
    return options;
  });

  createEffect(() => {
    if (!scopeOptions().some((option) => option.value === scope())) setScope("all");
  });

  return (
    <Page
      title="Stats & usage"
      summary="Token analytics counted off agent transcripts on disk — separate from subscription meters, which are the provider's word on allowance."
      wide
    >
      <Group
        title={analytics() ? `Last ${analytics()!.window_days} days` : `Last ${days()} days`}
        aside={
          <div class="stats-toolbar">
            <RadioGroup
              value={String(days())}
              options={WINDOWS}
              onChange={(value) => {
                const next = Number(value);
                if (next === days()) return;
                setDays(next);
                reload(next);
              }}
              label="Analytics window"
              hideLabel
              variant="segmented"
            />
            <RadioGroup
              value={metric()}
              options={METRICS}
              onChange={(value) => setMetric(value === "cost" ? "cost" : "tokens")}
              label="Analytics metric"
              hideLabel
              variant="segmented"
            />
            <Select<string>
              aria-label="Agent or account shown"
              class="stats-scope"
              value={scope()}
              options={scopeOptions()}
              onChange={setScope}
            />
            <Button
              variant="ghost"
              size="xs"
              disabled={usageLoading()}
              onClick={() => reload(days())}
            >
              Refresh
            </Button>
          </div>
        }
        plain
      >
        <Show when={error()}>{(message) => <p class="panel-error">{message()}</p>}</Show>
        <Show when={usageLoading() && !analytics()}>
          <p class="empty-copy">Loading analytics…</p>
        </Show>
        <Show when={analytics()}>
          {(data) => <StatsBody analytics={data()} metric={metric()} scope={scope()} />}
        </Show>
      </Group>
    </Page>
  );
}

function StatsBody(props: { analytics: UsageAnalytics; metric: StatsMetric; scope: string }) {
  const providers = createMemo(() => statsProviders(props.analytics, forgeStore.providers));
  const reporting = () => providers().filter((provider) => provider.analytics);
  const silent = () => providers().filter((provider) => !provider.analytics);
  const scope = () => parseScopeKey(props.scope);
  const summary = () =>
    scopedSummary(props.analytics, providers(), scope(), forgeStore.agent_profiles);
  const chart = () =>
    statsChart(
      providers(),
      forgeStore.agent_profiles,
      scope(),
      props.metric,
      props.analytics.window_days,
      new Date(),
    );
  const peak = () => chartPeak(chart(), props.metric);
  const mix = () => tokenMix(summary().tokenTotals);
  const windowDays = () => props.analytics.window_days;
  const colorSlots = () => providerColorSlots(providers().map((provider) => provider.id));

  const formatMetric = (value: number) =>
    props.metric === "cost" ? compactMoney(value) : compactTokens(value);
  const heroValue = () =>
    props.metric === "cost" ? money(summary().costMicros) : compactTokens(summary().tokens);
  const heroNote = () => {
    if (props.metric === "cost") {
      return summary().unpricedTurns > 0
        ? `floor · ${count(summary().unpricedTurns)} unpriced turns`
        : "API-equivalent estimate";
    }
    return `${summary().cachePercent}% from cache`;
  };
  const seriesTotal = () =>
    chart().series.reduce((sum, series) => sum + chartValue(series, props.metric), 0);

  return (
    <div class="stats-body">
      <div class="stats-top">
        <section class="forge-card stats-hero">
          <span class="stats-hero-label">{summary().label}</span>
          <span class="stats-hero-value">{heroValue()}</span>
          <span class="stats-hero-caption">
            {count(summary().sessions)} runs · {count(summary().turns)} turns
          </span>
          <div class="stats-hero-meta">
            <span>{heroNote()}</span>
            <span class="stats-hero-days">
              {summary().activeDays} of {windowDays()} active days
            </span>
          </div>
          <ul class="stats-series" aria-label="Visible agents">
            <For each={chart().series}>
              {(series) => (
                <li class="stats-series-row">
                  <span
                    class="stats-series-dot"
                    style={{ background: seriesColor(series.colorSlot) }}
                  />
                  <span class="stats-series-name forge-truncate">{series.name}</span>
                  <span class="stats-series-value">
                    {formatMetric(chartValue(series, props.metric))}
                  </span>
                  <span class="stats-series-share">
                    {percentLabel(chartValue(series, props.metric), seriesTotal())}
                  </span>
                </li>
              )}
            </For>
            <Show when={chart().series.length === 0}>
              <li class="stats-series-empty">
                {props.metric === "cost"
                  ? "No priced activity in this window."
                  : "No agent reported in this window."}
              </li>
            </Show>
          </ul>
        </section>

        <section class="forge-card stats-panel stats-chart-panel">
          <header class="stats-panel-head">
            <h4>{props.metric === "cost" ? "Daily cost" : "Daily tokens"}</h4>
            <div class="stats-panel-tools">
              <Show when={peak()}>
                {(day) => (
                  <Badge>
                    Peak {shortDay(day().date)} · {formatMetric(day().value)}
                  </Badge>
                )}
              </Show>
            </div>
          </header>
          <UsageChart chart={chart()} metric={props.metric} />
        </section>
      </div>

      <div class="stats-headline">
        <StatTile
          label="Tokens"
          value={compactTokens(summary().tokens)}
          note={`${summary().cachePercent}% from cache`}
        />
        <StatTile
          label="Est. cost"
          value={money(summary().costMicros)}
          note={
            summary().unpricedTurns > 0
              ? `floor · ${count(summary().unpricedTurns)} unpriced turns`
              : "API equivalent"
          }
        />
        <StatTile
          label="Agent runs"
          value={count(summary().sessions)}
          note={`${count(summary().turns)} turns`}
        />
        <StatTile
          label="Agent time"
          value={workedLabel(summary().workedSecs)}
          note="parallel runs add up"
        />
        <StatTile
          label="Active days"
          value={String(summary().activeDays)}
          note={`of ${windowDays()}`}
        />
      </div>

      <div class="stats-panels">
        <section class="forge-card stats-panel">
          <header class="stats-panel-head">
            <h4>Token mix</h4>
          </header>
          <Show
            when={summary().tokens > 0}
            fallback={<p class="empty-copy">No tokens counted in this window.</p>}
          >
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
                      <span class="forge-caption stats-legend-share">
                        {percentLabel(segment.tokens, summary().tokens)}
                      </span>
                    </li>
                    {/* Reasoning is already inside output, so it is a sub-row and
                        never a segment that would sum past the total. */}
                    <Show when={segment.key === "output" && summary().tokenTotals.reasoning > 0}>
                      <li class="forge-caption stats-legend-sub">
                        <span class="stats-legend-label">incl. reasoning</span>
                        <span class="stats-legend-value">
                          {compactTokens(summary().tokenTotals.reasoning)}
                        </span>
                        <span class="stats-legend-share" />
                      </li>
                    </Show>
                  </>
                )}
              </For>
            </ul>
          </Show>
        </section>

        <PlanLimits />
      </div>

      <section class="forge-card stats-panel">
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
                  shareLabel={
                    provider.analytics
                      ? providerShareLabel(provider.analytics, props.analytics)
                      : "0%"
                  }
                  colorSlot={colorSlots().get(provider.id) ?? 0}
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

function StatTile(props: { label: string; value: string; note?: string }) {
  return (
    <div class="forge-card stats-tile">
      <span class="stats-tile-label">{props.label}</span>
      <span class="stats-tile-value">{props.value}</span>
      <Show when={props.note}>{(note) => <span class="forge-caption">{note()}</span>}</Show>
    </div>
  );
}

function PlanLimits() {
  const groups = createMemo(() => limitGroups(forgeStore.usage, forgeStore.providers));
  const now = new Date();

  return (
    <Show when={forgeStore.providers.length > 0}>
      <section class="forge-card stats-panel">
        <header class="stats-panel-head">
          <h4>Plan limits</h4>
          <span class="forge-caption stats-limits-caption">used of each window</span>
        </header>
        <Show
          when={groups().length > 0}
          fallback={
            <p class="settings-hint">No provider has reported a subscription allowance yet.</p>
          }
        >
          <div class="stats-limits">
            <For each={groups()}>
              {(group) => (
                <div class="stats-limit-group">
                  <div class="stats-limit-head">
                    <span class="stats-limit-provider">{group.name}</span>
                    <Show when={group.readings.length > 1}>
                      <span class="forge-caption stats-limit-count">
                        {group.readings.length} accounts
                      </span>
                    </Show>
                  </div>
                  <For each={group.readings}>
                    {(reading) => (
                      <div class="stats-limit-account">
                        <Show when={group.readings.length > 1 || reading.profile_id !== null}>
                          <span class="stats-limit-account-name">
                            {accountName(reading.profile_id, forgeStore.agent_profiles)}
                          </span>
                        </Show>
                        <For each={reading.windows}>
                          {(window) => <LimitMeter window={window} now={now} />}
                        </For>
                      </div>
                    )}
                  </For>
                </div>
              )}
            </For>
          </div>
        </Show>
      </section>
    </Show>
  );
}

function LimitMeter(props: { window: UsageWindow; now: Date }) {
  const used = () => Math.min(100, Math.max(0, props.window.used_percent));
  const elapsed = () => elapsedPercent(props.window.resets_at, props.window.window, props.now);
  const pace = () => paceOf(used(), elapsed());
  const reset = () => resetsIn(props.window.resets_at, props.now);
  const detail = () =>
    `${used()}% used${
      elapsed() === null ? "" : ` · ${elapsed()}% of the ${props.window.window} window elapsed`
    }${reset() ? ` · ${reset()}` : ""}`;

  return (
    <div class="stats-meter" title={detail()}>
      <span class="stats-meter-window">{props.window.window}</span>
      <span class="stats-meter-track">
        <span
          class="stats-meter-fill"
          classList={{
            warn: used() >= 75 && used() < 90,
            full: used() >= 90,
          }}
          style={{ width: `${used()}%` }}
        />
        <Show when={elapsed() !== null}>
          <span class="stats-meter-elapsed" style={{ left: `${elapsed()}%` }} />
        </Show>
      </span>
      <span class="stats-meter-value">{used()}%</span>
      <span class="stats-meter-pace" data-pace={pace() ?? undefined} title={paceLabel(pace())} />
      <span class="stats-meter-reset" title={props.window.resets_at ?? undefined}>
        {reset() ?? ""}
      </span>
    </div>
  );
}

function paceLabel(pace: LimitPace | null): string | undefined {
  switch (pace) {
    case "ahead":
      return "Ahead of pace: spending faster than the window elapses";
    case "on":
      return "On pace with the window";
    case "under":
      return "Under pace: headroom left for the rest of the window";
    default:
      return undefined;
  }
}

function ProviderCard(props: {
  provider: StatsProvider;
  share: number;
  shareLabel: string;
  colorSlot: number;
}) {
  return (
    <article class="stats-provider">
      <header class="stats-provider-head">
        <span class="stats-provider-name">
          <span class="stats-series-dot" style={{ background: seriesColor(props.colorSlot) }} />
          {props.provider.name}
        </span>
        <Badge>{props.shareLabel}</Badge>
      </header>
      <Show when={props.provider.analytics}>
        {(analytics) => <ProviderMetrics analytics={analytics()} share={props.share} />}
      </Show>
    </article>
  );
}

function ProviderMetrics(props: { analytics: ProviderAnalytics; share: number }) {
  const accounts = createMemo(() => statsAccounts(props.analytics, forgeStore.agent_profiles));
  return (
    <>
      <p class="settings-hint forge-truncate stats-provider-model">
        {props.analytics.top_model ?? "no model recorded"}
      </p>
      <div class="stats-provider-figures">
        <span class="forge-card-title">
          {compactTokens(tokenTotal(props.analytics.tokens))}
          <small class="forge-caption">tokens</small>
        </span>
        <span class="forge-card-title">
          {money(props.analytics.cost_micros)}
          <Show when={props.analytics.unpriced_turns > 0}>
            <small class="forge-caption">floor</small>
          </Show>
        </span>
      </div>
      <span class="stats-provider-bar">
        <span class="stats-provider-fill" style={{ width: `${props.share}%` }} />
      </span>
      <span class="stats-provider-facts">
        {count(props.analytics.sessions)} runs · {count(props.analytics.turns)} turns ·{" "}
        {workedLabel(props.analytics.worked_secs)}
      </span>
      <Show when={accounts().length > 0}>
        <ul class="stats-accounts" aria-label="By account">
          <For each={accounts()}>
            {(account) => (
              <li>
                <span class="forge-truncate">{account.name}</span>
                <span>{compactTokens(tokenTotal(account.analytics.tokens))}</span>
                <span>
                  {money(account.analytics.cost_micros)}
                  {account.analytics.unpriced_turns > 0 ? " floor" : ""}
                </span>
                <span class="stats-account-share">{account.share}</span>
                <span class="forge-caption stats-account-facts">
                  {count(account.analytics.sessions)} runs · {count(account.analytics.turns)} turns
                  · {workedLabel(account.analytics.worked_secs ?? 0)}
                </span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </>
  );
}
