//! Token analytics read from the transcripts the agent CLIs already write.
//!
//! [`super::collect`] asks a provider what its *allowance* looks like. This
//! reads what actually happened: every billed turn a provider recorded on this
//! machine, summed into tokens, turns, days and an estimated cost. Both are
//! provider-specific facts, so both live in this crate (principle P2); nothing
//! outside it knows that Claude writes JSONL under `~/.claude/projects` or that
//! Codex writes rollouts under `~/.codex/sessions`.
//!
//! Two stores are read today:
//!
//! - **Claude Code** — `~/.claude/projects/<slug>/*.jsonl` (and the
//!   `<sessionId>/subagents/*.jsonl` beside them). Every assistant record
//!   carries `message.usage`, which is where all five token counts come from,
//!   including the five-minute/one-hour split that prices a cache write.
//! - **Codex CLI** — `~/.codex/sessions/<y>/<m>/<d>/rollout-*.jsonl`. Its
//!   `token_count` events carry a *cumulative* `total_token_usage`, so a turn
//!   is the difference between consecutive events, never the event itself.
//!
//! OpenCode and Cursor are absent for the same reason they declare no
//! [`domain::UsageSource`]: OpenCode bills through whichever model provider it
//! is configured with, and Cursor records nothing locally to count.
//!
//! Everything is best effort, like transcript discovery: an unreadable file, a
//! malformed line or a missing directory yields a smaller number, never an
//! error. The one thing it will not do is under-report silently — a scan that
//! hits its cap says how many transcripts it left unread.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use domain::{
    AgentProviderId, DailyUsage, ProviderAnalytics, ResolvedEnvironment, Timestamp, TokenTotals,
    UsageAnalytics,
};
use serde_json::Value;

use super::pricing::{price_for, ModelPrice};

/// Default horizon when the caller names none. Thirty days is what the page
/// draws as a calendar, and long enough that a week off does not empty it.
pub const DEFAULT_WINDOW_DAYS: u16 = 30;
/// Longest horizon accepted, so a stray request cannot turn into a full-disk
/// scan of years of history.
pub const MAX_WINDOW_DAYS: u16 = 365;
/// Transcripts read per provider, most recently modified first. A busy month
/// stays well under this; a machine with more history gets the recent part of
/// it and is told the rest was skipped.
const SCAN_LIMIT: usize = 500;

/// Read every supported provider's transcripts and aggregate them (§16.2).
///
/// `window_days` is clamped into `1..=MAX_WINDOW_DAYS`; `0` means the default.
/// The result is ordered by tokens descending, so the busiest provider leads.
#[must_use]
pub fn collect(window_days: u16, env: &ResolvedEnvironment) -> UsageAnalytics {
    let window_days = match window_days {
        0 => DEFAULT_WINDOW_DAYS,
        days => days.min(MAX_WINDOW_DAYS),
    };
    let now = Timestamp::now();
    let cutoff = now.as_offset() - time::Duration::days(i64::from(window_days));

    let mut daily: BTreeMap<String, u64> = BTreeMap::new();
    let mut providers = Vec::new();
    let mut scanned = 0_u32;
    let mut skipped = 0_u32;

    for (provider, files) in [
        ("claude", claude_transcripts(env, cutoff)),
        ("codex", codex_transcripts(env, cutoff)),
    ] {
        skipped += files.skipped;
        let mut totals = Totals::default();
        for path in &files.paths {
            let counted = match provider {
                "claude" => scan_claude(path, cutoff, &mut totals, &mut daily),
                _ => scan_codex(path, cutoff, &mut totals, &mut daily),
            };
            scanned += 1;
            if counted {
                totals.sessions += 1;
            }
        }
        if let Some(analytics) = totals.finish(AgentProviderId::new(provider)) {
            providers.push(analytics);
        }
    }

    providers.sort_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));

    UsageAnalytics {
        providers,
        daily: daily
            .into_iter()
            .map(|(date, tokens)| DailyUsage { date, tokens })
            .collect(),
        window_days,
        scanned,
        skipped,
        collected_at: now,
    }
}

// ---------------------------------------------------------------------
// Accumulation
// ---------------------------------------------------------------------

/// One provider's running totals while its transcripts are read.
#[derive(Default)]
struct Totals {
    tokens: TokenTotals,
    sessions: u32,
    turns: u32,
    cost_micros: u64,
    unpriced_turns: u32,
    /// Tokens per model, so the card can name the one that did the work.
    per_model: HashMap<String, u64>,
    worked_secs: u64,
    first: Option<Timestamp>,
    last: Option<Timestamp>,
    /// `message.id` hashes already counted. Resuming a Claude session copies
    /// earlier records into the new transcript, so without this every resumed
    /// conversation would be billed twice.
    seen: HashSet<u64>,
}

impl Totals {
    /// Record one billed turn.
    fn turn(&mut self, turn: &Turn, at: Option<Timestamp>, daily: &mut BTreeMap<String, u64>) {
        self.tokens.add(&turn.tokens);
        self.turns += 1;
        match turn.model.as_deref().and_then(price_for) {
            Some(price) => self.cost_micros += turn.cost_micros(price),
            None => self.unpriced_turns += 1,
        }
        if let Some(model) = &turn.model {
            *self.per_model.entry(model.clone()).or_default() += turn.tokens.total();
        }
        if let Some(at) = at {
            self.first = Some(self.first.map_or(at, |first| first.min(at)));
            self.last = Some(self.last.map_or(at, |last| last.max(at)));
            *daily.entry(date_key(at)).or_default() += turn.tokens.total();
        }
    }

    /// Add one transcript's first-to-last span to the worked clock.
    fn worked(&mut self, span: Option<(Timestamp, Timestamp)>) {
        if let Some((first, last)) = span {
            let secs = (last.as_offset() - first.as_offset()).whole_seconds();
            self.worked_secs += u64::try_from(secs).unwrap_or(0);
        }
    }

    /// Whether this turn is new, keyed on the provider's own message id.
    fn is_new(&mut self, key: &str) -> bool {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        self.seen.insert(hasher.finish())
    }

    fn finish(self, provider_id: AgentProviderId) -> Option<ProviderAnalytics> {
        if self.turns == 0 {
            return None;
        }
        let top_model = self
            .per_model
            .iter()
            .max_by_key(|(model, tokens)| (**tokens, std::cmp::Reverse(model.as_str())))
            .map(|(model, _)| model.clone());
        Some(ProviderAnalytics {
            provider_id,
            tokens: self.tokens,
            sessions: self.sessions,
            turns: self.turns,
            cost_micros: self.cost_micros,
            unpriced_turns: self.unpriced_turns,
            top_model,
            worked_secs: self.worked_secs,
            first_activity: self.first,
            last_activity: self.last,
        })
    }
}

/// One billed turn, normalized across providers.
#[derive(Default)]
struct Turn {
    tokens: TokenTotals,
    /// Cache writes at the five-minute TTL (1.25x) and the one-hour one (2x).
    cache_write_5m: u64,
    cache_write_1h: u64,
    model: Option<String>,
}

impl Turn {
    fn cost_micros(&self, price: ModelPrice) -> u64 {
        // A provider that reports a cache write without saying which TTL it
        // used is priced at the cheaper one: over-charging an estimate is the
        // worse error, and 5m is the default TTL on both providers.
        let five_minute = if self.cache_write_5m + self.cache_write_1h == 0 {
            self.tokens.cache_write
        } else {
            self.cache_write_5m
        };
        price.cost_micros(
            self.tokens.input,
            self.tokens.output,
            five_minute,
            self.cache_write_1h,
            self.tokens.cache_read,
        )
    }

    fn is_empty(&self) -> bool {
        self.tokens.total() == 0
    }
}

// ---------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------

/// Read one Claude transcript. Returns whether it carried a billed turn.
fn scan_claude(
    path: &Path,
    cutoff: time::OffsetDateTime,
    totals: &mut Totals,
    daily: &mut BTreeMap<String, u64>,
) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut counted = false;
    let mut span: Option<(Timestamp, Timestamp)> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // The cheap gate first: most lines in a transcript are user turns and
        // tool traffic, and parsing all of them costs more than the read.
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let at = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|raw| Timestamp::parse_rfc3339(raw).ok());
        // A resumed transcript carries records older than the window; count
        // them only when they fall inside it, so the window means something.
        if at.is_some_and(|at| at.as_offset() < cutoff) {
            continue;
        }
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}:{line:.120}", path.display()));
        if !totals.is_new(&id) {
            continue;
        }

        let mut turn = Turn {
            model: message
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned),
            ..Turn::default()
        };
        turn.tokens = TokenTotals {
            input: number(usage, "input_tokens"),
            output: number(usage, "output_tokens"),
            cache_write: number(usage, "cache_creation_input_tokens"),
            cache_read: number(usage, "cache_read_input_tokens"),
            reasoning: usage
                .get("output_tokens_details")
                .map_or(0, |details| number(details, "thinking_tokens")),
        };
        if let Some(creation) = usage.get("cache_creation") {
            turn.cache_write_5m = number(creation, "ephemeral_5m_input_tokens");
            turn.cache_write_1h = number(creation, "ephemeral_1h_input_tokens");
        }
        if turn.is_empty() {
            continue;
        }
        if let Some(at) = at {
            span = Some(match span {
                Some((first, last)) => (first.min(at), last.max(at)),
                None => (at, at),
            });
        }
        totals.turn(&turn, at, daily);
        counted = true;
    }
    totals.worked(span);
    counted
}

// ---------------------------------------------------------------------
// Codex CLI
// ---------------------------------------------------------------------

/// Read one Codex rollout. Returns whether it carried a billed turn.
///
/// `total_token_usage` is cumulative over the session, so a turn is the
/// *difference* between consecutive events. Summing the events themselves would
/// multiply a long session by its own length.
fn scan_codex(
    path: &Path,
    cutoff: time::OffsetDateTime,
    totals: &mut Totals,
    daily: &mut BTreeMap<String, u64>,
) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut counted = false;
    let mut span: Option<(Timestamp, Timestamp)> = None;
    let mut previous = TokenTotals::default();
    let mut model: Option<String> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let is_usage = line.contains("\"token_count\"");
        if !is_usage && !line.contains("\"turn_context\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(payload) = record.get("payload") else {
            continue;
        };
        if !is_usage {
            // The model can change mid-session; the latest one before a turn is
            // the one that turn ran on.
            if let Some(name) = payload.get("model").and_then(Value::as_str) {
                model = Some(name.to_owned());
            }
            continue;
        }
        let Some(cumulative) = payload
            .get("info")
            .and_then(|info| info.get("total_token_usage"))
        else {
            continue;
        };
        let running = codex_totals(cumulative);
        let turn = Turn {
            tokens: TokenTotals {
                input: running.input.saturating_sub(previous.input),
                output: running.output.saturating_sub(previous.output),
                cache_write: running.cache_write.saturating_sub(previous.cache_write),
                cache_read: running.cache_read.saturating_sub(previous.cache_read),
                reasoning: running.reasoning.saturating_sub(previous.reasoning),
            },
            model: model.clone(),
            ..Turn::default()
        };
        previous = running;
        if turn.is_empty() {
            continue;
        }
        let at = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|raw| Timestamp::parse_rfc3339(raw).ok());
        if at.is_some_and(|at| at.as_offset() < cutoff) {
            continue;
        }
        if let Some(at) = at {
            span = Some(match span {
                Some((first, last)) => (first.min(at), last.max(at)),
                None => (at, at),
            });
        }
        totals.turn(&turn, at, daily);
        counted = true;
    }
    totals.worked(span);
    counted
}

/// Codex counts cached input *inside* `input_tokens`; Claude keeps them apart.
/// Normalizing here is what lets one [`TokenTotals`] describe both.
fn codex_totals(usage: &Value) -> TokenTotals {
    let cached = number(usage, "cached_input_tokens");
    TokenTotals {
        input: number(usage, "input_tokens").saturating_sub(cached),
        output: number(usage, "output_tokens"),
        cache_write: number(usage, "cache_write_input_tokens"),
        cache_read: cached,
        reasoning: number(usage, "reasoning_output_tokens"),
    }
}

// ---------------------------------------------------------------------
// Transcript discovery
// ---------------------------------------------------------------------

/// The transcripts one provider offers, and how many the cap left out.
struct Transcripts {
    paths: Vec<PathBuf>,
    skipped: u32,
}

/// `$CLAUDE_CONFIG_DIR/projects`, falling back to `$HOME/.claude/projects`.
fn claude_transcripts(env: &ResolvedEnvironment, cutoff: time::OffsetDateTime) -> Transcripts {
    let root = env
        .get("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env.get("HOME")
                .map(|home| PathBuf::from(home).join(".claude"))
        })
        .map(|root| root.join("projects"));
    recent_transcripts(root.as_deref(), cutoff)
}

/// `$CODEX_HOME/sessions`, falling back to `$HOME/.codex/sessions`.
fn codex_transcripts(env: &ResolvedEnvironment, cutoff: time::OffsetDateTime) -> Transcripts {
    let root = env
        .get("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env.get("HOME")
                .map(|home| PathBuf::from(home).join(".codex"))
        })
        .map(|root| root.join("sessions"));
    recent_transcripts(root.as_deref(), cutoff)
}

/// Every `.jsonl` under `root` touched since `cutoff`, newest first, capped.
///
/// Modification time is the filter, not the records: a transcript last written
/// before the window opened cannot contain a turn inside it, and skipping it
/// costs nothing. Records *inside* a kept file are still checked one by one.
fn recent_transcripts(root: Option<&Path>, cutoff: time::OffsetDateTime) -> Transcripts {
    let Some(root) = root else {
        return Transcripts {
            paths: Vec::new(),
            skipped: 0,
        };
    };
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    collect_jsonl(root, 0, cutoff, &mut found);
    found.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let skipped = u32::try_from(found.len().saturating_sub(SCAN_LIMIT)).unwrap_or(u32::MAX);
    Transcripts {
        paths: found
            .into_iter()
            .take(SCAN_LIMIT)
            .map(|(_, path)| path)
            .collect(),
        skipped,
    }
}

/// Depth-bounded walk. Claude nests two levels (`<slug>/<session>/subagents`)
/// and Codex three (`<year>/<month>/<day>`); anything deeper is not a store we
/// know, and recursing into it would only buy IO.
fn collect_jsonl(
    dir: &Path,
    depth: usize,
    cutoff: time::OffsetDateTime,
    found: &mut Vec<(SystemTime, PathBuf)>,
) {
    const MAX_DEPTH: usize = 4;
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            if depth < MAX_DEPTH {
                collect_jsonl(&path, depth + 1, cutoff, found);
            }
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let modified_at = time::OffsetDateTime::from(modified);
        if modified_at < cutoff {
            continue;
        }
        found.push((modified, path));
    }
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// A non-negative integer field, defaulting to zero. Providers write these as
/// integers today; a float is read rather than dropped in case one changes.
fn number(value: &Value, key: &str) -> u64 {
    value.get(key).map_or(0, |field| {
        field
            .as_u64()
            .or_else(|| field.as_f64().map(|n| n.max(0.) as u64))
            .unwrap_or(0)
    })
}

/// The UTC calendar day a timestamp falls in, as `YYYY-MM-DD`.
fn date_key(at: Timestamp) -> String {
    at.as_offset()
        .to_offset(time::UtcOffset::UTC)
        .date()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use domain::EnvSource;

    fn env(home: &Path) -> ResolvedEnvironment {
        ResolvedEnvironment {
            shell: PathBuf::from("/bin/sh"),
            vars: vec![("HOME".to_owned(), home.display().to_string())],
            path_entries: Vec::new(),
            resolved_at: Timestamp::now(),
            source: EnvSource::ProcessFallback,
        }
    }

    fn write(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut file = fs::File::create(path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
    }

    fn claude_turn(id: &str, at: &str, input: u64, output: u64, cache_read: u64) -> String {
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
                    "cache_read_input_tokens": cache_read,
                    "output_tokens_details": { "thinking_tokens": 3 },
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": 0,
                        "ephemeral_1h_input_tokens": 0
                    }
                }
            }
        })
        .to_string()
    }

    fn codex_event(at: &str, input: u64, cached: u64, output: u64) -> String {
        serde_json::json!({
            "timestamp": at,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": cached,
                        "cache_write_input_tokens": 0,
                        "output_tokens": output,
                        "reasoning_output_tokens": 0,
                        "total_tokens": input + output
                    }
                }
            }
        })
        .to_string()
    }

    /// A timestamp `days_ago` days before now, as RFC-3339.
    fn ago(days: i64) -> String {
        Timestamp::from_offset(time::OffsetDateTime::now_utc() - time::Duration::days(days))
            .to_rfc3339()
    }

    #[test]
    fn claude_transcripts_are_summed_into_tokens_turns_and_cost() {
        let home = tempfile::tempdir().expect("tempdir");
        write(
            &home
                .path()
                .join(".claude/projects/-Users-me-proj/session.jsonl"),
            &[
                claude_turn("msg_1", &ago(1), 1_000_000, 0, 0),
                claude_turn("msg_2", &ago(1), 0, 1_000_000, 0),
            ],
        );

        let analytics = collect(30, &env(home.path()));
        let claude = &analytics.providers[0];
        assert_eq!(claude.provider_id.to_string(), "claude");
        assert_eq!(claude.turns, 2);
        assert_eq!(claude.sessions, 1);
        assert_eq!(claude.tokens.input, 1_000_000);
        assert_eq!(claude.tokens.output, 1_000_000);
        assert_eq!(claude.tokens.reasoning, 6);
        // $5 of input plus $25 of output on Opus.
        assert_eq!(claude.cost_micros, 30 * domain::MICROS_PER_USD);
        assert_eq!(claude.unpriced_turns, 0);
        assert_eq!(claude.top_model.as_deref(), Some("claude-opus-5"));
        assert_eq!(analytics.active_days(), 1);
        assert_eq!(analytics.skipped, 0);
    }

    #[test]
    fn a_resumed_transcript_does_not_bill_its_copied_records_twice() {
        let home = tempfile::tempdir().expect("tempdir");
        let root = home.path().join(".claude/projects/-Users-me-proj");
        let turn = claude_turn("msg_1", &ago(1), 100, 50, 0);
        write(&root.join("first.jsonl"), std::slice::from_ref(&turn));
        write(
            &root.join("resumed.jsonl"),
            &[turn, claude_turn("msg_2", &ago(1), 10, 5, 0)],
        );

        let claude = &collect(30, &env(home.path())).providers[0];
        assert_eq!(claude.turns, 2, "the copied record is counted once");
        assert_eq!(claude.tokens.input, 110);
    }

    #[test]
    fn codex_events_are_differenced_because_the_counter_is_cumulative() {
        let home = tempfile::tempdir().expect("tempdir");
        write(
            &home
                .path()
                .join(".codex/sessions/2026/08/26/rollout-test.jsonl"),
            &[
                serde_json::json!({
                    "timestamp": ago(1),
                    "type": "turn_context",
                    "payload": { "model": "gpt-5.6-sol" }
                })
                .to_string(),
                codex_event(&ago(1), 1_000, 400, 100),
                codex_event(&ago(1), 3_000, 1_400, 250),
            ],
        );

        let codex = &collect(30, &env(home.path())).providers[0];
        assert_eq!(codex.provider_id.to_string(), "codex");
        assert_eq!(codex.turns, 2);
        // Fresh input is 600 then 1_000: the cached half never counts as new.
        assert_eq!(codex.tokens.input, 1_600);
        assert_eq!(codex.tokens.cache_read, 1_400);
        assert_eq!(codex.tokens.output, 250);
        assert_eq!(codex.top_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            codex.unpriced_turns, 2,
            "an OpenAI model carries no price in this build"
        );
        assert_eq!(codex.cost_micros, 0);
    }

    #[test]
    fn records_older_than_the_window_are_left_out() {
        let home = tempfile::tempdir().expect("tempdir");
        write(
            &home
                .path()
                .join(".claude/projects/-Users-me-proj/session.jsonl"),
            &[
                claude_turn("msg_old", &ago(40), 1_000, 0, 0),
                claude_turn("msg_new", &ago(2), 7, 0, 0),
            ],
        );

        let claude = &collect(30, &env(home.path())).providers[0];
        assert_eq!(claude.turns, 1);
        assert_eq!(claude.tokens.input, 7);
    }

    #[test]
    fn an_empty_home_reports_nothing_rather_than_zeroes() {
        let home = tempfile::tempdir().expect("tempdir");
        let analytics = collect(30, &env(home.path()));
        assert!(analytics.providers.is_empty());
        assert!(analytics.daily.is_empty());
        assert_eq!(analytics.scanned, 0);
    }

    #[test]
    fn the_window_is_clamped_rather_than_trusted() {
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            collect(0, &env(home.path())).window_days,
            DEFAULT_WINDOW_DAYS
        );
        assert_eq!(
            collect(10_000, &env(home.path())).window_days,
            MAX_WINDOW_DAYS
        );
    }
}
