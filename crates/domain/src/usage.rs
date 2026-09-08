//! Aggregated agent usage analytics (§16.2).
//!
//! [`ProviderUsage`](crate::ProviderUsage) answers "how much of this month's
//! allowance is gone" — one meter per rolling window, read from the provider's
//! own endpoint. These types answer a different question: "what have the agents
//! on this machine actually *done*", counted in tokens, turns and days from the
//! transcripts the CLIs already write to disk.
//!
//! The two never merge. A subscription meter is the provider's word and cannot
//! be recomputed; an analytics reading is ours and is only ever as complete as
//! the transcripts still on disk. Keeping them apart is what lets the settings
//! page show a meter at 55% next to a token count that covers thirty days.
//!
//! Every number here is an integer: the protocol stays `Eq` (no float in a wire
//! type) and money is counted in micro-dollars, never in `f64`.

use serde::{Deserialize, Serialize};

use crate::{AgentProviderId, Timestamp};

/// One dollar, in the micro-dollar unit costs are counted in.
pub const MICROS_PER_USD: u64 = 1_000_000;

/// Token counts, split the way the providers report them (§16.2).
///
/// `reasoning` is a *subset* of `output` — thinking tokens are output tokens
/// that were billed as output — so it is never added into [`Self::total`]. The
/// other four are disjoint and do add up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenTotals {
    /// Fresh input tokens: what was sent and not served from cache.
    pub input: u64,
    /// Generated tokens, thinking included.
    pub output: u64,
    /// Tokens written into the prompt cache, billed at a premium.
    pub cache_write: u64,
    /// Tokens served from the prompt cache, billed at a discount.
    pub cache_read: u64,
    /// The thinking part of `output`, when the provider breaks it out.
    pub reasoning: u64,
}

impl TokenTotals {
    /// Every billed token: input, output and both halves of the cache.
    ///
    /// `reasoning` is deliberately absent — it is already inside `output`, and
    /// counting it twice is the easiest way to publish a wrong number.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    /// Share of the total that came out of (or went into) the prompt cache, as
    /// a whole percent. `0` when nothing was counted at all.
    #[must_use]
    pub fn cache_percent(&self) -> u8 {
        let total = self.total();
        if total == 0 {
            return 0;
        }
        let cached = self.cache_read + self.cache_write;
        u8::try_from((cached * 100) / total).unwrap_or(100).min(100)
    }

    /// Accumulate another reading into this one.
    pub fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.reasoning += other.reasoning;
    }
}

/// What one provider's transcripts add up to over the scanned window (§16.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAnalytics {
    pub provider_id: AgentProviderId,
    pub tokens: TokenTotals,
    /// Transcripts that carried at least one billed turn.
    pub sessions: u32,
    /// Billed turns across those transcripts — one per assistant reply.
    pub turns: u32,
    /// Estimated spend in micro-dollars, over the turns whose model has a
    /// published price. Turns on an unpriced model contribute nothing, which is
    /// what [`Self::unpriced_turns`] exists to say out loud.
    pub cost_micros: u64,
    /// Billed turns whose model carries no price in this build. A non-zero
    /// count means [`Self::cost_micros`] is a floor, not an estimate.
    pub unpriced_turns: u32,
    /// The model that produced the most tokens here, as the provider names it.
    pub top_model: Option<String>,
    /// Wall-clock time covered by the scanned runs: the sum of each
    /// transcript's first-to-last span. Two agents working in parallel are
    /// counted twice, on purpose — it measures agent time, not yours.
    pub worked_secs: u64,
    pub first_activity: Option<Timestamp>,
    pub last_activity: Option<Timestamp>,
}

/// One day's token total, keyed by its UTC calendar date (`YYYY-MM-DD`).
///
/// A day with no activity is absent rather than zero: the series is a list of
/// facts, and the view fills the gaps when it draws a calendar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub tokens: u64,
}

/// Everything the settings screen's stats page reads (§16.2).
///
/// Built by scanning the provider transcript stores; see
/// `agents::collect_analytics`. An empty `providers` means no transcript in the
/// window carried a usage record — never that the agents did nothing, which is
/// why `scanned`/`skipped` travel with the numbers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAnalytics {
    /// One entry per provider that reported anything, in descending token order.
    pub providers: Vec<ProviderAnalytics>,
    /// Daily totals over the window, oldest first, activity-only.
    pub daily: Vec<DailyUsage>,
    /// How far back the scan reached, in days.
    pub window_days: u16,
    /// Transcripts actually read.
    pub scanned: u32,
    /// Transcripts inside the window that the per-provider cap left unread. A
    /// non-zero count is shown: a bounded scan that looks exhaustive is worse
    /// than one that admits its bound.
    pub skipped: u32,
    pub collected_at: Timestamp,
}

impl UsageAnalytics {
    /// An answer that reports nothing, taken now.
    #[must_use]
    pub fn empty(window_days: u16) -> Self {
        Self {
            providers: Vec::new(),
            daily: Vec::new(),
            window_days,
            scanned: 0,
            skipped: 0,
            collected_at: Timestamp::now(),
        }
    }

    /// Tokens across every provider.
    #[must_use]
    pub fn tokens(&self) -> TokenTotals {
        let mut totals = TokenTotals::default();
        for provider in &self.providers {
            totals.add(&provider.tokens);
        }
        totals
    }

    /// Estimated spend across every provider, in micro-dollars.
    #[must_use]
    pub fn cost_micros(&self) -> u64 {
        self.providers.iter().map(|p| p.cost_micros).sum()
    }

    /// Whether any counted turn ran on a model with no published price, so the
    /// view can say the cost is partial instead of quietly under-reporting.
    #[must_use]
    pub fn has_unpriced(&self) -> bool {
        self.providers.iter().any(|p| p.unpriced_turns > 0)
    }

    /// Days with any activity in the window.
    #[must_use]
    pub fn active_days(&self) -> u32 {
        u32::try_from(self.daily.len()).unwrap_or(u32::MAX)
    }

    /// The busiest day in the window, if there was one.
    #[must_use]
    pub fn busiest_day(&self) -> Option<&DailyUsage> {
        self.daily.iter().max_by_key(|day| day.tokens)
    }

    /// Transcripts counted across every provider.
    #[must_use]
    pub fn sessions(&self) -> u32 {
        self.providers.iter().map(|p| p.sessions).sum()
    }

    /// Billed turns across every provider.
    #[must_use]
    pub fn turns(&self) -> u32 {
        self.providers.iter().map(|p| p.turns).sum()
    }

    /// Agent wall-clock across every provider, in seconds.
    #[must_use]
    pub fn worked_secs(&self) -> u64 {
        self.providers.iter().map(|p| p.worked_secs).sum()
    }

    /// The earliest activity any provider recorded, which is what "tracking
    /// since" means: the window is a bound, the data may not fill it.
    #[must_use]
    pub fn tracking_since(&self) -> Option<Timestamp> {
        self.providers.iter().filter_map(|p| p.first_activity).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(input: u64, output: u64, cache_write: u64, cache_read: u64) -> TokenTotals {
        TokenTotals {
            input,
            output,
            cache_write,
            cache_read,
            reasoning: 0,
        }
    }

    #[test]
    fn total_excludes_reasoning_because_it_is_already_output() {
        let tokens = TokenTotals {
            input: 10,
            output: 100,
            cache_write: 5,
            cache_read: 85,
            reasoning: 60,
        };
        assert_eq!(tokens.total(), 200);
    }

    #[test]
    fn cache_percent_is_a_whole_percent_and_zero_when_empty() {
        assert_eq!(totals(10, 10, 0, 80).cache_percent(), 80);
        assert_eq!(TokenTotals::default().cache_percent(), 0);
    }

    #[test]
    fn analytics_sums_across_providers() {
        let analytics = UsageAnalytics {
            providers: vec![
                ProviderAnalytics {
                    provider_id: AgentProviderId::new("claude"),
                    tokens: totals(1, 2, 3, 4),
                    sessions: 2,
                    turns: 9,
                    cost_micros: 1_500_000,
                    unpriced_turns: 0,
                    top_model: Some("claude-opus-5".to_owned()),
                    worked_secs: 60,
                    first_activity: Timestamp::from_unix_secs(1_000),
                    last_activity: Timestamp::from_unix_secs(2_000),
                },
                ProviderAnalytics {
                    provider_id: AgentProviderId::new("codex"),
                    tokens: totals(1, 1, 1, 1),
                    sessions: 1,
                    turns: 3,
                    cost_micros: 0,
                    unpriced_turns: 3,
                    top_model: None,
                    worked_secs: 30,
                    first_activity: Timestamp::from_unix_secs(500),
                    last_activity: Timestamp::from_unix_secs(900),
                },
            ],
            daily: vec![
                DailyUsage {
                    date: "2026-08-25".to_owned(),
                    tokens: 10,
                },
                DailyUsage {
                    date: "2026-08-26".to_owned(),
                    tokens: 4,
                },
            ],
            window_days: 30,
            scanned: 3,
            skipped: 0,
            collected_at: Timestamp::now(),
        };

        assert_eq!(analytics.tokens().total(), 14);
        assert_eq!(analytics.cost_micros(), 1_500_000);
        assert!(analytics.has_unpriced());
        assert_eq!(analytics.active_days(), 2);
        assert_eq!(analytics.busiest_day().map(|day| day.tokens), Some(10));
        assert_eq!(analytics.sessions(), 3);
        assert_eq!(analytics.turns(), 12);
        assert_eq!(analytics.worked_secs(), 90);
        assert_eq!(analytics.tracking_since(), Timestamp::from_unix_secs(500));
    }

    #[test]
    fn empty_analytics_reports_nothing_rather_than_zeros_with_meaning() {
        let analytics = UsageAnalytics::empty(30);
        assert_eq!(analytics.tokens().total(), 0);
        assert_eq!(analytics.active_days(), 0);
        assert!(analytics.busiest_day().is_none());
        assert!(analytics.tracking_since().is_none());
        assert!(!analytics.has_unpriced());
    }
}
