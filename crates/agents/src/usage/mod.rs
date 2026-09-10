//! Reading account usage from a provider (§16.2).
//!
//! Every provider-specific fact stays in this crate (principle P2). A provider
//! declares *where* its usage comes from with a [`UsageSource`]; nothing else in
//! the workspace branches on provider id. Three sources exist today:
//!
//! - [`UsageSource::Cli`] — run a CLI that prints one documented JSON document.
//! - [`UsageSource::CodexOAuth`] / [`UsageSource::ClaudeOauth`] — read the
//!   provider's *existing* local OAuth credentials and call its usage endpoint,
//!   with no extra login (the Orca/CodexBar approach).
//!
//! Every failure — no source, not signed in, network down, an unexpected shape —
//! resolves to `None`: the provider is simply absent from the result, exactly as
//! before. An error here must never become a number on screen.

pub mod analytics;
mod claude;
mod cli;
mod codex;
mod http;
mod pricing;

use std::path::Path;

use domain::{
    AgentDescriptor, AgentProfileId, ProviderUsage, ResolvedEnvironment, Timestamp, UsageSource,
};
use serde_json::Value;

pub use analytics::collect as collect_analytics;
pub use http::{HttpClient, UreqClient};

/// One login a provider's usage can be read from (§13.4).
///
/// A provider is not one allowance. The default account is what an unmodified
/// CLI uses; a launch profile that moved the config directory is a second
/// login, with limits of its own, and a meter that showed only the first was
/// reporting somebody else's numbers.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsageAccount<'a> {
    /// The profile that owns this account, stamped onto the reading. `None` is
    /// the provider's default account.
    pub profile_id: Option<AgentProfileId>,
    /// Where that profile moved the provider's config directory, already
    /// absolute (`domain::AgentProfile::resolve_config_dir`). `None` is the
    /// default account.
    pub config_dir: Option<&'a Path>,
}

/// Read one account's usage for `descriptor`.
///
/// `executable` is the resolved provider binary, needed only by the CLI source;
/// the OAuth sources read local credentials and ignore it. `account` selects
/// the login: the probe runs against an environment whose config-directory
/// variables point at that account, so the CLI source and the credential reads
/// agree on which login they are describing.
///
/// Returns `None` when the descriptor declares no source or the reading cannot
/// be produced — every such case is "this account reports no usage".
#[must_use]
pub fn collect(
    descriptor: &AgentDescriptor,
    executable: Option<&Path>,
    env: &ResolvedEnvironment,
    account: &UsageAccount<'_>,
) -> Option<ProviderUsage> {
    let env = &crate::descriptor::env_for_config_dir(descriptor, env, account.config_dir);
    let mut usage = match descriptor.usage_source.as_ref()? {
        UsageSource::Cli(probe) => cli::collect(descriptor, executable?, probe, env),
        UsageSource::CodexOAuth => codex::collect(descriptor, env, &UreqClient::default()),
        UsageSource::ClaudeOauth => {
            claude::collect(descriptor, env, account, &UreqClient::default())
        }
        // A source variant this build does not know reports no usage.
        _ => None,
    }?;
    usage.profile_id = account.profile_id;
    Some(usage)
}

/// Extract a whole-percent reading from a window object.
///
/// Three conventions appear in the wild:
/// - `used_percent` / `usage_percent` — already `0..=100` (Codex).
/// - `utilization` — also `0..=100` on Anthropic's OAuth usage endpoint
///   (CodexBar / Claude Code). Treating it as a `0..=1` fraction was the bug
///   that pegged every meter at 100%.
/// - `used_fraction` / `fraction_used` — explicit `0..=1` (our CLI probe).
///
/// Out-of-range values are clamped, not rejected — a provider reporting 104 is
/// over its limit, not broken.
fn percent_from(window: &Value) -> Option<u8> {
    if let Some(percent) = window
        .get("used_percent")
        .or_else(|| window.get("usage_percent"))
        .and_then(Value::as_f64)
    {
        return Some(percent.clamp(0., 100.).round() as u8);
    }
    if let Some(utilization) = window.get("utilization").and_then(Value::as_f64) {
        return Some(utilization.clamp(0., 100.).round() as u8);
    }
    let fraction = window
        .get("fraction_used")
        .or_else(|| window.get("used_fraction"))
        .and_then(Value::as_f64)?;
    Some((fraction.clamp(0., 1.) * 100.).round() as u8)
}

/// Extract a reset timestamp from a window object, if it carries one.
///
/// Accepts `resets_at` / `reset_at` as an RFC-3339 string (Claude) or as a
/// Unix-seconds number (Codex).
fn resets_from(window: &Value) -> Option<Timestamp> {
    let value = window.get("resets_at").or_else(|| window.get("reset_at"))?;
    if let Some(raw) = value.as_str() {
        return Timestamp::parse_rfc3339(raw).ok();
    }
    let secs = value
        .as_i64()
        .or_else(|| value.as_f64().map(|n| n as i64))?;
    Timestamp::from_unix_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_reads_an_explicit_percent() {
        let window = serde_json::json!({ "used_percent": 42.4 });
        assert_eq!(percent_from(&window), Some(42));
    }

    #[test]
    fn percent_reads_claude_utilization_as_whole_percent() {
        // Live Anthropic payloads use 10.0 / 49.0 — not 0.10 / 0.49.
        assert_eq!(
            percent_from(&serde_json::json!({ "utilization": 10.0 })),
            Some(10)
        );
        assert_eq!(
            percent_from(&serde_json::json!({ "utilization": 49.0 })),
            Some(49)
        );
        assert_eq!(
            percent_from(&serde_json::json!({ "utilization": 0.5 })),
            Some(1),
            "half a percent rounds to 1, not 50"
        );
    }

    #[test]
    fn percent_reads_an_explicit_fraction_and_clamps() {
        assert_eq!(
            percent_from(&serde_json::json!({ "used_fraction": 0.5 })),
            Some(50)
        );
        assert_eq!(
            percent_from(&serde_json::json!({ "used_fraction": 1.04 })),
            Some(100)
        );
    }

    #[test]
    fn percent_is_none_without_a_known_field() {
        assert_eq!(percent_from(&serde_json::json!({ "other": 1 })), None);
    }

    #[test]
    fn resets_parses_rfc3339_and_unix_seconds() {
        assert!(resets_from(&serde_json::json!({ "resets_at": "2026-08-22T18:00:00Z" })).is_some());
        assert!(resets_from(&serde_json::json!({ "reset_at": 1_787_707_211_i64 })).is_some());
        assert!(resets_from(&serde_json::json!({ "resets_at": "soon" })).is_none());
        assert!(resets_from(&serde_json::json!({})).is_none());
    }
}
