//! Grok account usage over its ACP entry (§16.2).
//!
//! The odd one of the four. Claude and Codex keep their meter behind an HTTP
//! endpoint that a local OAuth token opens; Grok does not publish one at all —
//! its account billing is a JSON-RPC *extension method*, `_x.ai/billing`, on
//! the same `grok agent stdio` wire the descriptor already declares for ACP.
//! So a reading here costs one short-lived child process and two lines of
//! stdin instead of one GET, and the credentials never leave the CLI: this
//! never opens `~/.grok/auth.json`, it asks the binary that owns it.
//!
//! The spawn reuses [`domain::AcpSpec::args`] rather than spelling
//! `agent stdio` again — one fact, declared once, in `builtins`.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use domain::{AgentDescriptor, ProviderUsage, ResolvedEnvironment, Timestamp, UsageWindow};
use serde_json::Value;

/// How long the whole handshake may take before the child is killed.
///
/// Generous next to the 3 s version probe because this one may start a real
/// agent (or attach to a running leader) rather than print a string, and
/// stingy next to the 5-minute sweep it runs inside.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Our id for `initialize`; the reply is waited for so the billing call cannot
/// race the handshake.
const INITIALIZE_ID: u64 = 1;
/// Our id for `_x.ai/billing`. Seeing this id come back is the exit condition.
const BILLING_ID: u64 = 2;

/// The ACP protocol version this handshake speaks. `grok agent stdio` answers
/// `initialize` with the same number.
const PROTOCOL_VERSION: u64 = 1;

pub(super) fn collect(
    descriptor: &AgentDescriptor,
    executable: &Path,
    env: &ResolvedEnvironment,
) -> Option<ProviderUsage> {
    let args = &descriptor.acp.as_ref()?.args;
    let body = ask_for_billing(executable, args, env)?;

    let windows = parse_billing(&body);
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

/// Spawn the ACP entry, say `initialize`, ask `_x.ai/billing`, return its
/// `result` — then kill the child, which would otherwise sit there being an
/// agent.
///
/// Every failure path returns `None`: a meter is worth showing only when the
/// number behind it arrived, and a usage sweep must never be able to fail a
/// provider that is working fine.
fn ask_for_billing(executable: &Path, args: &[String], env: &ResolvedEnvironment) -> Option<Value> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // Under the resolved login environment (§12), so a profile that moved
    // `GROK_HOME` is answered for *that* account and not the default one.
    command.env_clear();
    for (key, value) in &env.vars {
        command.env(key, value);
    }
    if !env.vars.iter().any(|(k, _)| k == "PATH") {
        if let Ok(joined) = std::env::join_paths(env.path_entries.iter()) {
            command.env("PATH", joined);
        }
    }

    let mut child = command
        .spawn()
        .inspect_err(|error| tracing::debug!(%error, "could not start the Grok ACP entry"))
        .ok()?;

    let result = converse(&mut child);
    // Killed either way: this child is an agent waiting for work, not a probe
    // that exits on its own.
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Write both requests, then read lines until the billing reply arrives.
///
/// The read runs on its own thread so a child that answers nothing is bounded
/// by [`TIMEOUT`] rather than by however long it feels like living.
fn converse(child: &mut Child) -> Option<Value> {
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(BILLING_ID) {
                let _ = tx.send(value.get("result").cloned());
                return;
            }
        }
        // The stream ended without an answer.
        let _ = tx.send(None);
    });

    let requests = format!(
        "{}\n{}\n",
        request(
            INITIALIZE_ID,
            "initialize",
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
            })
        ),
        request(BILLING_ID, "_x.ai/billing", serde_json::json!({})),
    );
    stdin.write_all(requests.as_bytes()).ok()?;
    stdin.flush().ok()?;

    // `stdin` is deliberately still open here. Grok treats EOF on the ACP wire
    // as the client hanging up and shuts down, so closing it after the write —
    // the obvious thing to do — loses the race against its own reply and the
    // meter reads empty every time. It is dropped below, once the answer is in.
    let reply = rx.recv_timeout(TIMEOUT).ok().flatten();
    drop(stdin);
    reply
}

fn request(id: u64, method: &str, params: Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        .to_string()
}

/// Map the billing reply to one window.
///
/// The shape is `{"config":{…},"subscription_tier":"…"}`, and only `config`
/// carries numbers: `creditUsagePercent` is already `0..=100`, and the period
/// that percentage covers is named by `currentPeriod.type` and ends at
/// `currentPeriod.end`. A reply with no readable percent yields no window
/// rather than a meter pinned at zero — an account with nothing to report and
/// an account Forge cannot read must not look the same on screen.
fn parse_billing(body: &Value) -> Vec<UsageWindow> {
    let Some(config) = body.get("config") else {
        return Vec::new();
    };
    let Some(percent) = config
        .get("creditUsagePercent")
        .and_then(Value::as_f64)
        .map(|percent| percent.clamp(0., 100.).round() as u8)
    else {
        return Vec::new();
    };

    let period = config.get("currentPeriod");
    let resets_at = period
        .and_then(|period| period.get("end"))
        .or_else(|| config.get("billingPeriodEnd"))
        .and_then(Value::as_str)
        .and_then(|raw| Timestamp::parse_rfc3339(raw).ok());

    vec![UsageWindow {
        used_percent: percent,
        window: window_label(period.and_then(|p| p.get("type")).and_then(Value::as_str)),
        resets_at,
    }]
}

/// `USAGE_PERIOD_TYPE_WEEKLY` → `"week"`, to sit beside Codex's own `"week"`
/// in one status bar rather than shouting an enum name at the reader.
///
/// An unknown period keeps its own tail lowercased instead of being forced
/// into one of the four: a label nobody recognises is still better than a
/// wrong one.
fn window_label(period_type: Option<&str>) -> String {
    let Some(raw) = period_type else {
        return "billing period".to_owned();
    };
    let tail = raw.trim_start_matches("USAGE_PERIOD_TYPE_");
    match tail {
        "HOURLY" => "hour".to_owned(),
        "DAILY" => "day".to_owned(),
        "WEEKLY" => "week".to_owned(),
        "MONTHLY" => "month".to_owned(),
        "" => "billing period".to_owned(),
        other => other.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{env_with_path, write_script};
    use domain::AgentProviderId;

    fn json(raw: &str) -> Value {
        serde_json::from_str(raw).expect("valid JSON")
    }

    /// The live `_x.ai/billing` reply, as one account actually answers it.
    #[test]
    fn the_live_reply_maps_to_one_weekly_window() {
        let windows = parse_billing(&json(
            r#"{
                "config": {
                    "creditUsagePercent": 26.0,
                    "currentPeriod": {
                        "type": "USAGE_PERIOD_TYPE_WEEKLY",
                        "start": "2026-09-06T17:14:35.974380+00:00",
                        "end": "2026-09-13T17:14:35.974380+00:00"
                    },
                    "onDemandCap": { "val": 0 },
                    "prepaidBalance": { "val": 0 },
                    "isUnifiedBillingUser": true,
                    "billingPeriodEnd": "2026-09-13T17:14:35.974380+00:00"
                },
                "subscription_tier": "SuperGrok"
            }"#,
        ));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, 26);
        assert_eq!(windows[0].window, "week");
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn the_period_end_is_read_when_the_period_itself_is_absent() {
        let windows = parse_billing(&json(
            r#"{ "config": {
                "creditUsagePercent": 4.4,
                "billingPeriodEnd": "2026-09-13T17:14:35Z"
            } }"#,
        ));
        assert_eq!(windows[0].used_percent, 4);
        assert_eq!(windows[0].window, "billing period");
        assert!(windows[0].resets_at.is_some());
    }

    /// An account over its allowance is over it, not broken.
    #[test]
    fn an_out_of_range_percent_is_clamped() {
        let over = parse_billing(&json(r#"{ "config": { "creditUsagePercent": 140 } }"#));
        assert_eq!(over[0].used_percent, 100);
        let under = parse_billing(&json(r#"{ "config": { "creditUsagePercent": -3 } }"#));
        assert_eq!(under[0].used_percent, 0);
    }

    /// The failure that matters: nothing readable must not read as 0%.
    #[test]
    fn a_reply_with_no_percent_yields_no_window() {
        assert!(parse_billing(&json("{}")).is_empty());
        assert!(parse_billing(&json(r#"{ "config": {} }"#)).is_empty());
        assert!(parse_billing(&json(r#"{ "subscription_tier": "SuperGrok" }"#)).is_empty());
        assert!(parse_billing(&json(r#"{ "config": { "creditUsagePercent": "26" } }"#)).is_empty());
    }

    #[test]
    fn period_names_become_the_labels_the_status_bar_uses() {
        assert_eq!(window_label(Some("USAGE_PERIOD_TYPE_WEEKLY")), "week");
        assert_eq!(window_label(Some("USAGE_PERIOD_TYPE_MONTHLY")), "month");
        assert_eq!(window_label(Some("USAGE_PERIOD_TYPE_DAILY")), "day");
        // Unrecognised, and honest about it.
        assert_eq!(
            window_label(Some("USAGE_PERIOD_TYPE_FORTNIGHTLY")),
            "fortnightly"
        );
        assert_eq!(window_label(Some("USAGE_PERIOD_TYPE_")), "billing period");
        assert_eq!(window_label(None), "billing period");
    }

    /// End to end against a fake agent that speaks the two lines this needs:
    /// the request has to be written, the `id` matched, and the child killed.
    #[test]
    fn collect_talks_to_the_acp_entry_and_reads_the_reply() {
        let dir = tempfile::tempdir().unwrap();
        // Answers `initialize`, then billing, then blocks like a real agent —
        // so a `collect` that waited for exit instead of for the id would hang.
        let script = write_script(
            dir.path(),
            "grok",
            r#"#!/bin/sh
read -r _line
echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}'
read -r _line
echo '{"jsonrpc":"2.0","id":2,"result":{"config":{"creditUsagePercent":73,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-09-13T17:14:35Z"}}}}'
while :; do sleep 1; done
"#,
        );
        let descriptor = crate::builtins::builtin("grok").unwrap();
        let usage = collect(&descriptor, &script, &env_with_path(vec![])).expect("a reading");
        assert_eq!(usage.provider_id, AgentProviderId::new("grok"));
        assert_eq!(usage.windows[0].used_percent, 73);
        assert_eq!(usage.windows[0].window, "week");
    }

    /// The contract against the real thing: `_x.ai/billing` is an undocumented
    /// extension method, so the fake above proves the plumbing and this proves
    /// the spelling. Ignored by default — it needs a signed-in Grok.
    #[test]
    #[ignore = "requires a signed-in grok CLI on PATH"]
    fn the_installed_grok_answers_the_billing_method() {
        let home = std::env::var("HOME").expect("a home");
        let path = std::env::var("PATH").unwrap_or_default();
        let env = ResolvedEnvironment {
            shell: std::path::PathBuf::from("/bin/sh"),
            vars: vec![("HOME".to_owned(), home), ("PATH".to_owned(), path.clone())],
            path_entries: std::env::split_paths(&path).collect(),
            resolved_at: Timestamp::now(),
            source: domain::EnvSource::ProcessFallback,
        };
        let descriptor = crate::builtins::builtin("grok").unwrap();
        let executable = crate::detection::detect(&descriptor, &env, None);
        let domain::DetectionStatus::Installed { executable, .. } = executable.status else {
            panic!("grok is not installed on this machine");
        };
        let usage = collect(&descriptor, &executable, &env).expect("a live reading");
        assert!(usage.windows[0].used_percent <= 100);
        assert!(!usage.windows[0].window.is_empty());
    }

    /// A binary that answers nothing reports no usage, and does not hang the
    /// sweep waiting for it.
    #[test]
    fn a_silent_agent_reports_no_usage() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "grok", "#!/bin/sh\nexit 0\n");
        let descriptor = crate::builtins::builtin("grok").unwrap();
        assert!(collect(&descriptor, &script, &env_with_path(vec![])).is_none());
    }
}
