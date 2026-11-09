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

/// The share of billed tokens served from the cache, whole and at most 100.
#[must_use]
pub fn cache_percent(tokens: &domain::TokenTotals) -> u64 {
    let cached = tokens.cache_write + tokens.cache_read;
    let total = tokens.input + tokens.output + cached;
    (cached * 100)
        .checked_div(total)
        .map_or(0, |percent| percent.min(100))
}

/// A UTC calendar date (`YYYY-MM-DD`) from days since 1970-01-01.
fn iso_date(days: i64) -> String {
    // Hinnant's civil-from-days, so no date crate is needed for a label.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[must_use]
pub fn today_utc() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| (since.as_secs() / 86_400) as i64)
}

/// One bar per day of the window ending `today`, as `dailyCells`: the daemon
/// leaves quiet days out, and a missing day is a zero, not a gap to close.
#[must_use]
pub fn daily_cells(
    daily: &[domain::DailyUsage],
    window_days: u16,
    today: i64,
) -> Vec<(String, u64)> {
    let days = i64::from(window_days.clamp(1, 366));
    (0..days)
        .rev()
        .map(|back| {
            let date = iso_date(today - back);
            let tokens = daily
                .iter()
                .find(|day| day.date == date)
                .map_or(0, |day| day.tokens);
            (date, tokens)
        })
        .collect()
}

/// `part` of `total` as a whole percent, as Tauri's `percentLabel`: a real
/// sliver reads `<1%` rather than `0%`.
#[must_use]
pub fn percent_label(part: u64, total: u64) -> String {
    if total == 0 || part == 0 {
        return "0%".into();
    }
    let percent = part as f64 * 100.0 / total as f64;
    if percent < 1.0 {
        "<1%".into()
    } else {
        format!("{}%", percent.round() as u64)
    }
}

/// `35d 4h`, `4h 20m`, `12m`, as Tauri's `workedLabel`; `—` for nothing.
#[must_use]
pub fn worked_label(secs: i64) -> String {
    if secs <= 0 {
        return "—".into();
    }
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if days > 0 {
        return if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        };
    }
    if hours > 0 {
        return if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        };
    }
    format!("{}m", minutes.max(1))
}

/// `resets in 3h 12m`, or `None` when the provider gave no reset or it has
/// passed, as Tauri's `resetsIn`.
#[must_use]
pub fn resets_in(at: Option<&domain::Timestamp>, now: &domain::Timestamp) -> Option<String> {
    let secs = (at?.0 - now.0).whole_seconds();
    (secs > 0).then(|| format!("resets in {}", worked_label(secs)))
}

/// How long a window lasts, from the label the provider gave it, as Tauri's
/// `windowDurationSecs`. Only a label it recognizes gets a pace mark: seven
/// invented days for an unknown window would measure nothing.
#[must_use]
pub fn window_duration_secs(label: &str) -> Option<f64> {
    let normalized = label.trim().to_ascii_lowercase();
    let number = |suffix: char| {
        normalized
            .strip_suffix(suffix)
            .filter(|digits| !digits.is_empty() && !digits.starts_with('.'))
            .and_then(|digits| digits.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    if let Some(hours) = number('h') {
        return Some(hours * 3600.0);
    }
    if let Some(days) = number('d') {
        return Some(days * 86_400.0);
    }
    match normalized.as_str() {
        "day" | "daily" => Some(86_400.0),
        "week" | "weekly" => Some(7.0 * 86_400.0),
        "month" | "monthly" => Some(30.0 * 86_400.0),
        _ => None,
    }
}

/// How far through its window a meter is, as a whole percent, or `None`
/// when the window's start cannot be read (`elapsedPercent`).
#[must_use]
pub fn elapsed_percent(
    resets_at: Option<&domain::Timestamp>,
    window: &str,
    now: &domain::Timestamp,
) -> Option<u8> {
    let duration = window_duration_secs(window)?;
    let left = (resets_at?.0 - now.0).as_seconds_f64();
    let elapsed = (duration - left) / duration;
    Some((elapsed.clamp(0.0, 1.0) * 100.0).round() as u8)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    Under,
    On,
    Ahead,
}

/// Spending faster, even with, or slower than the window elapses, with five
/// points of tolerance (`paceOf`).
#[must_use]
pub fn pace_of(used: u8, elapsed: Option<u8>) -> Option<Pace> {
    let (used, elapsed) = (i16::from(used), i16::from(elapsed?));
    Some(if used >= elapsed + 5 {
        Pace::Ahead
    } else if used <= elapsed - 5 {
        Pace::Under
    } else {
        Pace::On
    })
}

fn token_total(tokens: &domain::TokenTotals) -> u64 {
    tokens.input + tokens.output + tokens.cache_write + tokens.cache_read
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
    let mut sum = domain::TokenTotals::default();
    for provider in &analytics.providers {
        sum.input += provider.tokens.input;
        sum.output += provider.tokens.output;
        sum.cache_write += provider.tokens.cache_write;
        sum.cache_read += provider.tokens.cache_read;
    }
    // Reasoning is already inside output, so it is never added again.
    let tokens = sum.input + sum.output + sum.cache_write + sum.cache_read;
    Overview {
        tokens,
        cost_micros: analytics.providers.iter().map(|p| p.cost_micros).sum(),
        unpriced_turns: analytics.providers.iter().map(|p| p.unpriced_turns).sum(),
        active_days: analytics.daily.len(),
        cache_percent: cache_percent(&sum),
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
    let now = domain::Timestamp::now();
    for reading in usage {
        for window in &reading.windows {
            let used = window.used_percent.min(100);
            // Tauri's meter: amber from three quarters, red from nine tenths.
            let fill = if used >= 90 {
                k.c.danger.icon
            } else if used >= 75 {
                k.c.warning.icon
            } else {
                k.c.text_accent
            };
            let reset = resets_in(window.resets_at.as_ref(), &now).unwrap_or_default();
            let elapsed = elapsed_percent(window.resets_at.as_ref(), &window.window, &now);
            let pace = pace_of(used, elapsed);
            let mut detail = format!("{} {}: {used}% used", reading.provider_id, window.window);
            if let Some(elapsed) = elapsed {
                detail.push_str(&format!(", {elapsed}% of the window elapsed"));
            }
            if let Some(pace) = pace {
                detail.push_str(match pace {
                    Pace::Ahead => ", ahead of pace",
                    Pace::On => ", on pace",
                    Pace::Under => ", under pace",
                });
            }
            let mut track = row()
                .grow(1.0)
                .h(6.0)
                .radius(3.0)
                .fill(k.c.surface_hover)
                .child(
                    el().h_full()
                        .w_frac(f32::from(used) / 100.0)
                        .radius(3.0)
                        .fill(fill),
                );
            if let Some(elapsed) = elapsed {
                // Where even spending would be now, as `.stats-meter-elapsed`.
                track = track.child(
                    row()
                        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                        .a11y_hidden()
                        .child(el().w_frac(f32::from(elapsed) / 100.0))
                        .child(el().w(1.0).h_full().fill(k.c.text_muted)),
                );
            }
            let dot = match pace {
                Some(Pace::Ahead) => k.c.warning.icon,
                Some(Pace::Under) => k.c.success.icon,
                Some(Pace::On) => k.c.text_faint,
                None => rune::Color::TRANSPARENT,
            };
            view = view.child(
                row()
                    .key(format!("limit-{}-{}", reading.provider_id, window.window))
                    .role("meter")
                    .label(detail)
                    .gap(8.0)
                    .items_center()
                    .child(
                        text(format!("{} · {}", reading.provider_id, window.window))
                            .font(k.ty.small)
                            .color(k.c.text_muted)
                            .w(140.0)
                            .truncate(),
                    )
                    .child(track)
                    .child(
                        text(format!("{used}%"))
                            .font(k.ty.small)
                            .color(k.c.text_base)
                            .w(40.0),
                    )
                    .child(el().size(6.0, 6.0).radius(3.0).fill(dot).a11y_hidden())
                    .child(
                        text(reset)
                            .font(k.ty.text_xsmall)
                            .color(k.c.text_faint)
                            .w(120.0),
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
    let mut view = col().key("usage").gap(12.0).child(windows);
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
    let cells = daily_cells(&analytics.daily, analytics.window_days, today_utc());
    let peak = cells
        .iter()
        .map(|(_, tokens)| *tokens)
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
    let bar_w = if cells.len() > 45 { 3.0 } else { 6.0 };
    for (_, tokens) in &cells {
        let height = (*tokens as f32 / peak as f32 * CHART_H).max(1.0);
        bars = bars.child(el().w(bar_w).h(height).radius(1.0).fill(k.c.text_accent));
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
    let all_tokens: u64 = analytics
        .providers
        .iter()
        .map(|provider| token_total(&provider.tokens))
        .sum();
    for provider in &analytics.providers {
        let tokens = token_total(&provider.tokens);
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
                    text(percent_label(tokens, all_tokens))
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint),
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

    fn totals(input: u64, output: u64, write: u64, read: u64) -> domain::TokenTotals {
        domain::TokenTotals {
            input,
            output,
            cache_write: write,
            cache_read: read,
            reasoning: 0,
        }
    }

    fn analytics(providers: serde_json::Value, daily: serde_json::Value) -> UsageAnalytics {
        serde_json::from_value(serde_json::json!({
            "providers": providers, "daily": daily, "window_days": 30, "scanned": 1,
            "skipped": 0, "collected_at": "2026-08-30T00:00:00Z"
        }))
        .expect("analytics")
    }

    fn provider(unpriced: u32) -> serde_json::Value {
        serde_json::json!({
            "provider_id": "claude",
            "tokens": { "input": 100, "output": 200, "cache_write": 100, "cache_read": 600,
                        "reasoning": 50 },
            "sessions": 4, "turns": 40, "cost_micros": 1_500_000, "unpriced_turns": unpriced,
            "top_model": "claude-opus-5", "worked_secs": 3600,
            "first_activity": null, "last_activity": null, "accounts": []
        })
    }

    #[test]
    fn the_overview_counts_as_usage_stats_counts() {
        let data = analytics(serde_json::json!([provider(0)]), serde_json::json!([]));
        let summary = overview(&data);
        assert_eq!(
            summary.tokens, 1000,
            "reasoning is inside output, not added again"
        );
        assert_eq!(summary.cache_percent, 70);
        let data = analytics(
            serde_json::json!([provider(3), provider(4)]),
            serde_json::json!([
                { "date": "2026-08-29", "tokens": 10 },
                { "date": "2026-08-30", "tokens": 20 }
            ]),
        );
        let summary = overview(&data);
        assert_eq!(summary.unpriced_turns, 7);
        assert_eq!(summary.active_days, 2, "days with activity, not the window");
        let empty = overview(&analytics(serde_json::json!([]), serde_json::json!([])));
        assert_eq!(
            (
                empty.tokens,
                empty.cost_micros,
                empty.unpriced_turns,
                empty.active_days,
                empty.cache_percent
            ),
            (0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn the_cache_share_is_whole_capped_and_zero_when_empty() {
        assert_eq!(cache_percent(&totals(10, 10, 10, 70)), 80);
        assert_eq!(cache_percent(&totals(0, 0, 80, 20)), 100);
        assert_eq!(cache_percent(&totals(0, 0, 0, 0)), 0);
    }

    #[test]
    fn quiet_days_are_zero_bars_not_missing_ones() {
        assert_eq!(iso_date(0), "1970-01-01");
        assert_eq!(iso_date(20_695), "2026-08-30");
        let day = domain::DailyUsage {
            date: "2026-08-30".into(),
            tokens: 100,
            cost_micros: 250,
        };
        let cells = daily_cells(&[day], 3, 20_695);
        assert_eq!(
            cells,
            vec![
                ("2026-08-28".to_string(), 0),
                ("2026-08-29".to_string(), 0),
                ("2026-08-30".to_string(), 100)
            ]
        );
        let none = daily_cells(&[], 5, 20_695);
        assert_eq!(none.len(), 5);
        assert!(none.iter().all(|(_, tokens)| *tokens == 0));
    }

    #[test]
    fn limits_and_shares_are_worded_as_usage_stats_words_them() {
        assert_eq!(percent_label(2, 1000), "<1%");
        assert_eq!(percent_label(0, 1000), "0%");
        assert_eq!(percent_label(5, 0), "0%");
        assert_eq!(percent_label(996, 1000), "100%");
        let at = |stamp: &str| -> domain::Timestamp {
            serde_json::from_value(serde_json::json!(stamp)).expect("a timestamp")
        };
        let now = at("2026-10-05T12:00:00Z");
        assert_eq!(
            resets_in(Some(&at("2026-10-05T15:12:00Z")), &now).as_deref(),
            Some("resets in 3h 12m")
        );
        assert_eq!(
            resets_in(Some(&at("2026-10-05T11:00:00Z")), &now),
            None,
            "already reset"
        );
        assert_eq!(resets_in(None, &now), None);
        assert_eq!(worked_label(0), "—");
        assert_eq!(worked_label(90), "1m");
        assert_eq!(worked_label(3600 * 4 + 60 * 20), "4h 20m");
        assert_eq!(worked_label(86_400 * 35 + 3600 * 4), "35d 4h");
    }

    #[test]
    fn a_meter_paces_against_the_window_it_names() {
        assert_eq!(window_duration_secs("5h"), Some(5.0 * 3600.0));
        assert_eq!(window_duration_secs(" Week "), Some(7.0 * 86_400.0));
        assert_eq!(window_duration_secs("month"), Some(30.0 * 86_400.0));
        assert_eq!(window_duration_secs("2.5d"), Some(2.5 * 86_400.0));
        assert_eq!(window_duration_secs("whenever"), None);
        assert_eq!(window_duration_secs("h"), None);
        let at = |stamp: &str| -> domain::Timestamp {
            serde_json::from_value(serde_json::json!(stamp)).expect("a timestamp")
        };
        let now = at("2026-10-05T12:00:00Z");
        let reset = at("2026-10-05T15:00:00Z");
        assert_eq!(elapsed_percent(Some(&reset), "5h", &now), Some(40));
        assert_eq!(elapsed_percent(None, "5h", &now), None);
        assert_eq!(elapsed_percent(Some(&reset), "whenever", &now), None);
        assert_eq!(pace_of(60, Some(40)), Some(Pace::Ahead));
        assert_eq!(pace_of(41, Some(40)), Some(Pace::On));
        assert_eq!(pace_of(20, Some(40)), Some(Pace::Under));
        assert_eq!(pace_of(20, None), None);
    }
}
