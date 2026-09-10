//! Codex usage from local OAuth credentials (§16.2).
//!
//! Reads the access token from `~/.codex/auth.json` (honouring `$CODEX_HOME`)
//! and calls the ChatGPT usage endpoint with it. Nothing here logs the token.

use std::path::PathBuf;

use domain::{AgentDescriptor, ProviderUsage, ResolvedEnvironment, Timestamp, UsageWindow};
use serde::Deserialize;
use serde_json::Value;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// `auth.json` as written by the Codex CLI. Only the access token matters here;
/// everything else is ignored so a schema addition never breaks the read.
#[derive(Debug, Deserialize)]
struct AuthJson {
    tokens: Option<Tokens>,
}

#[derive(Debug, Deserialize)]
struct Tokens {
    access_token: Option<String>,
}

pub(super) fn collect(
    descriptor: &AgentDescriptor,
    env: &ResolvedEnvironment,
    http: &dyn super::HttpClient,
) -> Option<ProviderUsage> {
    let token = read_access_token(env)?;
    let body = http.get(USAGE_URL, &[("Authorization", &format!("Bearer {token}"))])?;

    let windows = parse_usage(&body);
    if windows.is_empty() {
        return None;
    }
    Some(ProviderUsage {
        provider_id: descriptor.id.clone(),
        // Stamped by `super::collect`, which is what knows the account.
        profile_id: None,
        windows,
        collected_at: Timestamp::now(),
    })
}

/// `$CODEX_HOME/auth.json`, falling back to `$HOME/.codex/auth.json`.
fn auth_path(env: &ResolvedEnvironment) -> Option<PathBuf> {
    if let Some(home) = env.get("CODEX_HOME") {
        return Some(PathBuf::from(home).join("auth.json"));
    }
    let home = env.get("HOME")?;
    Some(PathBuf::from(home).join(".codex").join("auth.json"))
}

fn read_access_token(env: &ResolvedEnvironment) -> Option<String> {
    let path = auth_path(env)?;
    let raw = std::fs::read(&path).ok()?;
    let auth: AuthJson = serde_json::from_slice(&raw).ok()?;
    auth.tokens?.access_token
}

/// Map the endpoint body to windows: `primary_window` → the rolling session
/// limit (`"5h"`), `secondary_window` → the weekly limit (`"week"`).
///
/// The live ChatGPT payload nests both under `rate_limit` and stamps resets as
/// Unix seconds in `reset_at` (CodexBar's shape). A flat root-level payload is
/// still accepted so older fixtures keep working. A window that carries no
/// readable percent is dropped rather than shown as zero.
fn parse_usage(body: &[u8]) -> Vec<UsageWindow> {
    let root: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(%error, "codex usage response was not JSON");
            return Vec::new();
        }
    };

    let container = root.get("rate_limit").unwrap_or(&root);

    [("primary_window", "5h"), ("secondary_window", "week")]
        .into_iter()
        .filter_map(|(key, label)| {
            let window = container.get(key)?;
            Some(UsageWindow {
                used_percent: super::percent_from(window)?,
                window: label.to_owned(),
                resets_at: super::resets_from(window),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::http::test_support::FakeHttp;
    use domain::{AgentProviderId, EnvSource};

    fn env_with(vars: Vec<(&str, &str)>) -> ResolvedEnvironment {
        ResolvedEnvironment {
            shell: PathBuf::from("/bin/sh"),
            vars: vars
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            path_entries: vec![],
            resolved_at: Timestamp::now(),
            source: EnvSource::ProcessFallback,
        }
    }

    #[test]
    fn maps_both_windows_with_resets() {
        // Live ChatGPT shape: nested under rate_limit, reset_at as unix seconds.
        let windows = parse_usage(
            br#"{
                "plan_type": "plus",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 0,
                        "limit_window_seconds": 18000,
                        "reset_at": 1787707211
                    },
                    "secondary_window": {
                        "used_percent": 15,
                        "limit_window_seconds": 604800,
                        "reset_at": 1788275576
                    }
                }
            }"#,
        );
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window, "5h");
        assert_eq!(windows[0].used_percent, 0);
        assert!(windows[0].resets_at.is_some());
        assert_eq!(windows[1].window, "week");
        assert_eq!(windows[1].used_percent, 15);
        assert!(windows[1].resets_at.is_some());
    }

    #[test]
    fn flat_root_payload_still_maps() {
        let windows = parse_usage(
            br#"{
                "primary_window":   { "used_percent": 42, "resets_at": "2026-08-22T18:00:00Z" },
                "secondary_window": { "used_percent": 10 }
            }"#,
        );
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].used_percent, 42);
        assert!(windows[0].resets_at.is_some());
        assert_eq!(windows[1].resets_at, None);
    }

    #[test]
    fn a_fraction_convention_also_maps() {
        let windows = parse_usage(br#"{ "primary_window": { "used_fraction": 0.5 } }"#);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, 50);
    }

    #[test]
    fn junk_and_empty_yield_no_windows() {
        assert!(parse_usage(b"not json").is_empty());
        assert!(parse_usage(b"{}").is_empty());
    }

    #[test]
    fn collect_reads_token_from_codex_home_then_calls_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{ "tokens": { "access_token": "secret" } }"#,
        )
        .unwrap();
        let env = env_with(vec![("CODEX_HOME", dir.path().to_str().unwrap())]);
        let http = FakeHttp::with(
            USAGE_URL,
            r#"{ "rate_limit": { "primary_window": { "used_percent": 33 } } }"#,
        );

        let descriptor = crate::builtins::builtin("codex").unwrap();
        let usage = collect(&descriptor, &env, &http).expect("a reading");
        assert_eq!(usage.provider_id, AgentProviderId::new("codex"));
        assert_eq!(usage.windows[0].used_percent, 33);
    }

    #[test]
    fn collect_is_none_when_not_signed_in() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with(vec![("CODEX_HOME", dir.path().to_str().unwrap())]);
        let http = FakeHttp::default();
        let descriptor = crate::builtins::builtin("codex").unwrap();
        assert!(collect(&descriptor, &env, &http).is_none());
    }
}
