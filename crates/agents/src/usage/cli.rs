//! The CLI usage source (§16.2): run a provider CLI that prints one documented
//! JSON document on stdout. Forge deliberately does not scrape human output — a
//! usage meter is only worth showing if the number behind it came from something
//! that promised to be a number.

use std::path::Path;

use domain::{
    AgentDescriptor, ProviderUsage, ResolvedEnvironment, Timestamp, UsageProbe, UsageWindow,
};
use serde::Deserialize;

/// The document a usage probe is expected to print on stdout.
#[derive(Debug, Deserialize)]
struct UsageDocument {
    /// Fraction of the allowance consumed. Out-of-range values are clamped
    /// rather than rejected: a provider reporting 1.04 is over its limit, not
    /// broken.
    used_fraction: f32,
    /// The window the fraction covers.
    #[serde(default = "default_window")]
    window: String,
    /// When the window resets.
    #[serde(default)]
    resets_at: Option<Timestamp>,
}

fn default_window() -> String {
    "session".to_owned()
}

/// Run `probe` against `executable` and parse the result into a single window.
///
/// Returns `None` when the probe fails or times out, or the output is not the
/// documented document — every one of those is "this provider reports no usage".
pub(super) fn collect(
    descriptor: &AgentDescriptor,
    executable: &Path,
    probe: &UsageProbe,
    env: &ResolvedEnvironment,
) -> Option<ProviderUsage> {
    let stdout =
        crate::detection::run_to_completion(executable, &probe.args, probe.timeout_ms, env)?;

    let document: UsageDocument = serde_json::from_slice(&stdout)
        .inspect_err(|error| {
            tracing::debug!(
                provider_id = %descriptor.id,
                %error,
                "usage probe output was not the documented JSON document"
            );
        })
        .ok()?;

    Some(ProviderUsage {
        provider_id: descriptor.id.clone(),
        // Stamped by `super::collect`, which is what knows the account.
        profile_id: None,
        windows: vec![UsageWindow {
            // Rounded to a whole percent on the way in, so every consumer sees
            // the same number the probe implied.
            used_percent: (document.used_fraction.clamp(0., 1.) * 100.).round() as u8,
            window: document.window,
            resets_at: document.resets_at,
        }],
        collected_at: Timestamp::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<UsageDocument> {
        serde_json::from_str(json).ok()
    }

    #[test]
    fn the_documented_shape_parses() {
        let document = parse(r#"{"used_fraction":0.42,"window":"5h"}"#).expect("parses");
        assert!((document.used_fraction - 0.42).abs() < f32::EPSILON);
        assert_eq!(document.window, "5h");
        assert_eq!(document.resets_at, None);
    }

    #[test]
    fn only_the_fraction_is_required() {
        let document = parse(r#"{"used_fraction":0.1}"#).expect("parses");
        assert_eq!(document.window, "session");
    }

    /// The failure that matters: human output must not become a meter.
    #[test]
    fn human_readable_output_is_not_a_usage_document() {
        assert!(parse("You have used 42% of your weekly limit.").is_none());
        assert!(parse("{}").is_none());
        assert!(parse(r#"{"used":0.42}"#).is_none());
    }
}
