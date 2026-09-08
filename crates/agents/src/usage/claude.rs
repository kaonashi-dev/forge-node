//! Claude usage from local OAuth credentials (§16.2).
//!
//! Reads the access token from `~/.claude/.credentials.json`, falling back on
//! macOS to the login Keychain (`security find-generic-password`, the existing
//! subprocess pattern — no new crate). Then calls the Anthropic usage endpoint.
//! Nothing here logs the token.

use std::path::{Path, PathBuf};

use domain::{AgentDescriptor, ProviderUsage, ResolvedEnvironment, Timestamp, UsageWindow};
use serde::Deserialize;
use serde_json::Value;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// The Keychain service name the Claude CLI stores its credentials under.
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// `.credentials.json` (and the Keychain blob) share this shape.
#[derive(Debug, Deserialize)]
struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthField>,
}

#[derive(Debug, Deserialize)]
struct OauthField {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

pub(super) fn collect(
    descriptor: &AgentDescriptor,
    env: &ResolvedEnvironment,
    http: &dyn super::HttpClient,
) -> Option<ProviderUsage> {
    let token = read_access_token(env)?;
    let body = http.get(
        USAGE_URL,
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("anthropic-beta", OAUTH_BETA),
        ],
    )?;

    let windows = parse_usage(&body);
    if windows.is_empty() {
        return None;
    }
    Some(ProviderUsage {
        provider_id: descriptor.id.clone(),
        windows,
        collected_at: Timestamp::now(),
    })
}

fn read_access_token(env: &ResolvedEnvironment) -> Option<String> {
    if let Some(home) = env.get("HOME") {
        let path = PathBuf::from(home)
            .join(".claude")
            .join(".credentials.json");
        if let Ok(raw) = std::fs::read(&path) {
            if let Some(token) = parse_credentials(&raw) {
                return Some(token);
            }
        }
    }
    keychain_token(env)
}

/// macOS Keychain fallback. Elsewhere the binary is absent and this yields
/// `None`, so no platform `cfg` is needed beyond skipping the spawn.
fn keychain_token(env: &ResolvedEnvironment) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let args = vec![
        "find-generic-password".to_owned(),
        "-s".to_owned(),
        KEYCHAIN_SERVICE.to_owned(),
        "-w".to_owned(),
    ];
    let out =
        crate::detection::run_to_completion(Path::new("/usr/bin/security"), &args, 3000, env)?;
    parse_credentials(&out)
}

fn parse_credentials(raw: &[u8]) -> Option<String> {
    let creds: Credentials = serde_json::from_slice(raw).ok()?;
    creds.claude_ai_oauth?.access_token
}

/// Map the endpoint body to windows: `five_hour` → `"5h"`, `seven_day` →
/// `"week"`. A window with no readable percent is dropped, not shown as zero.
fn parse_usage(body: &[u8]) -> Vec<UsageWindow> {
    let root: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(%error, "claude usage response was not JSON");
            return Vec::new();
        }
    };

    [("five_hour", "5h"), ("seven_day", "week")]
        .into_iter()
        .filter_map(|(key, label)| {
            let window = root.get(key)?;
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
    use domain::EnvSource;

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
    fn credentials_yield_the_access_token() {
        let token = parse_credentials(br#"{ "claudeAiOauth": { "accessToken": "abc" } }"#);
        assert_eq!(token.as_deref(), Some("abc"));
        assert!(parse_credentials(b"{}").is_none());
        assert!(parse_credentials(b"not json").is_none());
    }

    #[test]
    fn maps_five_hour_and_seven_day() {
        // Live Anthropic shape: utilization is already a whole percent.
        let windows = parse_usage(
            br#"{
                "five_hour": { "utilization": 10.0, "resets_at": "2026-08-22T18:00:00Z" },
                "seven_day": { "utilization": 49.0 }
            }"#,
        );
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window, "5h");
        assert_eq!(windows[0].used_percent, 10);
        assert!(windows[0].resets_at.is_some());
        assert_eq!(windows[1].window, "week");
        assert_eq!(windows[1].used_percent, 49);
    }

    #[test]
    fn collect_reads_token_from_home_then_calls_endpoint() {
        let home = tempfile::tempdir().unwrap();
        let claude_dir = home.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join(".credentials.json"),
            r#"{ "claudeAiOauth": { "accessToken": "secret" } }"#,
        )
        .unwrap();
        let env = env_with(vec![("HOME", home.path().to_str().unwrap())]);
        let http = FakeHttp::with(USAGE_URL, r#"{ "five_hour": { "utilization": 60.0 } }"#);

        let descriptor = crate::builtins::builtin("claude").unwrap();
        let usage = collect(&descriptor, &env, &http).expect("a reading");
        assert_eq!(usage.windows[0].used_percent, 60);
    }
}
