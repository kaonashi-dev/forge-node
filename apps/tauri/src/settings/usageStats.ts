import type {
  DailyUsage,
  ProviderAnalytics,
  TokenTotals,
  UsageAnalytics,
} from "../workbench/types";
import { tokenTotal } from "../workbench/types";

/** One dollar, in the micro-dollar unit costs are counted in. */
export const MICROS_PER_USD = 1_000_000;

/**
 * The stats page, as numbers.
 *
 * Everything here is a pure function of what `GetUsageAnalytics` answered, so
 * the view can be read as layout and the arithmetic can be read as arithmetic.
 * Nothing invents a figure the scan did not produce: there is no "agents
 * spawned" and no "pull requests opened" here, because a transcript scan does
 * not know either one.
 */

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

export function overview(analytics: UsageAnalytics): Overview {
  const tokens = totals(analytics);
  const total = tokenTotal(tokens);
  const cached = tokens.cache_read + tokens.cache_write;
  return {
    tokens: total,
    costMicros: analytics.providers.reduce((sum, p) => sum + p.cost_micros, 0),
    unpricedTurns: analytics.providers.reduce((sum, p) => sum + p.unpriced_turns, 0),
    activeDays: analytics.daily.length,
    cachePercent: total === 0 ? 0 : Math.min(100, Math.round((cached * 100) / total)),
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

export type HeatCell = {
  date: string;
  tokens: number;
  /** 0 for a silent day, 1–4 for quarters of the busiest one. */
  level: 0 | 1 | 2 | 3 | 4;
};

/**
 * The daily series as a calendar, gaps filled.
 *
 * The daemon sends activity-only days ("a day with no activity is absent
 * rather than zero"), so the empty days are synthesized here — a calendar with
 * the quiet days missing is not a calendar, it is a list that looks like one.
 *
 * Levels are quarters of the busiest day rather than absolute thresholds: what
 * a heavy day costs differs by an order of magnitude between an agent that
 * reads a repository and one that answers a question.
 */
export function heatmap(daily: DailyUsage[], windowDays: number, today: Date): HeatCell[] {
  const byDate = new Map(daily.map((day) => [day.date, day.tokens]));
  const busiest = daily.reduce((max, day) => Math.max(max, day.tokens), 0);
  const days = Math.max(1, Math.min(windowDays, 366));

  const cells: HeatCell[] = [];
  for (let back = days - 1; back >= 0; back -= 1) {
    const date = isoDate(new Date(today.getTime() - back * 86_400_000));
    const tokens = byDate.get(date) ?? 0;
    cells.push({ date, tokens, level: level(tokens, busiest) });
  }
  return cells;
}

function level(tokens: number, busiest: number): HeatCell["level"] {
  if (tokens === 0 || busiest === 0) return 0;
  const quarter = Math.ceil((tokens / busiest) * 4);
  return Math.min(4, Math.max(1, quarter)) as 1 | 2 | 3 | 4;
}

/** The day that produced the most tokens, or `null` when nothing did. */
export function busiestDay(daily: DailyUsage[]): DailyUsage | null {
  return daily.reduce<DailyUsage | null>(
    (best, day) => (day.tokens > 0 && (!best || day.tokens > best.tokens) ? day : best),
    null,
  );
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
  return `$${(micros / MICROS_PER_USD).toFixed(2)}`;
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

/** `Aug 30` — the short form the heatmap's ends and its best-day badge use. */
export function shortDay(date: string): string {
  const parsed = new Date(`${date}T00:00:00Z`);
  if (Number.isNaN(parsed.getTime())) return date;
  return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric", timeZone: "UTC" });
}

function isoDate(date: Date): string {
  return date.toISOString().slice(0, 10);
}
