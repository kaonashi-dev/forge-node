//! Parallel reviewers: which hypotheses the agents hunt, which files each one is
//! pointed at, and how their answers become one set of findings. Pure; the
//! daemon launches the agents and reads their answers.

use std::fmt::Write as _;

use domain::{DiffFile, Finding, Hypothesis, HypothesisChoice};

pub const MAX_AGENTS: usize = 8;

/// The order hypotheses fill the reviewers the diff's signals left: the default three first.
pub const ORDER: [Hypothesis; 19] = [
    Hypothesis::AuthBypass,
    Hypothesis::BrokenContract,
    Hypothesis::ErrorSwallow,
    Hypothesis::RaceCondition,
    Hypothesis::UntrustedRepoInput,
    Hypothesis::LifecycleCancelRetry,
    Hypothesis::MemoryDiskDrift,
    Hypothesis::TransitionCleanup,
    Hypothesis::NPlusOneQuery,
    Hypothesis::MissingValidation,
    Hypothesis::RetryNesting,
    Hypothesis::DefaultMismatch,
    Hypothesis::ResourceLeak,
    Hypothesis::DeadCode,
    Hypothesis::InconsistentPattern,
    Hypothesis::CrossLayerMismatch,
    Hypothesis::MissingTest,
    Hypothesis::MigrationGap,
    Hypothesis::ConfigDrift,
];

#[must_use]
pub fn instruction(hypothesis: Hypothesis) -> &'static str {
    match hypothesis {
        Hypothesis::RaceCondition => "Look for races: shared state touched without the lock that guards it elsewhere, async work whose ordering the change now assumes, a check and its use separated by an await or a thread hop.",
        Hypothesis::AuthBypass => "Look for a request that reaches data or an action without the authorization check its siblings perform: a missing check, one done after the effect, or a default that allows.",
        Hypothesis::RetryNesting => "Look for retries inside retries and timeouts multiplied by them: a retried call that itself retries, or a loop whose total time is no longer bounded.",
        Hypothesis::BrokenContract => "Look for a changed signature, return value, error or invariant whose callers still assume the old one. Read every caller the graph lists.",
        Hypothesis::DefaultMismatch => "Look for one default defined twice with different values: config against code, client against server, a constant against its documentation.",
        Hypothesis::ResourceLeak => "Look for a file, connection, lock, process or subscription that an error path no longer closes or releases.",
        Hypothesis::ErrorSwallow => "Look for errors that are caught or ignored without being logged, returned or turned into a visible failure, so a caller proceeds as if it succeeded.",
        Hypothesis::NPlusOneQuery => "Look for a query, request or subprocess issued once per item of a loop where one batched call would do.",
        Hypothesis::MissingValidation => "Look for external input (a request, a file, a subprocess's output, a wire message) used without checking its size, range or shape first.",
        Hypothesis::DeadCode => "Look for code the change leaves unreachable or exported functions that nothing calls any more.",
        Hypothesis::InconsistentPattern => "Compare the changed functions with the similar ones the graph lists, and report where the change diverges from how the project does the same thing elsewhere.",
        Hypothesis::CrossLayerMismatch => "Look for a value two layers must agree on (frontend and backend, protocol and handler, schema and code) that the change updates on one side only.",
        Hypothesis::MissingTest => "Look for a new branch or behaviour with no test that would fail if it broke.",
        Hypothesis::MigrationGap => "Look for a schema or migration change the code reading or writing it does not follow, or the reverse.",
        Hypothesis::ConfigDrift => "Look for a configuration key added, renamed or removed in one place but not in the code, the defaults or the documentation that read it.",
        Hypothesis::UntrustedRepoInput => "Look for paths, symlinks, files and configuration the checked-out repository controls that the change uses as a write target, a policy or a command: a path joined without being canonicalized inside its root, a symlink followed out of the checkout, a repository file that decides what runs or what is allowed without a person approving it.",
        Hypothesis::MemoryDiskDrift => "Look for state that lives only in memory and is lost or wrong after a restart, an incremental result that differs from what a full rebuild would produce, and a cache that is not invalidated when its source changes. Compare what is written with what is read back.",
        Hypothesis::LifecycleCancelRetry => "Look for a cancel that does not reach the children, threads or processes the work started, a retry or restart that drops the original request's settings, a start that can run twice, and a flag or guard that one exit path (an error, an early return, a panic) leaves set.",
        Hypothesis::TransitionCleanup => "Look for what a transition leaves behind: pending requests, reads or queued work not reset on a reconnect, restart or replacement; an old answer accepted because its generation or id is not checked; a limit enforced when reading that writing does not enforce, or the reverse.",
        _ => "Look for defects the change introduces.",
    }
}

/// Words in a file's path or patch that point a hypothesis at it.
#[must_use]
pub fn keywords(hypothesis: Hypothesis) -> &'static [&'static str] {
    match hypothesis {
        Hypothesis::RaceCondition => &[
            "mutex", "lock", "async", "await", "thread", "atomic", "spawn", "channel",
        ],
        Hypothesis::AuthBypass => &[
            "auth",
            "permission",
            "role",
            "token",
            "session",
            "login",
            "admin",
            "access",
        ],
        Hypothesis::RetryNesting => &["retry", "retries", "backoff", "timeout", "attempt"],
        Hypothesis::BrokenContract => &[
            "pub fn",
            "export",
            "interface",
            "trait",
            "fn ",
            "def ",
            "func ",
        ],
        Hypothesis::DefaultMismatch => &["default", "const ", "config"],
        Hypothesis::ResourceLeak => &[
            "open", "close", "drop", "file", "conn", "socket", "lock", "spawn",
        ],
        Hypothesis::ErrorSwallow => &[
            "catch",
            "except",
            "err",
            "unwrap_or",
            "ok()",
            "let _",
            "ignore",
        ],
        Hypothesis::NPlusOneQuery => &[
            "for ", "while ", ".map(", "query", "select", "fetch", "request",
        ],
        Hypothesis::MissingValidation => &[
            "read",
            "parse",
            "request",
            "input",
            "len",
            "from_str",
            "deserialize",
        ],
        Hypothesis::DeadCode => &["pub ", "export", "fn ", "def "],
        Hypothesis::InconsistentPattern => &["fn ", "def ", "func ", "function"],
        Hypothesis::CrossLayerMismatch => &[
            "protocol", "request", "response", "api", "schema", "client", "server",
        ],
        Hypothesis::MissingTest => &["if ", "match ", "else", "case ", "test"],
        Hypothesis::MigrationGap => &["migration", "schema", "alter", "create table"],
        Hypothesis::ConfigDrift => &["config", "toml", "env", "setting", "option"],
        Hypothesis::UntrustedRepoInput => &[
            "path",
            "symlink",
            "canonicalize",
            "write",
            "create",
            "rename",
            "toml",
            "config",
        ],
        Hypothesis::MemoryDiskDrift => &[
            "cache",
            "persist",
            "restart",
            "migration",
            "incremental",
            "rebuild",
            "snapshot",
            "reload",
        ],
        Hypothesis::LifecycleCancelRetry => &[
            "cancel", "abort", "retry", "drop", "flag", "atomic", "kill", "guard",
        ],
        Hypothesis::TransitionCleanup => &[
            "reconnect",
            "pending",
            "generation",
            "limit",
            "cap",
            "budget",
            "resync",
            "stale",
        ],
        _ => &[],
    }
}

/// Identifier words of the diff that point at a hypothesis when choosing which to
/// hunt. Narrower than [`keywords`]: a word every change has would pick its
/// hypothesis for every run.
#[must_use]
pub fn signals(hypothesis: Hypothesis) -> &'static [&'static str] {
    match hypothesis {
        Hypothesis::UntrustedRepoInput => &[
            "symlink",
            "symlinks",
            "readlink",
            "canonicalize",
            "rename",
            "hardlink",
            "chmod",
            "write",
            "create",
            "copy",
            "toml",
            "gitattributes",
            "gitmodules",
        ],
        Hypothesis::LifecycleCancelRetry => &[
            "cancel",
            "cancelled",
            "canceled",
            "cancellation",
            "abort",
            "aborted",
            "retry",
            "retries",
            "drop",
            "flag",
            "flags",
            "atomic",
            "kill",
            "killed",
            "shutdown",
        ],
        Hypothesis::MemoryDiskDrift => &[
            "cache",
            "cached",
            "persist",
            "persisted",
            "persistence",
            "restart",
            "restarted",
            "migration",
            "migrations",
            "incremental",
            "rebuild",
            "reload",
            "stale",
        ],
        Hypothesis::TransitionCleanup => &[
            "reconnect",
            "reconnected",
            "reconnects",
            "pending",
            "generation",
            "generations",
            "limit",
            "limits",
            "cap",
            "caps",
            "capped",
            "resync",
        ],
        Hypothesis::RaceCondition => &[
            "lock",
            "locked",
            "mutex",
            "rwlock",
            "thread",
            "threads",
            "spawn",
            "spawned",
            "concurrent",
            "concurrently",
            "condvar",
        ],
        Hypothesis::AuthBypass => &[
            "auth",
            "authorize",
            "authorized",
            "authorization",
            "authenticate",
            "authenticated",
            "token",
            "tokens",
            "permission",
            "permissions",
            "credential",
            "credentials",
            "login",
            "password",
        ],
        Hypothesis::MigrationGap => &["migration", "migrations", "alter", "schema", "column"],
        Hypothesis::RetryNesting => &["backoff", "retry", "retries"],
        _ => &[],
    }
}

/// Added lines past this many are not read for signals.
pub const MAX_SCORED_LINES: usize = 2000;
/// Bytes of one added line read for signals.
const MAX_SCORED_LINE_BYTES: usize = 512;
/// A changed file whose path names a signal counts like this many lines.
const PATH_WEIGHT: u32 = 2;
/// Below this, a hypothesis is left to [`ORDER`].
const MIN_SCORE: u32 = 2;
/// Signals named in a choice's reason.
const REASON_SIGNALS: usize = 3;

/// How many reviewers [`select`] chooses for the same arguments.
#[must_use]
pub fn count(size: usize, explicit: &[Hypothesis]) -> usize {
    if explicit.is_empty() {
        return size.clamp(1, MAX_AGENTS);
    }
    let mut seen: Vec<Hypothesis> = Vec::new();
    for hypothesis in explicit {
        if !seen.contains(hypothesis) {
            seen.push(*hypothesis);
        }
    }
    seen.len().min(MAX_AGENTS)
}

/// `explicit` (deduplicated, at most eight) when given. Otherwise `size`
/// (clamped to `1..=8`) hypotheses: those whose [`signals`] the changed
/// paths and the first [`MAX_SCORED_LINES`] added lines name most, then
/// [`ORDER`] for the rest.
#[must_use]
pub fn select(size: usize, explicit: &[Hypothesis], files: &[DiffFile]) -> Vec<HypothesisChoice> {
    let choice = |hypothesis, reason: &str| HypothesisChoice {
        hypothesis,
        reason: reason.to_owned(),
    };
    if !explicit.is_empty() {
        let mut chosen: Vec<HypothesisChoice> = Vec::new();
        for hypothesis in explicit {
            if !chosen.iter().any(|c| c.hypothesis == *hypothesis) && chosen.len() < MAX_AGENTS {
                chosen.push(choice(*hypothesis, HypothesisChoice::REQUESTED));
            }
        }
        return chosen;
    }
    let size = count(size, &[]);
    let mut scored = score(files);
    scored.retain(|(_, score, _)| *score >= MIN_SCORE);
    scored.sort_by_key(|(hypothesis, score, _)| {
        (
            std::cmp::Reverse(*score),
            ORDER.iter().position(|h| h == hypothesis),
        )
    });
    let mut chosen: Vec<HypothesisChoice> = scored
        .into_iter()
        .take(size)
        .map(|(hypothesis, _, reason)| HypothesisChoice { hypothesis, reason })
        .collect();
    for hypothesis in ORDER {
        if chosen.len() >= size {
            break;
        }
        if !chosen.iter().any(|c| c.hypothesis == hypothesis) {
            chosen.push(choice(hypothesis, HypothesisChoice::DEFAULT_ORDER));
        }
    }
    chosen
}

/// Risks a spec could carry, scored like a diff's signals: each of the source's first
/// [`MAX_SCORED_LINES`] lines counts once, each path it touches [`PATH_WEIGHT`]. At most
/// `limit`, strongest first, none below the score [`select`] would pick.
#[must_use]
pub fn suggest(source: &str, paths: &[String], limit: usize) -> Vec<HypothesisChoice> {
    let lines = source
        .lines()
        .take(MAX_SCORED_LINES)
        .map(|line| (clip(line, MAX_SCORED_LINE_BYTES), 1));
    let paths = paths.iter().map(|path| (path.as_str(), PATH_WEIGHT));
    let mut scored = score_texts(lines.chain(paths), "spec");
    scored.retain(|(_, score, _)| *score >= MIN_SCORE);
    scored.sort_by_key(|(hypothesis, score, _)| {
        (
            std::cmp::Reverse(*score),
            ORDER.iter().position(|h| h == hypothesis),
        )
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(hypothesis, _, reason)| HypothesisChoice { hypothesis, reason })
        .collect()
}

/// Each hypothesis with signals and its score: one per added line naming one
/// of them, [`PATH_WEIGHT`] per path that does. The reason names its most
/// frequent signals.
fn score(files: &[DiffFile]) -> Vec<(Hypothesis, u32, String)> {
    let paths = files.iter().map(|file| (file.path.as_str(), PATH_WEIGHT));
    let added = files
        .iter()
        .flat_map(|file| {
            file.patch
                .lines()
                .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        })
        .take(MAX_SCORED_LINES)
        .map(|line| (clip(&line[1..], MAX_SCORED_LINE_BYTES), 1));
    score_texts(paths.chain(added), "diff")
}

fn score_texts<'a>(
    texts: impl Iterator<Item = (&'a str, u32)>,
    origin: &str,
) -> Vec<(Hypothesis, u32, String)> {
    let candidates: Vec<(Hypothesis, &[&str])> = ORDER
        .iter()
        .map(|&hypothesis| (hypothesis, signals(hypothesis)))
        .filter(|(_, words)| !words.is_empty())
        .collect();
    let mut hits: Vec<Vec<u32>> = candidates
        .iter()
        .map(|(_, words)| vec![0; words.len()])
        .collect();
    let mut scores = vec![0u32; candidates.len()];
    let mut tally = |text: &str, weight: u32| {
        let tokens: Vec<&str> = identifier_words(text).collect();
        for (at, (_, words)) in candidates.iter().enumerate() {
            let mut named = false;
            for (index, word) in words.iter().enumerate() {
                if tokens.iter().any(|token| token.eq_ignore_ascii_case(word)) {
                    hits[at][index] += 1;
                    named = true;
                }
            }
            if named {
                scores[at] += weight;
            }
        }
    };
    for (text, weight) in texts {
        tally(text, weight);
    }
    candidates
        .iter()
        .zip(hits)
        .zip(scores)
        .map(|(((hypothesis, words), hits), score)| {
            let mut named: Vec<(&str, u32)> = words
                .iter()
                .copied()
                .zip(hits)
                .filter(|(_, n)| *n > 0)
                .collect();
            named.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            let listed: Vec<String> = named
                .iter()
                .take(REASON_SIGNALS)
                .map(|(word, n)| format!("{word} ×{n}"))
                .collect();
            (
                *hypothesis,
                score,
                format!("{origin}: {}", listed.join(", ")),
            )
        })
        .collect()
}

/// `fetchPendingReads`, `pending_reads` and `pending-reads` all give `pending`
/// and `reads`: identifiers split on every non-alphanumeric and on a lower-to-upper case step.
fn identifier_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .flat_map(|part| {
            let mut words = Vec::new();
            let mut start = 0;
            let bytes = part.as_bytes();
            for at in 1..bytes.len() {
                if bytes[at].is_ascii_uppercase() && bytes[at - 1].is_ascii_lowercase() {
                    words.push(&part[start..at]);
                    start = at;
                }
            }
            words.push(&part[start..]);
            words
        })
        .filter(|word| !word.is_empty())
}

fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    pub hypothesis: Hypothesis,
    /// The diff files this agent starts from; every file when none matched.
    pub slice_paths: Vec<String>,
    pub extra_instructions: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewerPlan {
    pub agents: Vec<AgentSpec>,
}

/// One agent per hypothesis, each pointed at the files whose path or patch
/// names one of its keywords.
#[must_use]
pub fn plan(hypotheses: &[Hypothesis], files: &[DiffFile]) -> ReviewerPlan {
    let agents = hypotheses
        .iter()
        .map(|&hypothesis| {
            let words = keywords(hypothesis);
            let matching: Vec<String> = files
                .iter()
                .filter(|file| {
                    let path = file.path.to_ascii_lowercase();
                    let patch = file.patch.to_ascii_lowercase();
                    words
                        .iter()
                        .any(|word| path.contains(word) || patch.contains(word))
                })
                .map(|file| file.path.clone())
                .collect();
            let slice_paths = if matching.is_empty() {
                files.iter().map(|file| file.path.clone()).collect()
            } else {
                matching
            };
            AgentSpec {
                hypothesis,
                slice_paths,
                extra_instructions: instruction(hypothesis).to_owned(),
            }
        })
        .collect();
    ReviewerPlan { agents }
}

/// `slice-<hypothesis>.md`: the agent's hypothesis and the files to start from.
#[must_use]
pub fn slice_file(spec: &AgentSpec, nonce: &str) -> String {
    let mut text = format!(
        "# Hypothesis: {}\n\n{}\n\n<slice-{nonce}>\n",
        spec.hypothesis.name(),
        spec.extra_instructions
    );
    for path in &spec.slice_paths {
        let _ = writeln!(text, "- {}", review::redact(path).0.replace('<', "&lt;"));
    }
    let _ = writeln!(text, "</slice-{nonce}>");
    text
}

/// One reviewer's prompt: the shared pack, its own slice and the v2 answer.
#[must_use]
pub fn prompt(
    resolved: &review::Resolved,
    pack: &std::path::Path,
    slice: &std::path::Path,
    spec: &AgentSpec,
    nonce: &str,
) -> String {
    let base = review::prompt(resolved, pack, nonce);
    let v1 = format!("\"schema\":\"{}\"", review::SCHEMA);
    let v2 = format!("\"schema\":\"{}\"", review::SCHEMA_V2);
    let base = base
        .replace(&v1, &v2)
        .replace(
            "\"suggestion\":null}",
            &format!(
                "\"suggestion\":null,\"hypothesis\":\"{}\",\"kb_ref\":null}}",
                spec.hypothesis.name()
            ),
        )
        .replace(
            ",\"diagram\":null}",
            ",\"patterns\":[],\"coverage_notes\":[]}",
        );
    format!(
        "{base}\n\n## Your hypothesis: {name}\n\n{instruction}\n\nYou are one of several \
         reviewers, each hunting one hypothesis. Read {slice} first: it names the files to start \
         from inside <slice-{nonce}>, but follow the code wherever the hypothesis leads. Report \
         only findings that fit your hypothesis; an empty `findings` list is a valid answer. A \
         finding without `reachability` is dropped. Set each finding's `hypothesis` to \"{name}\". `patterns` may list up to ten short \
         conventions this project follows that you confirmed while reading. Do not emit \
         `diagrams` or `pr_confidence`.",
        name = spec.hypothesis.name(),
        instruction = spec.extra_instructions,
        slice = slice.display(),
    )
}

/// One agent's outcome as the consolidation reads it.
pub enum AgentAnswer {
    Validated(review::Validated),
    /// Timed out, failed, or rejected by validation: the reason.
    Uncovered(String),
}

#[derive(Debug, Default)]
pub struct Consolidated {
    pub findings: Vec<Finding>,
    pub notes: Vec<String>,
    /// Hypotheses with at least one kept finding.
    pub covered: Vec<Hypothesis>,
    pub unverified: u32,
    pub withheld: Vec<String>,
    pub summary: String,
}

/// The union of every agent's validated findings, deduplicated by
/// fingerprint (the more severe wins, then the one with a rule), capped at
/// `max_findings` once. `None` when no agent answered.
#[must_use]
pub fn consolidate(
    answers: Vec<(Hypothesis, AgentAnswer)>,
    max_findings: usize,
) -> Option<Consolidated> {
    let mut out = Consolidated::default();
    let mut answered = false;
    let mut summaries = Vec::new();
    for (hypothesis, answer) in answers {
        match answer {
            AgentAnswer::Uncovered(reason) => out.notes.push(format!(
                "hypothesis_uncovered: {}: {reason}",
                hypothesis.name()
            )),
            AgentAnswer::Validated(validated) => {
                answered = true;
                out.unverified += validated.unverified;
                out.withheld.extend(validated.withheld);
                if !validated.summary.trim().is_empty() {
                    summaries.push(format!(
                        "**{}** — {}",
                        hypothesis.name(),
                        validated.summary.trim()
                    ));
                }
                if validated.findings.is_empty() {
                    out.notes
                        .push(format!("hypothesis discarded: {}", hypothesis.name()));
                }
                for mut finding in validated.findings {
                    finding.hypothesis = Some(hypothesis);
                    match out
                        .findings
                        .iter_mut()
                        .find(|kept| kept.fingerprint == finding.fingerprint)
                    {
                        Some(kept) if wins(&finding, kept) => *kept = finding,
                        Some(_) => {}
                        None => out.findings.push(finding),
                    }
                }
            }
        }
    }
    if !answered {
        return None;
    }
    out.findings.sort_by_key(|finding| finding.severity);
    let cut = out.findings.split_off(max_findings.min(out.findings.len()));
    if !cut.is_empty() {
        out.notes.push(format!(
            "{} finding(s) past max_findings ({max_findings}) were left out",
            cut.len()
        ));
    }
    out.withheld
        .extend(cut.into_iter().map(|finding| finding.fingerprint));
    out.withheld.sort();
    out.withheld.dedup();
    for (id, finding) in out.findings.iter_mut().enumerate() {
        finding.id = u32::try_from(id + 1).unwrap_or(u32::MAX);
    }
    for finding in &out.findings {
        if let Some(hypothesis) = finding.hypothesis {
            if !out.covered.contains(&hypothesis) {
                out.covered.push(hypothesis);
            }
        }
    }
    out.summary = summaries.join("\n\n");
    Some(out)
}

fn wins(candidate: &Finding, kept: &Finding) -> bool {
    candidate.severity < kept.severity
        || (candidate.severity == kept.severity
            && candidate.rule_id.is_some()
            && kept.rule_id.is_none())
}

#[cfg(test)]
mod tests;
