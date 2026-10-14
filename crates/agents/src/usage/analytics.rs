//! Token analytics read from the transcripts the agent CLIs already write.
//!
//! Billing facts stay in this crate (principle P2): Claude's `message.usage`,
//! deduped on `message.id`, and Codex's cumulative `total_token_usage`.
//! Stores and counting rules: `docs/agents.md`.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use domain::{
    AccountAnalytics, AgentProfile, AgentProfileId, AgentProviderId, DailyUsage, ProviderAnalytics,
    ResolvedEnvironment, Timestamp, TokenTotals, UsageAnalytics,
};
use serde::Deserialize;
use serde_json::Value;

use super::pricing::{price_for, ModelPrice};
use crate::jsonl::{self, present, text_of, Object, Text};

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
/// Longest record parsed. Larger than discovery's cap: an assistant record
/// whose tool call carries a whole file still carries a billed turn.
const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;
/// Directory entries one store's walk may visit.
const MAX_ENTRIES: usize = 20_000;

/// Read every supported provider's transcripts and aggregate them.
///
/// `window_days` is clamped into `1..=MAX_WINDOW_DAYS`; `0` means the default.
/// The result is ordered by tokens descending, so the busiest provider leads.
///
/// `profiles` are the launch profiles: each one that moves a provider's
/// config directory moves its transcripts with it, so a machine whose work runs
/// under a `Personal` profile would otherwise report an empty month. Every
/// account is read and the totals are the sum — this page is what the machine
/// spent, not what one login did — with the per-account split in
/// [`ProviderAnalytics::accounts`].
#[must_use]
pub fn collect(
    window_days: u16,
    env: &ResolvedEnvironment,
    profiles: &[AgentProfile],
) -> UsageAnalytics {
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
        ("claude", claude_transcripts(env, profiles, cutoff)),
        ("codex", codex_transcripts(env, profiles, cutoff)),
    ] {
        skipped += files.skipped;
        let mut totals = Totals::new(&files.stores);
        for (path, store) in &files.paths {
            totals.account = *store;
            let Scanned { counted, unread } = match provider {
                "claude" => scan_claude(path, cutoff, &mut totals),
                _ => scan_codex(path, cutoff, &mut totals),
            };
            scanned += 1;
            if unread.any() {
                skipped += 1;
            }
            if counted {
                totals.sessions += 1;
                totals.accounts[*store].sessions += 1;
            }
        }
        for (date, tokens) in &totals.daily {
            *daily.entry(date.clone()).or_default() += tokens;
        }
        if let Some(analytics) = totals.finish(AgentProviderId::new(provider)) {
            providers.push(analytics);
        }
    }

    providers.sort_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));

    UsageAnalytics {
        providers,
        daily: daily_series(daily),
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
    daily: BTreeMap<String, u64>,
    /// One entry per [`Store`], indexed like [`Transcripts::stores`].
    accounts: Vec<AccountAnalytics>,
    /// The account the transcript being read belongs to.
    account: usize,
}

impl Totals {
    fn new(stores: &[Store]) -> Self {
        Self {
            tokens: TokenTotals::default(),
            sessions: 0,
            turns: 0,
            cost_micros: 0,
            unpriced_turns: 0,
            per_model: HashMap::new(),
            worked_secs: 0,
            first: None,
            last: None,
            seen: HashSet::new(),
            daily: BTreeMap::new(),
            accounts: stores
                .iter()
                .map(|store| AccountAnalytics {
                    profile_id: store.profile_id,
                    tokens: TokenTotals::default(),
                    sessions: 0,
                    turns: 0,
                    cost_micros: 0,
                    unpriced_turns: 0,
                })
                .collect(),
            account: 0,
        }
    }

    /// Record one billed turn.
    fn turn(&mut self, turn: &Turn, at: Option<Timestamp>) {
        let account = &mut self.accounts[self.account];
        self.tokens.add(&turn.tokens);
        account.tokens.add(&turn.tokens);
        self.turns += 1;
        account.turns += 1;
        match turn.model.as_deref().and_then(price_for) {
            Some(price) => {
                let cost = turn.cost_micros(price);
                self.cost_micros += cost;
                account.cost_micros += cost;
            }
            None => {
                self.unpriced_turns += 1;
                account.unpriced_turns += 1;
            }
        }
        if let Some(model) = &turn.model {
            *self.per_model.entry(model.clone()).or_default() += turn.tokens.total();
        }
        if let Some(at) = at {
            self.first = Some(self.first.map_or(at, |first| first.min(at)));
            self.last = Some(self.last.map_or(at, |last| last.max(at)));
            *self.daily.entry(date_key(at)).or_default() += turn.tokens.total();
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
        let mut accounts: Vec<_> = self
            .accounts
            .into_iter()
            .filter(|account| account.turns > 0)
            .collect();
        accounts.sort_by(|a, b| b.tokens.total().cmp(&a.tokens.total()));
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
            accounts,
            daily: daily_series(self.daily),
        })
    }
}

fn daily_series(daily: BTreeMap<String, u64>) -> Vec<DailyUsage> {
    daily
        .into_iter()
        .map(|(date, tokens)| DailyUsage { date, tokens })
        .collect()
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

/// What one transcript contributed.
struct Scanned {
    counted: bool,
    /// Records the scan could not read, so the transcript is reported skipped
    /// as well as scanned.
    unread: jsonl::Unread,
}

/// The fields a billed Claude turn is read from. Everything else, including
/// the message `content` that dominates a record, is skipped unallocated.
#[derive(Deserialize)]
struct ClaudeRecord<'a> {
    #[serde(rename = "type", borrow, default)]
    kind: Text<'a>,
    #[serde(borrow, default)]
    timestamp: Text<'a>,
    #[serde(borrow, default)]
    message: Object<ClaudeMessage<'a>>,
}

impl ClaudeRecord<'_> {
    /// The typed parse refused the line, so it is read as before: a repeated
    /// key keeps its last value, as `serde_json`'s map does.
    fn from_value(value: &Value) -> ClaudeRecord<'static> {
        ClaudeRecord {
            kind: text_of(value, "type"),
            timestamp: text_of(value, "timestamp"),
            message: Object(
                value
                    .get("message")
                    .filter(|message| message.is_object())
                    .map(|message| ClaudeMessage {
                        id: text_of(message, "id"),
                        model: text_of(message, "model"),
                        usage: message.get("usage").cloned(),
                    }),
            ),
        }
    }
}

#[derive(Deserialize)]
struct ClaudeMessage<'a> {
    #[serde(borrow, default)]
    id: Text<'a>,
    #[serde(borrow, default)]
    model: Text<'a>,
    #[serde(default, deserialize_with = "present")]
    usage: Option<Value>,
}

/// Read one Claude transcript.
fn scan_claude(path: &Path, cutoff: time::OffsetDateTime, totals: &mut Totals) -> Scanned {
    let Ok(file) = fs::File::open(path) else {
        return Scanned {
            counted: false,
            unread: jsonl::Unread::default(),
        };
    };
    let mut counted = false;
    let mut span: Option<(Timestamp, Timestamp)> = None;
    let unread = jsonl::for_each(BufReader::new(file), MAX_RECORD_BYTES, |line| {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        // The cheap gate first: most lines in a transcript are user turns and
        // tool traffic, and parsing all of them costs more than the read.
        if !line.contains("\"usage\"") {
            return;
        }
        let Some(record) = typed_or_value(line, ClaudeRecord::from_value) else {
            return;
        };
        if record.kind.as_str() != Some("assistant") {
            return;
        }
        let Some(message) = record.message.0 else {
            return;
        };
        let Some(usage) = message.usage else {
            return;
        };
        let at = record
            .timestamp
            .as_str()
            .and_then(|raw| Timestamp::parse_rfc3339(raw).ok());
        // A resumed transcript carries records older than the window; count
        // them only when they fall inside it, so the window means something.
        if at.is_some_and(|at| at.as_offset() < cutoff) {
            return;
        }
        let id = message
            .id
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}:{line:.120}", path.display()));
        if !totals.is_new(&id) {
            return;
        }

        let mut turn = Turn {
            model: message.model.as_str().map(str::to_owned),
            ..Turn::default()
        };
        turn.tokens = TokenTotals {
            input: number(&usage, "input_tokens"),
            output: number(&usage, "output_tokens"),
            cache_write: number(&usage, "cache_creation_input_tokens"),
            cache_read: number(&usage, "cache_read_input_tokens"),
            reasoning: usage
                .get("output_tokens_details")
                .map_or(0, |details| number(details, "thinking_tokens")),
        };
        if let Some(creation) = usage.get("cache_creation") {
            turn.cache_write_5m = number(creation, "ephemeral_5m_input_tokens");
            turn.cache_write_1h = number(creation, "ephemeral_1h_input_tokens");
        }
        if turn.is_empty() {
            return;
        }
        if let Some(at) = at {
            span = Some(match span {
                Some((first, last)) => (first.min(at), last.max(at)),
                None => (at, at),
            });
        }
        totals.turn(&turn, at);
        counted = true;
    });
    totals.worked(span);
    Scanned { counted, unread }
}

// ---------------------------------------------------------------------
// Codex CLI
// ---------------------------------------------------------------------

#[derive(Deserialize)]
struct CodexRecord<'a> {
    #[serde(borrow, default)]
    timestamp: Text<'a>,
    #[serde(borrow, default)]
    payload: Object<CodexPayload<'a>>,
}

impl CodexRecord<'_> {
    fn from_value(value: &Value) -> CodexRecord<'static> {
        CodexRecord {
            timestamp: text_of(value, "timestamp"),
            payload: Object(
                value
                    .get("payload")
                    .filter(|payload| payload.is_object())
                    .map(|payload| CodexPayload {
                        model: text_of(payload, "model"),
                        info: payload.get("info").cloned(),
                    }),
            ),
        }
    }
}

#[derive(Deserialize)]
struct CodexPayload<'a> {
    #[serde(borrow, default)]
    model: Text<'a>,
    #[serde(default, deserialize_with = "present")]
    info: Option<Value>,
}

/// Read one Codex rollout.
///
/// `total_token_usage` is cumulative over the session, so a turn is the
/// *difference* between consecutive events. Summing the events themselves would
/// multiply a long session by its own length.
fn scan_codex(path: &Path, cutoff: time::OffsetDateTime, totals: &mut Totals) -> Scanned {
    let Ok(file) = fs::File::open(path) else {
        return Scanned {
            counted: false,
            unread: jsonl::Unread::default(),
        };
    };
    let mut counted = false;
    let mut span: Option<(Timestamp, Timestamp)> = None;
    let mut previous = TokenTotals::default();
    let mut model: Option<String> = None;
    let unread = jsonl::for_each(BufReader::new(file), MAX_RECORD_BYTES, |line| {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        let is_usage = line.contains("\"token_count\"");
        if !is_usage && !line.contains("\"turn_context\"") {
            return;
        }
        let Some(record) = typed_or_value(line, CodexRecord::from_value) else {
            return;
        };
        let Some(payload) = record.payload.0 else {
            return;
        };
        if !is_usage {
            // The model can change mid-session; the latest one before a turn is
            // the one that turn ran on.
            if let Some(name) = payload.model.as_str() {
                model = Some(name.to_owned());
            }
            return;
        }
        let Some(cumulative) = payload
            .info
            .as_ref()
            .and_then(|info| info.get("total_token_usage"))
        else {
            return;
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
            return;
        }
        let at = record
            .timestamp
            .as_str()
            .and_then(|raw| Timestamp::parse_rfc3339(raw).ok());
        if at.is_some_and(|at| at.as_offset() < cutoff) {
            return;
        }
        if let Some(at) = at {
            span = Some(match span {
                Some((first, last)) => (first.min(at), last.max(at)),
                None => (at, at),
            });
        }
        totals.turn(&turn, at);
        counted = true;
    });
    totals.worked(span);
    Scanned { counted, unread }
}

/// One record, typed so the bulk of it is skipped unallocated, or read as a
/// `Value` when the typed parse refuses it. `None` for a line that is not
/// JSON, or not an object.
fn typed_or_value<'a, T: Deserialize<'a>>(line: &'a str, from_value: fn(&Value) -> T) -> Option<T> {
    match serde_json::from_str::<Object<T>>(line) {
        Ok(Object(record)) => record,
        Err(_) => serde_json::from_str::<Value>(line)
            .ok()
            .filter(Value::is_object)
            .map(|value| from_value(&value)),
    }
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

/// One directory a provider's transcripts live in, and whose account it is.
struct Store {
    root: PathBuf,
    profile_id: Option<AgentProfileId>,
}

/// The transcripts one provider offers, each with the index of its
/// [`Store`], and how many the cap left out.
struct Transcripts {
    stores: Vec<Store>,
    paths: Vec<(PathBuf, usize)>,
    skipped: u32,
}

#[derive(Default)]
struct TranscriptCandidates {
    newest: BinaryHeap<Reverse<(SystemTime, PathBuf, usize)>>,
    skipped: u32,
}

impl TranscriptCandidates {
    fn insert(&mut self, modified: SystemTime, path: PathBuf, store: usize) {
        let candidate = Reverse((modified, path, store));
        if self.newest.len() < SCAN_LIMIT {
            self.newest.push(candidate);
            return;
        }
        self.skipped = self.skipped.saturating_add(1);
        if let Some(mut oldest) = self.newest.peek_mut() {
            if candidate < *oldest {
                *oldest = candidate;
            }
        }
    }

    fn finish(self, stores: Vec<Store>) -> Transcripts {
        Transcripts {
            stores,
            paths: self
                .newest
                .into_sorted_vec()
                .into_iter()
                .map(|Reverse((_, path, store))| (path, store))
                .collect(),
            skipped: self.skipped,
        }
    }
}

/// `$CLAUDE_CONFIG_DIR/projects` and every profile's, falling back to
/// `$HOME/.claude/projects`.
fn claude_transcripts(
    env: &ResolvedEnvironment,
    profiles: &[AgentProfile],
    cutoff: time::OffsetDateTime,
) -> Transcripts {
    let default = env.get("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| {
        env.get("HOME")
            .map(|home| PathBuf::from(home).join(".claude"))
    });
    recent_transcripts(stores(default, "claude", profiles, env, "projects"), cutoff)
}

/// `$CODEX_HOME/sessions` and every profile's, falling back to
/// `$HOME/.codex/sessions`.
fn codex_transcripts(
    env: &ResolvedEnvironment,
    profiles: &[AgentProfile],
    cutoff: time::OffsetDateTime,
) -> Transcripts {
    let default = env.get("CODEX_HOME").map(PathBuf::from).or_else(|| {
        env.get("HOME")
            .map(|home| PathBuf::from(home).join(".codex"))
    });
    recent_transcripts(stores(default, "codex", profiles, env, "sessions"), cutoff)
}

/// Every directory `provider`'s transcripts may live in: one per launch
/// profile that moved it, plus the account the resolved environment points at,
/// with `leaf` appended to each directory.
///
/// Duplicates are dropped by canonical path, because a profile pointing at the
/// default account would otherwise have every one of its turns counted twice.
/// Profiles come first so such an account is labelled by the profile's name.
fn stores(
    default: Option<PathBuf>,
    provider: &str,
    profiles: &[AgentProfile],
    env: &ResolvedEnvironment,
    leaf: &str,
) -> Vec<Store> {
    let home = env.get("HOME").map(PathBuf::from);
    let dirs = profiles
        .iter()
        .filter_map(|p| {
            let dir = p.config_dir.as_deref()?;
            (p.provider_id.as_str() == provider).then(|| {
                let dir = match home.as_deref() {
                    Some(home) => AgentProfile::resolve_config_dir(dir, home),
                    None => dir.to_path_buf(),
                };
                (dir, Some(p.id))
            })
        })
        .chain(default.map(|dir| (dir, None)));

    let mut seen = HashSet::new();
    let mut stores = Vec::new();
    for (dir, profile_id) in dirs {
        let root = dir.join(leaf);
        let key = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if seen.insert(key) {
            stores.push(Store { root, profile_id });
        }
    }
    stores
}

/// Every `.jsonl` under `roots` touched since `cutoff`, newest first, capped.
/// The cap is over all of them together: the page reads the busiest 500
/// transcripts on the machine, not 500 per account.
///
/// Modification time is the filter, not the records: a transcript last written
/// before the window opened cannot contain a turn inside it, and skipping it
/// costs nothing. Records *inside* a kept file are still checked one by one.
fn recent_transcripts(stores: Vec<Store>, cutoff: time::OffsetDateTime) -> Transcripts {
    let mut found = TranscriptCandidates::default();
    for (index, store) in stores.iter().enumerate() {
        let mut budget = Budget {
            remaining: MAX_ENTRIES,
            exhausted: false,
        };
        collect_jsonl(&store.root, index, 0, cutoff, &mut budget, &mut found);
        if budget.exhausted {
            found.skipped = found.skipped.saturating_add(1);
        }
    }
    found.finish(stores)
}

/// Directory entries left to visit in one store.
struct Budget {
    remaining: usize,
    exhausted: bool,
}

/// Depth-bounded walk. Claude nests two levels (`<slug>/<session>/subagents`)
/// and Codex three (`<year>/<month>/<day>`); anything deeper is not a store we
/// know, and recursing into it would only buy IO.
fn collect_jsonl(
    dir: &Path,
    store: usize,
    depth: usize,
    cutoff: time::OffsetDateTime,
    budget: &mut Budget,
    found: &mut TranscriptCandidates,
) {
    const MAX_DEPTH: usize = 4;
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if budget.remaining == 0 {
            budget.exhausted = true;
            return;
        }
        budget.remaining -= 1;
        // The entry's own type is free on most file systems; a stat is paid
        // only for a transcript.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_DEPTH {
                collect_jsonl(&entry.path(), store, depth + 1, cutoff, budget, found);
            }
            continue;
        }
        if Path::new(&entry.file_name())
            .extension()
            .is_none_or(|ext| ext != "jsonl")
        {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        let modified_at = time::OffsetDateTime::from(modified);
        if modified_at < cutoff {
            continue;
        }
        found.insert(modified, entry.path(), store);
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

    #[test]
    fn transcript_candidates_keep_only_the_newest_and_count_every_skip() {
        let candidates: Vec<_> = (0..SCAN_LIMIT * 5)
            .map(|i| {
                (
                    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs((i % 701) as u64),
                    PathBuf::from(format!("{i:04}.jsonl")),
                )
            })
            .collect();
        let mut expected = candidates.clone();
        expected.sort_unstable_by(|a, b| b.cmp(a));
        let expected: Vec<_> = expected
            .into_iter()
            .take(SCAN_LIMIT)
            .map(|(_, path)| path)
            .collect();

        for reverse in [false, true] {
            let mut found = TranscriptCandidates::default();
            for i in 0..candidates.len() {
                let index = if reverse { candidates.len() - i - 1 } else { i };
                let (modified, path) = &candidates[index];
                found.insert(*modified, path.clone(), 0);
                assert!(found.newest.len() <= SCAN_LIMIT);
            }
            let found = found.finish(Vec::new());
            let paths: Vec<_> = found.paths.into_iter().map(|(path, _)| path).collect();
            assert_eq!(paths, expected);
            assert_eq!(found.skipped as usize, candidates.len() - SCAN_LIMIT);
        }
    }

    #[test]
    fn transcript_limit_applies_across_roots_after_the_cutoff_filter() {
        let dir = tempfile::tempdir().unwrap();
        let roots = [dir.path().join("one"), dir.path().join("two")];
        for root in &roots {
            fs::create_dir(root).unwrap();
        }
        let cutoff = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        let mut expected = Vec::new();
        for i in 0..SCAN_LIMIT + 5 {
            let path = roots[i % roots.len()].join(format!("{i:04}.jsonl"));
            let file = fs::File::create(&path).unwrap();
            file.set_modified(cutoff + std::time::Duration::from_secs(i as u64))
                .unwrap();
            expected.push(path);
        }
        let old = fs::File::create(roots[0].join("old.jsonl")).unwrap();
        old.set_modified(SystemTime::UNIX_EPOCH).unwrap();
        fs::write(roots[1].join("ignored.txt"), "").unwrap();
        expected.reverse();
        expected.truncate(SCAN_LIMIT);

        let stores = roots
            .iter()
            .map(|root| Store {
                root: root.clone(),
                profile_id: None,
            })
            .collect();
        let found = recent_transcripts(stores, cutoff.into());
        let paths: Vec<_> = found.paths.into_iter().map(|(path, _)| path).collect();
        assert_eq!(paths, expected);
        assert_eq!(found.skipped, 5);
    }

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

        let analytics = collect(30, &env(home.path()), &[]);
        let claude = &analytics.providers[0];
        assert_eq!(claude.provider_id.to_string(), "claude");
        assert_eq!(claude.daily, analytics.daily);
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

    fn profile(provider: &str, dir: &str) -> domain::AgentProfile {
        domain::AgentProfile {
            id: domain::AgentProfileId::new(),
            provider_id: AgentProviderId::new(provider),
            name: "Personal".to_owned(),
            executable: None,
            config_dir: Some(PathBuf::from(dir)),
            args: Vec::new(),
            created_at: Timestamp::now(),
        }
    }

    /// A profile moves the config directory, and the transcripts move with it
    /// Reading only the default account would report an empty month
    /// for a machine whose work all runs under a `Personal` profile.
    #[test]
    fn a_profiles_account_is_read_alongside_the_default_one() {
        let home = tempfile::tempdir().expect("tempdir");
        write(
            &home
                .path()
                .join(".claude/projects/-Users-me-proj/default.jsonl"),
            &[claude_turn("msg_1", &ago(1), 100, 50, 0)],
        );
        // The profile's own account, named the way the form suggests.
        write(
            &home
                .path()
                .join(".claude-personal/projects/-Users-me-proj/personal.jsonl"),
            &[claude_turn("msg_2", &ago(1), 1_000, 500, 0)],
        );

        let profiles = [
            profile("claude", ".claude-personal"),
            // Another provider's profile must not drag Claude's scan sideways.
            profile("codex", ".codex-work"),
        ];
        let claude = &collect(30, &env(home.path()), &profiles).providers[0];
        assert_eq!(claude.turns, 2, "both accounts are summed");
        assert_eq!(claude.tokens.input, 1_100);
        assert_eq!(claude.sessions, 2);

        let split: Vec<_> = claude
            .accounts
            .iter()
            .map(|account| (account.profile_id, account.tokens.input, account.sessions))
            .collect();
        assert_eq!(
            split,
            [(Some(profiles[0].id), 1_000, 1), (None, 100, 1)],
            "each account keeps its own share, busiest first"
        );
    }

    /// The same directory named twice — a profile pointing at the default
    /// account — must not bill its turns twice.
    #[test]
    fn an_account_named_twice_is_read_once() {
        let home = tempfile::tempdir().expect("tempdir");
        write(
            &home
                .path()
                .join(".claude/projects/-Users-me-proj/session.jsonl"),
            &[claude_turn("msg_1", &ago(1), 100, 50, 0)],
        );

        let named = profile("claude", ".claude");
        let claude = &collect(30, &env(home.path()), std::slice::from_ref(&named)).providers[0];
        assert_eq!(claude.turns, 1);
        assert_eq!(claude.tokens.input, 100);
        assert_eq!(claude.accounts.len(), 1);
        assert_eq!(
            claude.accounts[0].profile_id,
            Some(named.id),
            "the account carries the profile's name rather than \"default\""
        );
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

        let claude = &collect(30, &env(home.path()), &[]).providers[0];
        assert_eq!(claude.turns, 2, "the copied record is counted once");
        assert_eq!(claude.tokens.input, 110);
    }

    /// The record is skipped before it is resident, and the transcript is
    /// reported as skipped so the page does not look exhaustive.
    #[test]
    fn a_record_over_the_cap_is_skipped_and_reported() {
        let home = tempfile::tempdir().expect("tempdir");
        let huge = serde_json::json!({
            "type": "assistant",
            "timestamp": ago(1),
            "message": {
                "id": "msg_huge",
                "usage": { "input_tokens": 1 },
                "content": [{ "type": "text", "text": "x".repeat(MAX_RECORD_BYTES as usize) }]
            }
        })
        .to_string();
        write(
            &home
                .path()
                .join(".claude/projects/-Users-me-proj/session.jsonl"),
            &[
                claude_turn("msg_1", &ago(1), 10, 0, 0),
                huge,
                claude_turn("msg_2", &ago(1), 20, 0, 0),
            ],
        );

        let analytics = collect(30, &env(home.path()), &[]);
        let claude = &analytics.providers[0];
        assert_eq!(claude.turns, 2);
        assert_eq!(claude.tokens.input, 30);
        assert_eq!(analytics.scanned, 1);
        assert_eq!(analytics.skipped, 1);
    }

    /// Fields are read the way `Value::as_str` reads them: a mistyped one is
    /// absent, not a reason to drop the turn.
    #[test]
    fn a_mistyped_field_reads_as_absent_rather_than_dropping_the_turn() {
        let home = tempfile::tempdir().expect("tempdir");
        let numeric_id = serde_json::json!({
            "type": "assistant",
            "timestamp": ago(1),
            "message": {
                "id": 7,
                "model": ["not", "a", "name"],
                "content": { "nested": [1, 2, { "deep": null }] },
                "usage": { "input_tokens": 5, "output_tokens": 1 }
            }
        })
        .to_string();
        write(
            &home
                .path()
                .join(".claude/projects/-Users-me-proj/session.jsonl"),
            &[numeric_id],
        );

        let claude = &collect(30, &env(home.path()), &[]).providers[0];
        assert_eq!(claude.turns, 1);
        assert_eq!(claude.tokens.input, 5);
        assert_eq!(claude.unpriced_turns, 1, "no model name, so no price");
    }

    type ClaudeView = (
        Option<String>,
        Option<String>,
        Option<(Option<String>, Option<String>, Option<Value>)>,
    );

    fn claude_view(record: &ClaudeRecord) -> ClaudeView {
        let text = |text: &Text| text.as_str().map(str::to_owned);
        (
            text(&record.kind),
            text(&record.timestamp),
            record.message.0.as_ref().map(|message| {
                (
                    text(&message.id),
                    text(&message.model),
                    message.usage.clone(),
                )
            }),
        )
    }

    /// The typed records stand in for `Value`, so every shape — a repeated
    /// key, a `null`, a mistyped or non-object field — reads the same.
    #[test]
    fn typed_records_read_exactly_what_a_value_reads() {
        for line in [
            r#"{"type":"assistant","timestamp":"t","message":{"id":"m1","model":"x","usage":{"input_tokens":1},"content":[{"type":"text","text":"big"}]}}"#,
            r#"{"type":"assistant","message":null,"usage":1}"#,
            r#"{"type":"assistant","message":"flat \"usage\""}"#,
            r#"{"type":"assistant","message":[{"usage":{}}]}"#,
            r#"{"type":"assistant","message":{"usage":null,"id":7,"model":["a"]}}"#,
            r#"{"type":"assistant","type":"user","message":{"usage":{}}}"#,
            r#"{"type":"assistant","message":{"id":"a","id":"b","usage":{}}}"#,
            r#"{"type":"assistant","timestamp":"2026\u002d01","message":{"id":"\u0041","usage":{}}}"#,
        ] {
            let value: Value = serde_json::from_str(line).unwrap();
            let typed = typed_or_value(line, ClaudeRecord::from_value).unwrap();
            assert_eq!(
                claude_view(&typed),
                claude_view(&ClaudeRecord::from_value(&value)),
                "{line}"
            );
        }
        assert!(typed_or_value(r#"["assistant"]"#, ClaudeRecord::from_value).is_none());

        for line in [
            r#"{"timestamp":"t","payload":{"type":"token_count","info":{"total_token_usage":{}}}}"#,
            r#"{"payload":null}"#,
            r#"{"payload":"turn_context"}"#,
            r#"{"payload":["model","x"]}"#,
            r#"{"payload":{"model":3,"info":null}}"#,
            r#"{"payload":{"model":"a","model":"b"}}"#,
        ] {
            let value: Value = serde_json::from_str(line).unwrap();
            let typed = typed_or_value(line, CodexRecord::from_value).unwrap();
            let expected = CodexRecord::from_value(&value);
            let view = |record: &CodexRecord| {
                (
                    record.timestamp.as_str().map(str::to_owned),
                    record.payload.0.as_ref().map(|payload| {
                        (
                            payload.model.as_str().map(str::to_owned),
                            payload.info.clone(),
                        )
                    }),
                )
            };
            assert_eq!(view(&typed), view(&expected), "{line}");
        }
    }

    #[test]
    fn the_directory_walk_stops_at_its_entry_budget_and_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..5 {
            fs::write(dir.path().join(format!("{i}.jsonl")), "").expect("write");
        }
        let mut budget = Budget {
            remaining: 3,
            exhausted: false,
        };
        let mut found = TranscriptCandidates::default();
        collect_jsonl(
            dir.path(),
            0,
            0,
            time::OffsetDateTime::UNIX_EPOCH,
            &mut budget,
            &mut found,
        );
        assert!(budget.exhausted);
        assert_eq!(found.newest.len(), 3);
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

        let codex = &collect(30, &env(home.path()), &[]).providers[0];
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

        let claude = &collect(30, &env(home.path()), &[]).providers[0];
        assert_eq!(claude.turns, 1);
        assert_eq!(claude.tokens.input, 7);
    }

    #[test]
    fn an_empty_home_reports_nothing_rather_than_zeroes() {
        let home = tempfile::tempdir().expect("tempdir");
        let analytics = collect(30, &env(home.path()), &[]);
        assert!(analytics.providers.is_empty());
        assert!(analytics.daily.is_empty());
        assert_eq!(analytics.scanned, 0);
    }

    #[test]
    fn the_window_is_clamped_rather_than_trusted() {
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            collect(0, &env(home.path()), &[]).window_days,
            DEFAULT_WINDOW_DAYS
        );
        assert_eq!(
            collect(10_000, &env(home.path()), &[]).window_days,
            MAX_WINDOW_DAYS
        );
    }
}
