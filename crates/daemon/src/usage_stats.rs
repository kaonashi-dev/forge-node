//! Caching in front of the transcript token scan (§16.2).
//!
//! `agents::collect_analytics` reads every recent transcript on disk line by
//! line — hundreds of megabytes on a machine that has been busy for a month.
//! That is far too much IO to repeat because a settings page re-rendered, and
//! far too much to do while holding the core lock, so it gets the same
//! treatment as the pull-request listing: its own mutex, a short TTL, and a
//! result keyed by the question that produced it.
//!
//! The key is the window, not the clock: asking for 30 days and then for 7 must
//! not answer the second question with the first one's numbers.

use std::time::{Duration, Instant};

use domain::UsageAnalytics;

/// How long a completed scan answers the same window.
///
/// Longer than the transcript-discovery TTL (10 s) on purpose: this scan is an
/// order of magnitude more IO, and a token total that is a minute stale is
/// indistinguishable from a fresh one on screen.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// The last completed scan, reusable until it goes stale.
#[derive(Default)]
pub struct Cache {
    scanned_at: Option<Instant>,
    window_days: u16,
    analytics: Option<UsageAnalytics>,
}

impl Cache {
    /// Return the last scan when it answered this same window recently.
    #[must_use]
    pub fn fresh(&self, window_days: u16) -> Option<UsageAnalytics> {
        self.fresh_at(window_days, Instant::now())
    }

    /// Store a completed scan for `window_days`.
    pub fn update(&mut self, window_days: u16, analytics: UsageAnalytics) {
        self.update_at(window_days, analytics, Instant::now());
    }

    fn fresh_at(&self, window_days: u16, now: Instant) -> Option<UsageAnalytics> {
        let fresh = self.window_days == window_days
            && self
                .scanned_at
                .is_some_and(|at| now.saturating_duration_since(at) < CACHE_TTL);
        fresh.then(|| self.analytics.clone()).flatten()
    }

    fn update_at(&mut self, window_days: u16, analytics: UsageAnalytics, now: Instant) {
        self.window_days = window_days;
        self.analytics = Some(analytics);
        self.scanned_at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analytics(scanned: u32) -> UsageAnalytics {
        UsageAnalytics {
            scanned,
            ..UsageAnalytics::empty(30)
        }
    }

    #[test]
    fn an_empty_cache_answers_nothing() {
        assert!(Cache::default().fresh(30).is_none());
    }

    #[test]
    fn a_recent_scan_answers_the_same_window() {
        let mut cache = Cache::default();
        cache.update(30, analytics(7));
        assert_eq!(cache.fresh(30).map(|a| a.scanned), Some(7));
    }

    #[test]
    fn a_different_window_is_a_different_question() {
        let mut cache = Cache::default();
        cache.update(30, analytics(7));
        assert!(cache.fresh(7).is_none());
    }

    #[test]
    fn a_stale_scan_is_not_reused() {
        let now = Instant::now();
        let mut cache = Cache::default();
        cache.update_at(30, analytics(7), now - CACHE_TTL * 2);
        assert!(cache.fresh_at(30, now).is_none());
    }
}
