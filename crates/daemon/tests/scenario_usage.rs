//! §16.2: the settings screen's stats page, end to end through the daemon.
//!
//! The daemon under test reads its transcripts from the harness tree, not from
//! the developer's home (`common` points `CLAUDE_CONFIG_DIR`/`CODEX_HOME` at
//! `Harness::agent_home`), so the numbers asserted here are the fixtures' and
//! nothing else's.

mod common;

use std::fs;
use std::path::Path;

use protocol::{Request, Response};

/// Write one JSONL transcript, creating its directories.
fn write_transcript(path: &Path, lines: &[String]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create the transcript directory");
    fs::write(path, format!("{}\n", lines.join("\n"))).expect("write the transcript");
}

/// An RFC-3339 timestamp `days` days ago, which is how a fixture lands inside
/// the scanned window without hardcoding a date.
fn ago(days: i64) -> String {
    domain::Timestamp::from_offset(time::OffsetDateTime::now_utc() - time::Duration::days(days))
        .to_rfc3339()
}

fn claude_turn(id: &str, at: &str, input: u64, output: u64) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": at,
        "message": {
            "id": id,
            "model": "claude-opus-5",
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        }
    })
    .to_string()
}

fn analytics(client: &client::Client, window_days: Option<u16>) -> domain::UsageAnalytics {
    match client
        .request(Request::GetUsageAnalytics { window_days })
        .expect("the daemon answers the analytics read")
    {
        Response::UsageAnalytics(analytics) => *analytics,
        other => panic!("expected UsageAnalytics, got {other:?}"),
    }
}

/// The read answers from the transcripts on disk, and answers the *same* way
/// twice: the second call is served from the daemon's cache, which is what
/// keeps a re-rendering settings page from re-reading a month of JSONL.
#[test]
fn usage_analytics_are_read_from_the_transcripts_on_disk() {
    let harness = common::Harness::new();
    write_transcript(
        &harness
            .agent_home()
            .join("claude/projects/-tmp-proj/session.jsonl"),
        &[
            claude_turn("msg_1", &ago(1), 1_000_000, 0),
            claude_turn("msg_2", &ago(1), 0, 1_000_000),
        ],
    );

    let daemon = harness.boot();
    let client = daemon.connect("scenario-usage");

    let first = analytics(&client, Some(30));
    assert_eq!(first.window_days, 30);
    assert_eq!(first.providers.len(), 1, "only Claude wrote anything");
    let claude = &first.providers[0];
    assert_eq!(claude.provider_id.as_str(), "claude");
    assert_eq!(claude.sessions, 1);
    assert_eq!(claude.turns, 2);
    assert_eq!(claude.tokens.total(), 2_000_000);
    // $5 of input plus $25 of output, in micro-dollars.
    assert_eq!(claude.cost_micros, 30 * domain::MICROS_PER_USD);
    assert_eq!(claude.unpriced_turns, 0);
    assert_eq!(first.active_days(), 1);
    assert_eq!(first.skipped, 0, "nothing was left unread");

    // A transcript written after the first read is invisible until the cache
    // expires — the staleness is the deal the TTL makes, and it is deliberate.
    write_transcript(
        &harness
            .agent_home()
            .join("claude/projects/-tmp-proj/later.jsonl"),
        &[claude_turn("msg_3", &ago(1), 5, 5)],
    );
    let second = analytics(&client, Some(30));
    assert_eq!(
        second.providers[0].turns, 2,
        "the second read is served from the cache"
    );

    // A different window is a different question, so it is scanned afresh and
    // sees the file the cached answer predates.
    let narrower = analytics(&client, Some(7));
    assert_eq!(narrower.window_days, 7);
    assert_eq!(narrower.providers[0].turns, 3);
}

/// A machine with no transcripts reports nothing at all — not a provider with
/// zeroes, which would read as "the agents did nothing" rather than "there is
/// nothing to read".
#[test]
fn a_machine_with_no_transcripts_reports_no_providers() {
    let harness = common::Harness::new();
    let daemon = harness.boot();
    let client = daemon.connect("scenario-usage-empty");

    let empty = analytics(&client, None);
    assert!(empty.providers.is_empty());
    assert!(empty.daily.is_empty());
    assert_eq!(empty.scanned, 0);
    assert_eq!(
        empty.window_days, 30,
        "no window asked for takes the default"
    );
}
