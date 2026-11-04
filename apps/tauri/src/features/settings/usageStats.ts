import {
  providerId,
  providerName,
  type AgentProfile,
  type ProviderInfo,
  type ProviderUsage,
} from "../../contracts/runtime";
import type {
  AccountAnalytics,
  DailyUsage,
  ProviderAnalytics,
  TokenTotals,
  UsageAnalytics,
} from "../../contracts/workbench";
import {
  analyticsSessions,
  analyticsTurns,
  analyticsWorkedSecs,
  tokenTotal,
} from "../../contracts/workbench";

/** One dollar, in the micro-dollar unit costs are counted in. */
export const MICROS_PER_USD = 1_000_000;

export type StatsProvider = {
  id: string;
  name: string;
  analytics: ProviderAnalytics | null;
};

export function statsProviders(
  analytics: UsageAnalytics,
  providers: ProviderInfo[],
): StatsProvider[] {
  const names = new Map(
    providers.map((provider) => [providerId(provider), providerName(provider)]),
  );
  const rows: StatsProvider[] = analytics.providers.map((provider) => ({
    id: provider.provider_id,
    name: names.get(provider.provider_id) ?? provider.provider_id,
    analytics: provider,
  }));
  const included = new Set(rows.map((row) => row.id));
  for (const [id, name] of names) {
    if (id && !included.has(id)) rows.push({ id, name, analytics: null });
  }
  return rows;
}

export type StatsMetric = "tokens" | "cost";

/**
 * What the page is looking at. `profileId` is absent for a whole provider and
 * `null` for that provider's unnamed default account.
 */
export type StatsScope = {
  provider: string | null;
  profileId?: string | null;
};

export function accountScopeKey(provider: string, profileId: string | null): string {
  return `account:${provider}:${profileId ?? "default"}`;
}

export function scopeKey(scope: StatsScope): string {
  if (scope.provider === null) return "all";
  if (scope.profileId === undefined) return `provider:${scope.provider}`;
  return accountScopeKey(scope.provider, scope.profileId);
}

export function parseScopeKey(key: string): StatsScope {
  if (key === "all") return { provider: null };
  const [kind, provider, profile] = key.split(":");
  if (!provider) return { provider: null };
  if (kind === "provider") return { provider };
  if (kind === "account") {
    return { provider, profileId: profile === "default" ? null : (profile ?? null) };
  }
  return { provider: null };
}

export function accountName(profileId: string | null, profiles: AgentProfile[]): string {
  if (profileId === null) return "Default account";
  return profiles.find((profile) => profile.id === profileId)?.name ?? "Removed account";
}

export type StatsAccount = {
  key: string;
  name: string;
  analytics: AccountAnalytics;
  /** Whole percent of the provider's tokens, with `<1%` kept visible. */
  share: string;
};

/**
 * One row per account a provider's transcripts came from, or none when the
 * only one is the unnamed default — a split of one is not a split.
 */
export function statsAccounts(
  provider: ProviderAnalytics,
  profiles: AgentProfile[],
): StatsAccount[] {
  const accounts = provider.accounts ?? [];
  if (accounts.length === 0 || (accounts.length === 1 && accounts[0]?.profile_id === null)) {
    return [];
  }
  const total = tokenTotal(provider.tokens);
  return accounts.map((account) => ({
    key: accountScopeKey(provider.provider_id, account.profile_id),
    name: accountName(account.profile_id, profiles),
    analytics: account,
    share: percentLabel(tokenTotal(account.tokens), total),
  }));
}

export type AccountScope = {
  key: string;
  providerId: string;
  providerName: string;
  profileId: string | null;
  name: string;
  tokens: number;
  costMicros: number;
  sessions: number;
  turns: number;
  workedSecs: number;
};

/** Every account a provider reported, ready to become a scope filter entry. */
export function accountScopes(
  providers: StatsProvider[],
  profiles: AgentProfile[],
): AccountScope[] {
  return providers.flatMap((provider) => {
    const analytics = provider.analytics;
    if (!analytics) return [];
    return (analytics.accounts ?? []).map((account) => ({
      key: accountScopeKey(provider.id, account.profile_id),
      providerId: provider.id,
      providerName: provider.name,
      profileId: account.profile_id,
      name: accountName(account.profile_id, profiles),
      tokens: tokenTotal(account.tokens),
      costMicros: account.cost_micros,
      sessions: account.sessions,
      turns: account.turns,
      workedSecs: account.worked_secs ?? 0,
    }));
  });
}

export type Overview = {
  tokens: number;
  costMicros: number;
  /** Non-zero means the cost is a floor, not an estimate. */
  unpricedTurns: number;
  /** Days the scan saw *any* activity, not days elapsed. */
  activeDays: number;
  /** Share of billed tokens that went into or came out of the prompt cache. */
  cachePercent: number;
};

export function cachePercent(tokens: TokenTotals): number {
  const total = tokenTotal(tokens);
  const cached = tokens.cache_read + tokens.cache_write;
  return total === 0 ? 0 : Math.min(100, Math.round((cached * 100) / total));
}

export function overview(analytics: UsageAnalytics): Overview {
  const tokens = totals(analytics);
  return {
    tokens: tokenTotal(tokens),
    costMicros: analytics.providers.reduce((sum, provider) => sum + provider.cost_micros, 0),
    unpricedTurns: analytics.providers.reduce((sum, provider) => sum + provider.unpriced_turns, 0),
    activeDays: analytics.daily.length,
    cachePercent: cachePercent(tokens),
  };
}

export type ScopeSummary = Overview & {
  label: string;
  tokenTotals: TokenTotals;
  sessions: number;
  turns: number;
  workedSecs: number;
  daily: DailyUsage[];
};

/** The same reading as {@link overview}, narrowed to one provider or account. */
export function scopedSummary(
  analytics: UsageAnalytics,
  providers: StatsProvider[],
  scope: StatsScope,
  profiles: AgentProfile[],
): ScopeSummary {
  if (scope.provider === null) {
    const tokenTotals = totals(analytics);
    return {
      label: "All agents",
      tokenTotals,
      tokens: tokenTotal(tokenTotals),
      costMicros: analytics.providers.reduce((sum, provider) => sum + provider.cost_micros, 0),
      unpricedTurns: analytics.providers.reduce(
        (sum, provider) => sum + provider.unpriced_turns,
        0,
      ),
      sessions: analyticsSessions(analytics),
      turns: analyticsTurns(analytics),
      workedSecs: analyticsWorkedSecs(analytics),
      activeDays: analytics.daily.length,
      cachePercent: cachePercent(tokenTotals),
      daily: analytics.daily,
    };
  }

  const provider = providers.find((row) => row.id === scope.provider);
  const row = provider?.analytics;
  if (!provider || !row) return emptySummary("Unknown provider");

  if (scope.profileId === undefined) {
    return {
      label: provider.name,
      tokenTotals: row.tokens,
      tokens: tokenTotal(row.tokens),
      costMicros: row.cost_micros,
      unpricedTurns: row.unpriced_turns,
      sessions: row.sessions,
      turns: row.turns,
      workedSecs: row.worked_secs,
      activeDays: (row.daily ?? []).length,
      cachePercent: cachePercent(row.tokens),
      daily: row.daily ?? [],
    };
  }

  const account = (row.accounts ?? []).find(
    (candidate) => candidate.profile_id === scope.profileId,
  );
  if (!account) return emptySummary(`${provider.name} · Unknown account`);
  const daily = account.daily ?? [];
  return {
    label: `${provider.name} · ${accountName(account.profile_id, profiles)}`,
    tokenTotals: account.tokens,
    tokens: tokenTotal(account.tokens),
    costMicros: account.cost_micros,
    unpricedTurns: account.unpriced_turns,
    sessions: account.sessions,
    turns: account.turns,
    workedSecs: account.worked_secs ?? 0,
    activeDays: daily.length,
    cachePercent: cachePercent(account.tokens),
    daily,
  };
}

function emptySummary(label: string): ScopeSummary {
  const tokenTotals = { input: 0, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 };
  return {
    label,
    tokenTotals,
    tokens: 0,
    costMicros: 0,
    unpricedTurns: 0,
    sessions: 0,
    turns: 0,
    workedSecs: 0,
    activeDays: 0,
    cachePercent: 0,
    daily: [],
  };
}

/** Every provider's tokens, added up. */
export function totals(analytics: UsageAnalytics): TokenTotals {
  return analytics.providers.reduce<TokenTotals>(
    (acc, provider) => ({
      input: acc.input + provider.tokens.input,
      output: acc.output + provider.tokens.output,
      cache_write: acc.cache_write + provider.tokens.cache_write,
      cache_read: acc.cache_read + provider.tokens.cache_read,
      reasoning: acc.reasoning + provider.tokens.reasoning,
    }),
    { input: 0, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
  );
}

/**
 * The earliest activity any provider reported, or `null` before anything ran.
 *
 * "Tracking since" is a fact about the transcripts, not about the install:
 * a fresh scan of an old checkout reaches back as far as the window allows.
 */
export function trackingSince(analytics: UsageAnalytics): string | null {
  const stamps = analytics.providers
    .map((provider) => provider.first_activity)
    .filter((stamp): stamp is string => stamp !== null)
    .sort();
  return stamps[0] ?? null;
}

export type DailyCell = {
  date: string;
  tokens: number;
  costMicros: number;
};

/**
 * The window as one cell per day, gaps filled.
 *
 * The daemon sends activity-only days ("a day with no activity is absent
 * rather than zero"), so the empty days are synthesized here — a chart with
 * the quiet days missing is not a chart, it is a list that looks like one.
 */
export function dailyCells(daily: DailyUsage[], windowDays: number, today: Date): DailyCell[] {
  const byDate = new Map(daily.map((day) => [day.date, day]));
  const days = Math.max(1, Math.min(windowDays, 366));

  const cells: DailyCell[] = [];
  for (let back = days - 1; back >= 0; back -= 1) {
    const date = isoDate(new Date(today.getTime() - back * 86_400_000));
    const day = byDate.get(date);
    cells.push({
      date,
      tokens: day?.tokens ?? 0,
      costMicros: day?.cost_micros ?? 0,
    });
  }
  return cells;
}

export type ChartSeries = {
  key: string;
  name: string;
  providerId: string;
  colorSlot: number;
  points: DailyCell[];
  tokens: number;
  costMicros: number;
};

export type StatsChart = {
  dates: string[];
  series: ChartSeries[];
};

/**
 * The chart's series for the selected scope and metric.
 *
 * One series per provider by default. A provider or account scope narrows it
 * to a single line rather than falling back to the total, which would put
 * another agent's tokens under the selected name.
 */
export function statsChart(
  providers: StatsProvider[],
  profiles: AgentProfile[],
  scope: StatsScope,
  metric: StatsMetric,
  windowDays: number,
  today: Date,
): StatsChart {
  const dates = dailyCells([], windowDays, today).map((cell) => cell.date);
  const slots = providerColorSlots(providers.map((provider) => provider.id));

  const rows: {
    key: string;
    name: string;
    providerId: string;
    tokens: TokenTotals;
    costMicros: number;
    daily: DailyUsage[];
  }[] = [];
  if (scope.provider === null) {
    for (const provider of providers) {
      if (!provider.analytics) continue;
      rows.push({
        key: provider.id,
        name: provider.name,
        providerId: provider.id,
        tokens: provider.analytics.tokens,
        costMicros: provider.analytics.cost_micros,
        daily: provider.analytics.daily ?? [],
      });
    }
  } else {
    const provider = providers.find((row) => row.id === scope.provider);
    if (!provider?.analytics) return { dates, series: [] };
    if (scope.profileId === undefined) {
      rows.push({
        key: provider.id,
        name: provider.name,
        providerId: provider.id,
        tokens: provider.analytics.tokens,
        costMicros: provider.analytics.cost_micros,
        daily: provider.analytics.daily ?? [],
      });
    } else {
      const account = (provider.analytics.accounts ?? []).find(
        (candidate) => candidate.profile_id === scope.profileId,
      );
      rows.push({
        key: accountScopeKey(provider.id, scope.profileId),
        name: `${provider.name} · ${accountName(scope.profileId, profiles)}`,
        providerId: provider.id,
        tokens: account?.tokens ?? {
          input: 0,
          output: 0,
          cache_write: 0,
          cache_read: 0,
          reasoning: 0,
        },
        costMicros: account?.cost_micros ?? 0,
        daily: account?.daily ?? [],
      });
    }
  }

  const series = rows
    .map((row) => {
      return {
        key: row.key,
        name: row.name,
        providerId: row.providerId,
        colorSlot: slots.get(row.providerId) ?? 0,
        points: dailyCells(row.daily, windowDays, today),
        tokens: tokenTotal(row.tokens),
        costMicros: row.costMicros,
      };
    })
    .filter((row) => chartValue(row, metric) > 0)
    .sort((left, right) => chartValue(right, metric) - chartValue(left, metric));

  return { dates, series };
}

/** A series' headline figure under the selected metric. */
export function chartValue(series: ChartSeries, metric: StatsMetric): number {
  return metric === "cost" ? series.costMicros : series.tokens;
}

/** The day the selected metric peaked across every visible series. */
export function chartPeak(
  chart: StatsChart,
  metric: StatsMetric,
): { date: string; value: number } | null {
  let best: { date: string; value: number } | null = null;
  for (const [index, date] of chart.dates.entries()) {
    const value = chart.series.reduce(
      (sum, series) => sum + pointValue(series.points[index], metric),
      0,
    );
    if (value > 0 && (!best || value > best.value)) best = { date, value };
  }
  return best;
}

export function pointValue(point: DailyCell | undefined, metric: StatsMetric): number {
  if (!point) return 0;
  return metric === "cost" ? point.costMicros : point.tokens;
}

/** A stable palette slot per provider, so a metric switch does not recolor lines. */
export function providerColorSlots(ids: string[]): Map<string, number> {
  const unique = [...new Set(ids)].sort();
  return new Map(unique.map((id, index) => [id, index]));
}

const SERIES_COLORS = [
  "var(--forge-ansi-5)",
  "var(--forge-ansi-4)",
  "var(--forge-ansi-1)",
  "var(--forge-ansi-2)",
  "var(--forge-ansi-3)",
  "var(--forge-ansi-6)",
  "var(--forge-ansi-13)",
  "var(--forge-ansi-14)",
] as const;

export function seriesColor(slot: number): string {
  return SERIES_COLORS[slot % SERIES_COLORS.length] ?? SERIES_COLORS[0];
}

/** A "nice" zero-based scale for the chart's y-axis. */
export function chartScale(peak: number, count = 4): { max: number; ticks: number[] } {
  if (!(peak > 0)) return { max: 0, ticks: [0] };

  const rawStep = peak / count;
  const magnitude = 10 ** Math.floor(Math.log10(rawStep));
  const normalized = rawStep / magnitude;
  const step = (normalized > 5 ? 10 : normalized > 2 ? 5 : normalized > 1 ? 2 : 1) * magnitude;

  const max = Math.ceil(peak / step) * step;
  const ticks: number[] = [];
  for (let value = 0; value <= max + step * 1e-6; value += step) ticks.push(value);
  return { max, ticks };
}

export type MixSegment = {
  key: "input" | "output" | "cache_write" | "cache_read";
  label: string;
  tokens: number;
  /** Whole percent of the billed total. */
  percent: number;
};

/**
 * Input, output and the two halves of the cache, as a bar.
 *
 * `reasoning` is not a segment: it is already inside `output`, and a bar that
 * adds it alongside sums to more than the total it claims to divide.
 */
export function tokenMix(tokens: TokenTotals): MixSegment[] {
  const total = tokenTotal(tokens);
  const parts: { key: MixSegment["key"]; label: string }[] = [
    { key: "input", label: "New input" },
    { key: "output", label: "Output" },
    { key: "cache_read", label: "Cache read" },
    { key: "cache_write", label: "Cache write" },
  ];
  return parts.map((part) => ({
    ...part,
    tokens: tokens[part.key],
    percent: total === 0 ? 0 : Math.round((tokens[part.key] * 100) / total),
  }));
}

/** Each provider's share of the billed tokens, for the bar under its card. */
export function providerShare(provider: ProviderAnalytics, analytics: UsageAnalytics): number {
  const total = tokenTotal(totals(analytics));
  if (total === 0) return 0;
  return Math.round((tokenTotal(provider.tokens) * 100) / total);
}

/** The same share for the badge, where a real sliver must not read as 0%. */
export function providerShareLabel(provider: ProviderAnalytics, analytics: UsageAnalytics): string {
  return percentLabel(tokenTotal(provider.tokens), tokenTotal(totals(analytics)));
}

export type LimitGroup = {
  id: string;
  name: string;
  readings: ProviderUsage[];
};

/** One group per provider, with every account reading kept in order. */
export function limitGroups(readings: ProviderUsage[], providers: ProviderInfo[]): LimitGroup[] {
  const names = new Map(
    providers.map((provider) => [providerId(provider), providerName(provider)]),
  );
  const groups = new Map<string, LimitGroup>();
  for (const reading of readings) {
    const id = reading.provider_id;
    const group = groups.get(id) ?? {
      id,
      name: names.get(id) ?? id,
      readings: [],
    };
    group.readings.push(reading);
    groups.set(id, group);
  }
  return [...groups.values()];
}

export type LimitPace = "under" | "on" | "ahead";

/**
 * The window's expected duration, from the label the provider gave it.
 *
 * Only a label we recognize gets a pace mark: inventing seven days for an
 * unknown window would draw a marker that is not measuring anything.
 */
export function windowDurationSecs(label: string): number | null {
  const normalized = label.trim().toLowerCase();
  const hours = /^(\d+(?:\.\d+)?)h$/.exec(normalized);
  if (hours) return Number(hours[1]) * 3600;
  const days = /^(\d+(?:\.\d+)?)d$/.exec(normalized);
  if (days) return Number(days[1]) * 86_400;
  if (normalized === "day" || normalized === "daily") return 86_400;
  if (normalized === "week" || normalized === "weekly") return 7 * 86_400;
  if (normalized === "month" || normalized === "monthly") return 30 * 86_400;
  return null;
}

/** How far through a window we are, or `null` when its start cannot be read. */
export function elapsedPercent(resetsAt: string | null, window: string, now: Date): number | null {
  const duration = windowDurationSecs(window);
  if (duration === null || !resetsAt) return null;
  const reset = new Date(resetsAt).getTime();
  if (Number.isNaN(reset)) return null;
  const started = reset - duration * 1000;
  const elapsed = (now.getTime() - started) / (duration * 1000);
  return Math.round(Math.min(1, Math.max(0, elapsed)) * 100);
}

/** Spending faster, even, or slower than the window elapses. */
export function paceOf(usedPercent: number, elapsed: number | null): LimitPace | null {
  if (elapsed === null) return null;
  if (usedPercent >= elapsed + 5) return "ahead";
  if (usedPercent <= elapsed - 5) return "under";
  return "on";
}

// ------------------------------------------------------------- formatting ---

/** `2.2B`, `21.2M`, `694.7k`, `812`. */
export function compactTokens(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  return String(value);
}

/** Micro-dollars as money. Two decimals, because it is money. */
export function money(micros: number): string {
  return `$${(micros / MICROS_PER_USD).toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

/** A compact money label for axes and dense figures. */
export function compactMoney(micros: number): string {
  const usd = micros / MICROS_PER_USD;
  if (usd >= 1_000_000) return `$${(usd / 1_000_000).toFixed(1)}M`;
  if (usd >= 1000) return `$${(usd / 1000).toFixed(1)}k`;
  if (usd >= 10) return `$${Math.round(usd)}`;
  if (usd >= 1) return `$${usd.toFixed(1)}`;
  return `$${usd.toFixed(2)}`;
}

/** `12,309`. */
export function count(value: number): string {
  return value.toLocaleString("en-US");
}

/** A whole percent that never claims `0%` for something that happened. */
export function percentLabel(part: number, total: number): string {
  if (total <= 0 || part <= 0) return "0%";
  const percent = (part * 100) / total;
  if (percent < 1) return "<1%";
  return `${Math.round(percent)}%`;
}

/** `resets in 3h 12m`, or `null` when the provider gave no reset or it has passed. */
export function resetsIn(stamp: string | null, now: Date): string | null {
  if (!stamp) return null;
  const at = new Date(stamp).getTime();
  if (Number.isNaN(at)) return null;
  const secs = Math.floor((at - now.getTime()) / 1000);
  return secs > 0 ? `resets in ${workedLabel(secs)}` : null;
}

/** `35d 4h`, `4h 20m`, `12m`, `—` for nothing at all. */
export function workedLabel(secs: number): string {
  if (secs <= 0) return "—";
  const days = Math.floor(secs / 86_400);
  const hours = Math.floor((secs % 86_400) / 3600);
  const minutes = Math.floor((secs % 3600) / 60);
  if (days > 0) return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  if (hours > 0) return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  return `${Math.max(1, minutes)}m`;
}

/** `Jul 22, 2026` from an ISO stamp, or `—` when there is none. */
export function dayLabel(stamp: string | null): string {
  if (!stamp) return "—";
  const parsed = new Date(stamp);
  if (Number.isNaN(parsed.getTime())) return "—";
  return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

/** `Aug 30` — the short form the chart's ends and its best-day badge use. */
export function shortDay(date: string): string {
  const parsed = new Date(`${date}T00:00:00Z`);
  if (Number.isNaN(parsed.getTime())) return date;
  return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric", timeZone: "UTC" });
}

function isoDate(date: Date): string {
  return date.toISOString().slice(0, 10);
}
