//! A review swarm: which hypotheses its agents hunt, which files each one is
//! pointed at, and how their answers become one set of findings. Pure; the
//! daemon launches the agents and reads their answers.

use std::fmt::Write as _;

use domain::{DiffFile, Finding, Hypothesis};

pub const MAX_AGENTS: usize = 8;

/// The order agents take hypotheses in: the default three first.
pub const ORDER: [Hypothesis; 15] = [
    Hypothesis::AuthBypass,
    Hypothesis::BrokenContract,
    Hypothesis::ErrorSwallow,
    Hypothesis::RaceCondition,
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
        _ => &[],
    }
}

/// `explicit` (deduplicated, at most eight) when given, else the first
/// `size` of [`ORDER`], `size` clamped to `1..=8`.
#[must_use]
pub fn select(size: usize, explicit: &[Hypothesis]) -> Vec<Hypothesis> {
    if !explicit.is_empty() {
        let mut chosen = Vec::new();
        for hypothesis in explicit {
            if !chosen.contains(hypothesis) && chosen.len() < MAX_AGENTS {
                chosen.push(*hypothesis);
            }
        }
        return chosen;
    }
    ORDER[..size.clamp(1, MAX_AGENTS)].to_vec()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    pub hypothesis: Hypothesis,
    /// The diff files this agent starts from; every file when none matched.
    pub slice_paths: Vec<String>,
    pub extra_instructions: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SwarmPlan {
    pub agents: Vec<AgentSpec>,
}

/// One agent per hypothesis, each pointed at the files whose path or patch
/// names one of its keywords.
#[must_use]
pub fn plan(hypotheses: &[Hypothesis], files: &[DiffFile]) -> SwarmPlan {
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
    SwarmPlan { agents }
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

/// The swarm agent's prompt: the shared pack, its own slice and the v2 answer.
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
         only findings that fit your hypothesis; an empty `findings` list is a valid answer. Set \
         each finding's `hypothesis` to \"{name}\". `patterns` may list up to ten short \
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
