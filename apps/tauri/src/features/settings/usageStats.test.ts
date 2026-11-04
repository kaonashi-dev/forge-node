import { describe, expect, it } from "vitest";
import type { AgentProfile, ProviderInfo, ProviderUsage } from "../../contracts/runtime";
import type {
  AccountAnalytics,
  ProviderAnalytics,
  UsageAnalytics,
} from "../../contracts/workbench";
import {
  accountName,
  accountScopes,
  cachePercent,
  chartPeak,
  chartScale,
  compactMoney,
  compactTokens,
  dailyCells,
  dayLabel,
  elapsedPercent,
  limitGroups,
  money,
  overview,
  paceOf,
  parseScopeKey,
  percentLabel,
  providerShare,
  providerShareLabel,
  resetsIn,
  scopeKey,
  scopedSummary,
  statsAccounts,
  statsChart,
  statsProviders,
  tokenMix,
  totals,
  trackingSince,
  windowDurationSecs,
  workedLabel,
} from "./usageStats";

const CLAUDE = {
  descriptor: { id: "claude", display_name: "Claude Code" },
} satisfies ProviderInfo;

const CODEX = {
  descriptor: { id: "codex", display_name: "Codex CLI" },
} satisfies ProviderInfo;

function provider(partial: Partial<ProviderAnalytics> = {}): ProviderAnalytics {
  return {
    provider_id: "claude",
    tokens: { input: 100, output: 200, cache_write: 100, cache_read: 600, reasoning: 50 },
    sessions: 4,
    turns: 40,
    cost_micros: 1_500_000,
    unpriced_turns: 0,
    top_model: "claude-opus-5",
    worked_secs: 3600,
    first_activity: "2026-08-01T00:00:00Z",
    last_activity: "2026-08-30T00:00:00Z",
    ...partial,
  };
}

function analytics(partial: Partial<UsageAnalytics> = {}): UsageAnalytics {
  return {
    providers: [provider()],
    daily: [],
    window_days: 30,
    scanned: 4,
    skipped: 0,
    collected_at: "2026-08-30T12:00:00Z",
    ...partial,
  };
}

function account(profile_id: string | null, input: number): AccountAnalytics {
  return {
    profile_id,
    tokens: { input, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
    sessions: 1,
    turns: 2,
    cost_micros: 0,
    unpriced_turns: 0,
  };
}

function profile(id: string, name: string): AgentProfile {
  return {
    id,
    provider_id: "claude",
    name,
    executable: null,
    config_dir: `.claude-${id}`,
    args: [],
    created_at: "2026-08-01T00:00:00Z",
  };
}

const TODAY = new Date("2026-08-30T12:00:00Z");

describe("overview", () => {
  it("counts billed tokens and leaves reasoning out of the total", () => {
    // reasoning: 50 is inside output: 200 and must not be added again.
    expect(overview(analytics()).tokens).toBe(1000);
  });

  it("reports the cache share as a whole percent of the billed total", () => {
    expect(overview(analytics()).cachePercent).toBe(70);
  });

  it("counts active days from the series, not from the window", () => {
    const data = analytics({
      window_days: 30,
      daily: [
        { date: "2026-08-29", tokens: 10 },
        { date: "2026-08-30", tokens: 20 },
      ],
    });
    expect(overview(data).activeDays).toBe(2);
  });

  it("adds the unpriced turns up, so the cost can say it is a floor", () => {
    const data = analytics({
      providers: [provider({ unpriced_turns: 3 }), provider({ unpriced_turns: 4 })],
    });
    expect(overview(data).unpricedTurns).toBe(7);
  });

  it("is all zeroes, not NaN, with nothing scanned", () => {
    expect(overview(analytics({ providers: [] }))).toEqual({
      tokens: 0,
      costMicros: 0,
      unpricedTurns: 0,
      activeDays: 0,
      cachePercent: 0,
    });
  });
});

describe("cachePercent", () => {
  it("counts both halves of the cache and never exceeds 100", () => {
    expect(
      cachePercent({ input: 10, output: 10, cache_write: 10, cache_read: 70, reasoning: 0 }),
    ).toBe(80);
    expect(
      cachePercent({ input: 0, output: 0, cache_write: 80, cache_read: 20, reasoning: 0 }),
    ).toBe(100);
  });

  it("is 0 rather than NaN with nothing counted", () => {
    expect(cachePercent({ input: 0, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 })).toBe(
      0,
    );
  });
});

describe("totals", () => {
  it("adds every provider's five counters", () => {
    const summed = totals(analytics({ providers: [provider(), provider()] }));
    expect(summed).toEqual({
      input: 200,
      output: 400,
      cache_write: 200,
      cache_read: 1200,
      reasoning: 100,
    });
  });
});

describe("trackingSince", () => {
  it("takes the earliest first activity across providers", () => {
    const data = analytics({
      providers: [
        provider({ first_activity: "2026-08-10T00:00:00Z" }),
        provider({ first_activity: "2026-07-22T00:00:00Z" }),
      ],
    });
    expect(trackingSince(data)).toBe("2026-07-22T00:00:00Z");
  });

  it("is null when no provider reported one", () => {
    expect(
      trackingSince(analytics({ providers: [provider({ first_activity: null })] })),
    ).toBeNull();
  });
});

describe("dailyCells", () => {
  it("fills the days the daemon left out, because absent means zero", () => {
    const cells = dailyCells([{ date: "2026-08-30", tokens: 100, cost_micros: 250 }], 3, TODAY);
    expect(cells.map((cell) => cell.date)).toEqual(["2026-08-28", "2026-08-29", "2026-08-30"]);
    expect(cells.map((cell) => cell.tokens)).toEqual([0, 0, 100]);
    expect(cells.map((cell) => cell.costMicros)).toEqual([0, 0, 250]);
  });

  it("is all zeroes rather than empty when nothing ran", () => {
    const cells = dailyCells([], 5, TODAY);
    expect(cells).toHaveLength(5);
    expect(cells.every((cell) => cell.tokens === 0 && cell.costMicros === 0)).toBe(true);
  });
});

describe("scopedSummary", () => {
  const providers = (data: UsageAnalytics) => statsProviders(data, [CLAUDE, CODEX]);

  it("reads every provider for the all-agents scope", () => {
    const data = analytics({
      providers: [provider(), provider({ provider_id: "codex", cost_micros: 500_000 })],
      daily: [{ date: "2026-08-30", tokens: 100 }],
    });
    const summary = scopedSummary(data, providers(data), { provider: null }, []);
    expect(summary.label).toBe("All agents");
    expect(summary.tokens).toBe(2000);
    expect(summary.costMicros).toBe(2_000_000);
    expect(summary.sessions).toBe(8);
    expect(summary.activeDays).toBe(1);
  });

  it("narrows to one provider without falling back to the total", () => {
    const data = analytics({ providers: [provider(), provider({ provider_id: "codex" })] });
    const summary = scopedSummary(data, providers(data), { provider: "codex" }, []);
    expect(summary.label).toBe("Codex CLI");
    expect(summary.tokens).toBe(1000);
    expect(summary.costMicros).toBe(1_500_000);
  });

  it("names an account and reads its own daily series", () => {
    const work = profile("work", "Work");
    const data = analytics({
      providers: [
        provider({
          accounts: [
            {
              ...account("work", 900),
              daily: [{ date: "2026-08-30", tokens: 900, cost_micros: 900_000 }],
              worked_secs: 120,
            },
            account(null, 100),
          ],
        }),
      ],
    });
    const summary = scopedSummary(
      data,
      providers(data),
      { provider: "claude", profileId: "work" },
      [work],
    );
    expect(summary.label).toBe("Claude Code · Work");
    expect(summary.tokens).toBe(900);
    expect(summary.workedSecs).toBe(120);
    expect(summary.activeDays).toBe(1);
  });

  it("reports an unknown scope as zeroes rather than another scope's figures", () => {
    const data = analytics();
    expect(scopedSummary(data, providers(data), { provider: "grok" }, []).tokens).toBe(0);
    expect(
      scopedSummary(data, providers(data), { provider: "claude", profileId: "gone" }, []).tokens,
    ).toBe(0);
  });
});

describe("statsChart", () => {
  const work = profile("work", "Work");
  const data = analytics({
    providers: [
      provider({
        provider_id: "claude",
        tokens: { input: 15, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
        cost_micros: 1500,
        accounts: [
          {
            ...account("work", 5),
            cost_micros: 500,
            daily: [{ date: "2026-08-30", tokens: 5, cost_micros: 500 }],
          },
        ],
        daily: [
          { date: "2026-08-29", tokens: 10, cost_micros: 1000 },
          { date: "2026-08-30", tokens: 5, cost_micros: 500 },
        ],
      }),
      provider({
        provider_id: "codex",
        tokens: { input: 6, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
        cost_micros: 200,
        daily: [{ date: "2026-08-30", tokens: 6, cost_micros: 200 }],
      }),
    ],
  });
  const providers = statsProviders(data, [CLAUDE, CODEX]);

  it("draws every provider by default, filling the quiet days", () => {
    const chart = statsChart(providers, [], { provider: null }, "tokens", 3, TODAY);
    expect(chart.dates).toEqual(["2026-08-28", "2026-08-29", "2026-08-30"]);
    expect(chart.series.map((series) => [series.name, series.tokens])).toEqual([
      ["Claude Code", 15],
      ["Codex CLI", 6],
    ]);
    expect(chart.series[0]?.points.map((point) => point.tokens)).toEqual([0, 10, 5]);
  });

  it("switches the ranking to cost for the cost metric", () => {
    const chart = statsChart(providers, [], { provider: null }, "cost", 3, TODAY);
    expect(chart.series.map((series) => [series.name, series.costMicros])).toEqual([
      ["Claude Code", 1500],
      ["Codex CLI", 200],
    ]);
  });

  it("does not recolor a provider when the metric changes", () => {
    const tokens = statsChart(providers, [], { provider: null }, "tokens", 3, TODAY);
    const cost = statsChart(providers, [], { provider: null }, "cost", 3, TODAY);
    expect(tokens.series.map((series) => [series.key, series.colorSlot])).toEqual(
      cost.series.map((series) => [series.key, series.colorSlot]),
    );
  });

  it("draws one provider's own series when filtered", () => {
    const chart = statsChart(providers, [], { provider: "codex" }, "tokens", 3, TODAY);
    expect(chart.series.map((series) => series.tokens)).toEqual([6]);
  });

  it("draws one account's own series and names it", () => {
    const chart = statsChart(
      providers,
      [work],
      { provider: "claude", profileId: "work" },
      "tokens",
      3,
      TODAY,
    );
    expect(chart.series.map((series) => [series.name, series.tokens])).toEqual([
      ["Claude Code · Work", 5],
    ]);
  });

  it("never falls back to the total for provider data it does not have", () => {
    const chart = statsChart(providers, [], { provider: "grok" }, "tokens", 3, TODAY);
    expect(chart.series).toEqual([]);
  });
});

describe("chartPeak", () => {
  it("sums the visible series per day", () => {
    const data = analytics({
      providers: [
        provider({ provider_id: "claude", daily: [{ date: "2026-08-30", tokens: 10 }] }),
        provider({ provider_id: "codex", daily: [{ date: "2026-08-30", tokens: 5 }] }),
      ],
    });
    const providers = statsProviders(data, [CLAUDE, CODEX]);
    const chart = statsChart(providers, [], { provider: null }, "tokens", 3, TODAY);
    expect(chartPeak(chart, "tokens")).toEqual({ date: "2026-08-30", value: 15 });
  });

  it("is null when every visible day is silent", () => {
    const chart = { dates: ["2026-08-30"], series: [] };
    expect(chartPeak(chart, "tokens")).toBeNull();
  });
});

describe("chartScale", () => {
  it("rounds the top to a readable step", () => {
    const scale = chartScale(3_900_000_000);
    expect(scale.max).toBe(4_000_000_000);
    expect(scale.ticks).toEqual([0, 1_000_000_000, 2_000_000_000, 3_000_000_000, 4_000_000_000]);
  });

  it("keeps a zero baseline with nothing counted", () => {
    expect(chartScale(0)).toEqual({ max: 0, ticks: [0] });
  });
});

describe("tokenMix", () => {
  it("divides the billed total and excludes reasoning from the segments", () => {
    const mix = tokenMix({
      input: 100,
      output: 200,
      cache_write: 100,
      cache_read: 600,
      reasoning: 50,
    });
    expect(mix.map((segment) => [segment.key, segment.percent])).toEqual([
      ["input", 10],
      ["output", 20],
      ["cache_read", 60],
      ["cache_write", 10],
    ]);
  });

  it("is zeroes with nothing counted", () => {
    const mix = tokenMix({ input: 0, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 });
    expect(mix.every((segment) => segment.percent === 0)).toBe(true);
  });
});

describe("providerShare", () => {
  it("is the provider's slice of every provider's tokens", () => {
    const big = provider({
      tokens: { input: 900, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
    });
    const small = provider({
      tokens: { input: 100, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
    });
    const data = analytics({ providers: [big, small] });
    expect(providerShare(big, data)).toBe(90);
    expect(providerShare(small, data)).toBe(10);
    expect(providerShareLabel(big, data)).toBe("90%");
  });

  it("keeps a real sliver visible instead of calling it zero", () => {
    const big = provider({
      tokens: { input: 1000, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
    });
    const sliver = provider({
      tokens: { input: 1, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
    });
    const data = analytics({ providers: [big, sliver] });
    expect(providerShare(sliver, data)).toBe(0);
    expect(providerShareLabel(sliver, data)).toBe("<1%");
  });

  it("is 0 rather than NaN when nothing was counted", () => {
    const empty = provider({
      tokens: { input: 0, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
    });
    expect(providerShare(empty, analytics({ providers: [empty] }))).toBe(0);
  });
});

describe("statsAccounts", () => {
  it("names each account by its profile and the default one as Default account", () => {
    const rows = statsAccounts(
      provider({
        tokens: { input: 1000, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
        accounts: [account("work", 750), account(null, 250)],
      }),
      [profile("work", "Work")],
    );
    expect(rows.map((row) => [row.name, row.share])).toEqual([
      ["Work", "75%"],
      ["Default account", "25%"],
    ]);
  });

  it("shows nothing for a lone default account", () => {
    expect(statsAccounts(provider({ accounts: [account(null, 10)] }), [])).toEqual([]);
    expect(statsAccounts(provider(), [])).toEqual([]);
  });

  it("keeps a lone named account and labels one whose profile was removed", () => {
    const rows = statsAccounts(provider({ accounts: [account("gone", 10)] }), []);
    expect(rows.map((row) => row.name)).toEqual(["Removed account"]);
  });
});

describe("accountScopes", () => {
  it("keeps every account, including the default one, as a filter entry", () => {
    const data = analytics({
      providers: [
        provider({
          accounts: [account("work", 750), account(null, 250)],
        }),
      ],
    });
    const scopes = accountScopes(statsProviders(data, [CLAUDE]), [profile("work", "Work")]);
    expect(scopes.map((scope) => [scope.key, scope.name])).toEqual([
      ["account:claude:work", "Work"],
      ["account:claude:default", "Default account"],
    ]);
  });

  it("names an account by its provider and profile", () => {
    expect(accountName(null, [])).toBe("Default account");
    expect(accountName("work", [profile("work", "Work")])).toBe("Work");
    expect(accountName("gone", [])).toBe("Removed account");
  });
});

describe("scope keys", () => {
  it("round-trips all, provider and account scopes", () => {
    for (const scope of [
      { provider: null },
      { provider: "claude" },
      { provider: "claude", profileId: null },
      { provider: "claude", profileId: "work" },
    ]) {
      expect(parseScopeKey(scopeKey(scope))).toEqual(scope);
    }
  });

  it("falls back to all for an unrecognised key", () => {
    expect(parseScopeKey("nonsense")).toEqual({ provider: null });
  });
});

describe("plan window pace", () => {
  it("reads the labels the providers actually send", () => {
    expect(windowDurationSecs("5h")).toBe(5 * 3600);
    expect(windowDurationSecs("week")).toBe(7 * 86_400);
    expect(windowDurationSecs("month")).toBe(30 * 86_400);
    expect(windowDurationSecs("whenever")).toBeNull();
  });

  it("places the even-pace tick where the window has elapsed", () => {
    const now = new Date("2026-10-05T12:00:00Z");
    expect(elapsedPercent("2026-10-05T15:00:00Z", "5h", now)).toBe(40);
    expect(elapsedPercent(null, "5h", now)).toBeNull();
    expect(elapsedPercent("2026-10-05T15:00:00Z", "whenever", now)).toBeNull();
  });

  it("calls spending ahead only outside a small tolerance", () => {
    expect(paceOf(60, 40)).toBe("ahead");
    expect(paceOf(41, 40)).toBe("on");
    expect(paceOf(20, 40)).toBe("under");
    expect(paceOf(20, null)).toBeNull();
  });
});

describe("limitGroups", () => {
  function reading(provider_id: string, profile_id: string | null): ProviderUsage {
    return {
      provider_id,
      profile_id,
      windows: [{ used_percent: 10, window: "5h", resets_at: null }],
      collected_at: "2026-10-05T12:00:00Z",
    };
  }

  it("groups accounts by provider in first-seen order", () => {
    const groups = limitGroups(
      [reading("claude", "work"), reading("codex", null), reading("claude", null)],
      [CLAUDE, CODEX],
    );
    expect(groups.map((group) => [group.name, group.readings.length])).toEqual([
      ["Claude Code", 2],
      ["Codex CLI", 1],
    ]);
  });
});

describe("formatting", () => {
  it("compacts tokens the way the providers quote them", () => {
    expect(compactTokens(2_200_000_000)).toBe("2.2B");
    expect(compactTokens(21_200_000)).toBe("21.2M");
    expect(compactTokens(694_700)).toBe("694.7k");
    expect(compactTokens(812)).toBe("812");
  });

  it("prints money with both decimals", () => {
    expect(money(1_526_250_000)).toBe("$1,526.25");
    expect(money(0)).toBe("$0.00");
  });

  it("compacts money for axes without losing its unit", () => {
    expect(compactMoney(2_000_000_000)).toBe("$2.0k");
    expect(compactMoney(25_000_000)).toBe("$25");
    expect(compactMoney(1_500_000)).toBe("$1.5");
    expect(compactMoney(0)).toBe("$0.00");
  });

  it("never rounds real activity down to 0%", () => {
    expect(percentLabel(2, 1000)).toBe("<1%");
    expect(percentLabel(0, 1000)).toBe("0%");
    expect(percentLabel(5, 0)).toBe("0%");
    expect(percentLabel(996, 1000)).toBe("100%");
  });

  it("says when a meter resets, and nothing once it has", () => {
    const now = new Date("2026-10-05T12:00:00Z");
    expect(resetsIn("2026-10-05T15:12:00Z", now)).toBe("resets in 3h 12m");
    expect(resetsIn("2026-10-05T11:00:00Z", now)).toBeNull();
    expect(resetsIn(null, now)).toBeNull();
  });

  it("reads worked time at the scale it happened", () => {
    expect(workedLabel(0)).toBe("—");
    expect(workedLabel(90)).toBe("1m");
    expect(workedLabel(3600 * 4 + 60 * 20)).toBe("4h 20m");
    expect(workedLabel(86_400 * 35 + 3600 * 4)).toBe("35d 4h");
  });

  it("says — for a date it cannot read", () => {
    expect(dayLabel(null)).toBe("—");
    expect(dayLabel("not a date")).toBe("—");
  });
});
