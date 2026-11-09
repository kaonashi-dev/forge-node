//! Settings › Stats & usage. Two readings that never merge: `UsageAnalytics`
//! is our count of what was spent, off the transcripts on disk, and a
//! `ProviderUsage` is the provider's word on what allowance is left. Numbers
//! are worded as Tauri's `usageStats.ts` words them.

use domain::{ProviderUsage, UsageAnalytics};
use rune::{El, col, el, row, text};
use rune_kit::{Kit, SegmentedControl};

use crate::app::AppState;

const MICROS_PER_USD: f64 = 1_000_000.0;
pub const WINDOWS: [u16; 3] = [7, 30, 90];
const CHART_H: f32 = 80.0;

pub struct Usage {
    pub window_days: u16,
    pub analytics: Option<UsageAnalytics>,
    pub error: Option<String>,
    /// The window asked for and not answered yet.
    pub pending: Option<u16>,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            window_days: 30,
            analytics: None,
            error: None,
            pending: None,
        }
    }
}

/// `1.2M`, as Tauri's `compactTokens`.
#[must_use]
pub fn compact_tokens(value: u64) -> String {
    let v = value as f64;
    if value >= 1_000_000_000 {
        format!("{:.1}B", v / 1e9)
    } else if value >= 1_000_000 {
        format!("{:.1}M", v / 1e6)
    } else if value >= 1000 {
        format!("{:.1}k", v / 1e3)
    } else {
        value.to_string()
    }
}

/// Micro-dollars as `$1,234.56`: two decimals, because it is money.
#[must_use]
pub fn money(micros: u64) -> String {
    let cents = (micros as f64 / MICROS_PER_USD * 100.0).round() as u64;
    let (dollars, cents) = (cents / 100, cents % 100);
    let digits = dollars.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("${grouped}.{cents:02}")
}

pub struct Overview {
    pub tokens: u64,
    pub cost_micros: u64,
    pub unpriced_turns: u32,
    pub active_days: usize,
    pub cache_percent: u64,
    pub sessions: u32,
}

#[must_use]
pub fn overview(analytics: &UsageAnalytics) -> Overview {
    let (mut input, mut output, mut write, mut read) = (0u64, 0u64, 0u64, 0u64);
    for provider in &analytics.providers {
        input += provider.tokens.input;
        output += provider.tokens.output;
        write += provider.tokens.cache_write;
        read += provider.tokens.cache_read;
    }
    // Reasoning is already inside output, so it is never added again.
    let tokens = input + output + write + read;
    Overview {
        tokens,
        cost_micros: analytics.providers.iter().map(|p| p.cost_micros).sum(),
        unpriced_turns: analytics.providers.iter().map(|p| p.unpriced_turns).sum(),
        active_days: analytics.daily.len(),
        cache_percent: ((write + read) * 100)
            .checked_div(tokens)
            .map_or(0, |percent| percent.min(100)),
        sessions: analytics.providers.iter().map(|p| p.sessions).sum(),
    }
}

fn tile(k: &Kit, label: &str, value: String, note: String) -> El {
    col()
        .gap(2.0)
        .padding(12.0)
        .radius(8.0)
        .fill(k.c.surface_hover)
        .grow(1.0)
        .child(
            text(label.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        )
        .child(text(value).font(k.ty.body_medium).color(k.c.text_base))
        .child(text(note).font(k.ty.text_xsmall).color(k.c.text_faint))
}

fn limits(usage: &[ProviderUsage], k: &Kit) -> El {
    let mut view = col().key("plan-limits").gap(4.0).child(
        text("Plan limits")
            .font(k.ty.small_medium)
            .color(k.c.text_base),
    );
    if usage.is_empty() {
        return view.child(
            text("No provider has reported a subscription allowance yet.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    for reading in usage {
        for window in &reading.windows {
            view = view.child(
                row()
                    .gap(8.0)
                    .child(
                        text(format!("{} · {}", reading.provider_id, window.window))
                            .font(k.ty.small)
                            .color(k.c.text_muted)
                            .grow(1.0),
                    )
                    .child(
                        text(format!("{}% used", window.used_percent))
                            .font(k.ty.small)
                            .color(k.c.text_base),
                    ),
            );
        }
    }
    view
}

pub fn section(s: &AppState, k: &Kit) -> El {
    let usage = &s.usage;
    let labels: Vec<String> = WINDOWS.iter().map(|days| format!("{days}d")).collect();
    let selected = WINDOWS.iter().position(|days| *days == usage.window_days);
    let windows = SegmentedControl::new(&labels, selected)
        .label("Analytics window")
        .fit()
        .on_pick(|s: &mut AppState, index: usize, _tx| {
            if let Some(days) = WINDOWS.get(index) {
                s.usage.window_days = *days;
                s.usage.analytics = None;
            }
        })
        .build(k)
        .key("usage-window");
    let mut view = col()
        .key("usage")
        .gap(12.0)
        .child(
            text("Stats & usage")
                .font(k.ty.body_medium)
                .color(k.c.text_base),
        )
        .child(
            text(
                "Token analytics counted off agent transcripts on disk — separate from \
                 subscription meters, which are the provider's word on allowance.",
            )
            .font(k.ty.small)
            .color(k.c.text_faint),
        )
        .child(windows);
    let Some(analytics) = &usage.analytics else {
        let note = usage
            .error
            .clone()
            .unwrap_or_else(|| "Loading analytics…".to_string());
        return view.child(text(note).font(k.ty.small).color(k.c.text_faint));
    };
    let summary = overview(analytics);
    let cost_note = if summary.unpriced_turns > 0 {
        format!("floor · {} unpriced turns", summary.unpriced_turns)
    } else {
        "API equivalent".to_string()
    };
    view = view.child(
        row()
            .key("usage-tiles")
            .gap(8.0)
            .child(tile(
                k,
                "Tokens",
                compact_tokens(summary.tokens),
                format!("{}% from cache", summary.cache_percent),
            ))
            .child(tile(k, "Est. cost", money(summary.cost_micros), cost_note))
            .child(tile(
                k,
                "Agent runs",
                summary.sessions.to_string(),
                String::new(),
            ))
            .child(tile(
                k,
                "Active days",
                summary.active_days.to_string(),
                format!("of {}", analytics.window_days),
            )),
    );
    let peak = analytics
        .daily
        .iter()
        .map(|day| day.tokens)
        .max()
        .unwrap_or(0)
        .max(1);
    let mut bars = row()
        .key("usage-daily")
        .role("img")
        .label(format!("Daily tokens over {} days", analytics.window_days))
        .h(CHART_H)
        .gap(2.0)
        .items_end();
    for day in &analytics.daily {
        let height = (day.tokens as f32 / peak as f32 * CHART_H).max(1.0);
        bars = bars.child(el().w(6.0).h(height).radius(1.0).fill(k.c.text_accent));
    }
    view = view
        .child(
            text("Daily tokens")
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(bars);
    let mut providers = col().key("usage-providers").gap(4.0).child(
        text("By provider")
            .font(k.ty.small_medium)
            .color(k.c.text_base),
    );
    if analytics.providers.is_empty() {
        providers = providers.child(
            text("No transcript in the window carried a usage record.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    for provider in &analytics.providers {
        let tokens = provider.tokens.input
            + provider.tokens.output
            + provider.tokens.cache_write
            + provider.tokens.cache_read;
        providers = providers.child(
            row()
                .gap(12.0)
                .child(
                    text(provider.provider_id.to_string())
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .grow(1.0),
                )
                .child(
                    text(compact_tokens(tokens))
                        .font(k.ty.small)
                        .color(k.c.text_muted),
                )
                .child(
                    text(money(provider.cost_micros))
                        .font(k.ty.small)
                        .color(k.c.text_muted),
                )
                .child(
                    text(format!("{} runs", provider.sessions))
                        .font(k.ty.small)
                        .color(k.c.text_faint),
                ),
        );
    }
    if analytics.skipped > 0 {
        providers = providers.child(
            text(format!(
                "{} transcripts skipped: the scan is bounded, so this is not every file.",
                analytics.skipped
            ))
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint),
        );
    }
    let usage_readings = s
        .store
        .as_ref()
        .map_or(&[][..], |store| store.usage.as_slice());
    view.child(providers).child(limits(usage_readings, k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_worded_as_usage_stats_words_them() {
        assert_eq!(compact_tokens(999), "999");
        assert_eq!(compact_tokens(1_260), "1.3k");
        assert_eq!(compact_tokens(3_400_000), "3.4M");
        assert_eq!(compact_tokens(2_000_000_000), "2.0B");
        assert_eq!(money(0), "$0.00");
        assert_eq!(money(1_234_567_890), "$1,234.57");
        assert_eq!(money(5_000), "$0.01");
    }
}
