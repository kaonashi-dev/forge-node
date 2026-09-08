import { describe, expect, it } from "vitest";
import type { ProviderAnalytics, UsageAnalytics } from "../workbench/types";
import {
  busiestDay,
  compactTokens,
  dayLabel,
  heatmap,
  money,
  overview,
  providerShare,
  tokenMix,
  totals,
  trackingSince,
  workedLabel,
} from "./usageStats";

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

describe("heatmap", () => {
  const today = new Date("2026-08-30T12:00:00Z");

  it("fills the days the daemon left out, because absent means zero", () => {
    const cells = heatmap([{ date: "2026-08-30", tokens: 100 }], 3, today);
    expect(cells.map((cell) => cell.date)).toEqual(["2026-08-28", "2026-08-29", "2026-08-30"]);
    expect(cells.map((cell) => cell.tokens)).toEqual([0, 0, 100]);
  });

  it("levels are quarters of the busiest day, not absolute counts", () => {
    const cells = heatmap(
      [
        { date: "2026-08-28", tokens: 100 },
        { date: "2026-08-29", tokens: 50 },
        { date: "2026-08-30", tokens: 1 },
      ],
      3,
      today,
    );
    expect(cells.map((cell) => cell.level)).toEqual([4, 2, 1]);
  });

  it("gives a silent day level 0 and a busy day never less than 1", () => {
    const cells = heatmap(
      [
        { date: "2026-08-30", tokens: 1_000_000 },
        { date: "2026-08-29", tokens: 1 },
      ],
      2,
      today,
    );
    expect(cells.map((cell) => cell.level)).toEqual([1, 4]);
  });

  it("is all zeroes rather than empty when nothing ran", () => {
    const cells = heatmap([], 5, today);
    expect(cells).toHaveLength(5);
    expect(cells.every((cell) => cell.level === 0)).toBe(true);
  });
});

describe("busiestDay", () => {
  it("names the best day", () => {
    expect(
      busiestDay([
        { date: "2026-08-28", tokens: 10 },
        { date: "2026-08-30", tokens: 99 },
      ]),
    ).toEqual({ date: "2026-08-30", tokens: 99 });
  });

  it("is null when every day is silent", () => {
    expect(busiestDay([{ date: "2026-08-28", tokens: 0 }])).toBeNull();
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
  });

  it("is 0 rather than NaN when nothing was counted", () => {
    const empty = provider({
      tokens: { input: 0, output: 0, cache_write: 0, cache_read: 0, reasoning: 0 },
    });
    expect(providerShare(empty, analytics({ providers: [empty] }))).toBe(0);
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
    expect(money(1_526_250_000)).toBe("$1526.25");
    expect(money(0)).toBe("$0.00");
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
