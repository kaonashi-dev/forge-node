use std::collections::BTreeSet;

use domain::{Finding, ReviewSuppression, Severity, Verdict};
use sha2::{Digest, Sha256};

use crate::clip;
use crate::config::CATEGORIES;
use crate::output::{RawFinding, ReviewOutput};
use crate::pack::redact;
use crate::presets::Resolved;
use crate::rules::Rule;

pub const MAX_FINDINGS: usize = 200;
pub const MAX_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_SUGGESTION_BYTES: usize = 8 * 1024;

/// How far the model's line may sit from where its evidence actually is.
const ANCHOR_SLACK: u32 = 3;

/// A path in the reviewed diff. `text` is the new side, `None` when it could
/// not be read, which leaves its findings unverifiable and so dropped. A
/// finding on an `excluded` file is kept general and without evidence: the
/// file was left out of the pack because it may hold secrets.
#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
    pub text: Option<String>,
    pub excluded: bool,
}

#[derive(Debug)]
pub struct Validated {
    /// With the diagram appended, when the model drew one.
    pub summary: String,
    pub findings: Vec<Finding>,
    /// Claims whose line is not near their evidence, or outside every new-side
    /// hunk, demoted to general.
    pub invalid_anchors: u32,
    /// Dropped: a path outside the diff or escaping it, or evidence not in the file.
    pub unverified: u32,
    /// Dropped from a v2 answer: no `reachability`, or one with no trigger.
    pub without_reachability: u32,
    /// Findings considered, after the caps, the category and the confidence
    /// filters, before dedupe.
    pub total: u32,
    /// Dropped below `min_confidence`.
    pub low_confidence: u32,
    /// Cut by `max_findings` after dedupe.
    pub trimmed: u32,
    /// Dropped by the project's feedback suppressions, before `max_findings`.
    pub suppressed: u32,
    /// Fingerprints of findings the reviewer reported that were hidden: suppressed,
    /// below `min_confidence`, past `max_findings` or without reachability.
    pub withheld: Vec<String>,
}

/// New-side `(first, last)` line ranges of a unified patch; pure deletions have none.
#[must_use]
pub fn hunks(patch: &str) -> Vec<(u32, u32)> {
    patch
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("@@ -")?;
            let new = rest.split_whitespace().find(|part| part.starts_with('+'))?;
            let mut parts = new[1..].splitn(2, ',');
            let start: u32 = parts.next()?.parse().ok()?;
            let count: u32 = match parts.next() {
                Some(count) => count.parse().ok()?,
                None => 1,
            };
            (count > 0).then(|| (start, start + count - 1))
        })
        .collect()
}

/// Anchor, cap, dedupe and fingerprint the model's findings against the diff.
/// A `rule_id` survives only when it names one of the in-scope `rules`, whose
/// severity then caps the finding's; a finding in a disabled known category
/// without one, or below `min_confidence`, is dropped.
///
/// # Errors
/// More than half of the anchors fell outside the diff.
pub fn validate(
    output: ReviewOutput,
    files: &[ChangedFile],
    rules: &[Rule],
    resolved: &Resolved,
    suppressions: &[ReviewSuppression],
) -> Result<Validated, String> {
    let mut findings = output.findings;
    findings.truncate(MAX_FINDINGS);
    let findings: Vec<RawFinding> = findings
        .into_iter()
        .map(|mut raw| {
            if !CATEGORIES.contains(&raw.category.as_str()) {
                raw.category = "logic".to_owned();
            }
            let rule = raw
                .rule_id
                .as_deref()
                .and_then(|id| rules.iter().find(|rule| rule.id == id))
                .filter(|rule| rule.matches(&raw.path));
            match rule {
                Some(rule) => raw.severity = raw.severity.max(rule.severity),
                None => raw.rule_id = None,
            }
            capped(raw)
        })
        .collect();
    let findings: Vec<RawFinding> = findings
        .into_iter()
        .filter(|raw| {
            raw.rule_id.is_some() || resolved.categories.get(&raw.category) != Some(false)
        })
        .collect();
    let (findings, low): (Vec<RawFinding>, Vec<RawFinding>) = findings
        .into_iter()
        .partition(|raw| raw.confidence >= resolved.min_confidence);
    let low_confidence = u32::try_from(low.len()).unwrap_or(u32::MAX);
    let (findings, unreachable): (Vec<RawFinding>, Vec<RawFinding>) =
        if output.schema == domain::SchemaVersion::V2 {
            findings.into_iter().partition(|raw| {
                raw.reachability
                    .as_ref()
                    .is_some_and(|reach| !reach.trigger.trim().is_empty())
            })
        } else {
            (findings, Vec::new())
        };
    let without_reachability = u32::try_from(unreachable.len()).unwrap_or(u32::MAX);
    let mut withheld: BTreeSet<String> = low
        .iter()
        .chain(&unreachable)
        .map(|raw| {
            fingerprint(
                &raw.path,
                &raw.category,
                raw.rule_id.as_deref(),
                &redact(&raw.evidence).0,
            )
        })
        .collect();
    let total = u32::try_from(findings.len()).unwrap_or(u32::MAX);
    let mut invalid_anchors = 0u32;
    let mut anchor_attempts = 0u32;
    let mut unverified = 0u32;
    let mut kept: Vec<Finding> = Vec::new();
    let prepared: Vec<Prepared<'_>> = files
        .iter()
        .map(|file| {
            let text = file.text.as_deref().unwrap_or_default();
            Prepared {
                path: &file.path,
                excluded: file.excluded,
                hunks: hunks(&file.patch),
                line_count: text.lines().count(),
                nonblank: text
                    .lines()
                    .zip(1u32..)
                    .filter(|(line, _)| !line.trim().is_empty())
                    .map(|(line, number)| (number, collapsed(line)))
                    .collect(),
            }
        })
        .collect();

    for raw in findings {
        let file = (!raw.path.contains("..") && !raw.path.starts_with('/'))
            .then(|| prepared.iter().find(|file| file.path == raw.path))
            .flatten();
        let excluded = file.is_some_and(|file| file.excluded);
        let occurrences = file
            .filter(|_| !excluded)
            .map(|file| evidence_lines(&file.nonblank, &raw.evidence))
            .unwrap_or_default();
        let file = match file {
            Some(file) if excluded || !occurrences.is_empty() => file,
            _ => {
                unverified += 1;
                continue;
            }
        };
        let mut line = None;
        let mut end_line = None;
        if let Some(asked) = raw.line.filter(|_| !excluded) {
            anchor_attempts += 1;
            let anchored = occurrences
                .iter()
                .copied()
                .filter(|found| found.abs_diff(asked) <= ANCHOR_SLACK)
                .min_by_key(|found| found.abs_diff(asked));
            let in_hunk = anchored.is_some_and(|anchored| {
                file.hunks
                    .iter()
                    .any(|(first, last)| (*first..=*last).contains(&anchored))
            });
            match anchored {
                Some(anchored) if in_hunk => {
                    line = Some(anchored);
                    end_line = raw
                        .end_line
                        .and_then(|end| {
                            let shifted = i64::from(end) + i64::from(anchored) - i64::from(asked);
                            u32::try_from(shifted).ok()
                        })
                        .filter(|end| {
                            *end >= anchored
                                && usize::try_from(*end).is_ok_and(|end| end <= file.line_count)
                        });
                }
                _ => invalid_anchors += 1,
            }
        }

        let raw = redacted(raw);
        let finding = Finding {
            id: 0,
            fingerprint: fingerprint(
                &raw.path,
                &raw.category,
                raw.rule_id.as_deref(),
                &raw.evidence,
            ),
            path: raw.path,
            line,
            end_line,
            severity: raw.severity,
            category: raw.category,
            rule_id: raw.rule_id,
            confidence: raw.confidence,
            title: raw.title,
            body: raw.body,
            evidence: if excluded {
                String::new()
            } else {
                raw.evidence
            },
            suggestion: raw.suggestion,
            lineage: domain::Lineage::New,
            reaction: None,
            hypothesis: None,
            kb_ref: raw.kb_ref,
            reachability: raw.reachability,
        };
        match kept.iter_mut().find(|other| overlaps(other, &finding)) {
            Some(other) if finding.severity < other.severity => *other = finding,
            Some(_) => {}
            None => kept.push(finding),
        }
    }

    if (invalid_anchors + unverified) * 2 > anchor_attempts + unverified {
        return Err(format!(
            "{invalid_anchors} of {anchor_attempts} anchors were outside the diff or away from \
             their evidence and {unverified} could not be verified: the answer reviewed \
             something other than this change"
        ));
    }
    let (hidden, mut kept): (Vec<Finding>, Vec<Finding>) = kept
        .into_iter()
        .partition(|finding| crate::suppress::is_suppressed(finding, suppressions));
    let suppressed = u32::try_from(hidden.len()).unwrap_or(u32::MAX);
    kept.sort_by_key(|finding| finding.severity);
    let cut = kept.split_off(resolved.max_findings.min(kept.len()));
    let trimmed = u32::try_from(cut.len()).unwrap_or(u32::MAX);
    withheld.extend(
        hidden
            .into_iter()
            .chain(cut)
            .map(|finding| finding.fingerprint),
    );
    for (id, finding) in kept.iter_mut().enumerate() {
        finding.id = u32::try_from(id + 1).unwrap_or(u32::MAX);
    }
    let summary = match output.diagram {
        Some(diagram) if diagram.trim_start().starts_with("```") => {
            format!("{}\n\n{diagram}", output.summary)
        }
        Some(diagram) => format!("{}\n\n```mermaid\n{diagram}\n```", output.summary),
        None => output.summary,
    };
    let summary = redact(&summary).0;
    Ok(Validated {
        summary,
        findings: kept,
        invalid_anchors,
        unverified,
        without_reachability,
        total,
        low_confidence,
        trimmed,
        suppressed,
        withheld: withheld.into_iter().collect(),
    })
}

fn redacted(mut raw: RawFinding) -> RawFinding {
    for text in [&mut raw.title, &mut raw.body, &mut raw.evidence] {
        *text = redact(text).0;
    }
    for text in [
        raw.suggestion.as_mut(),
        raw.kb_ref.as_mut(),
        raw.reachability.as_mut().map(|reach| &mut reach.trigger),
    ]
    .into_iter()
    .flatten()
    {
        *text = redact(text).0;
    }
    raw
}

fn capped(mut raw: RawFinding) -> RawFinding {
    for (text, max) in [
        (&mut raw.title, MAX_TEXT_BYTES),
        (&mut raw.body, MAX_TEXT_BYTES),
        (&mut raw.evidence, MAX_TEXT_BYTES),
    ] {
        let end = clip(text, max).len();
        text.truncate(end);
    }
    if let Some(suggestion) = raw.suggestion.as_mut() {
        let end = clip(suggestion, MAX_SUGGESTION_BYTES).len();
        suggestion.truncate(end);
    }
    raw
}

/// What validation needs from one changed file, computed once for all findings.
struct Prepared<'a> {
    path: &'a str,
    excluded: bool,
    hunks: Vec<(u32, u32)>,
    line_count: usize,
    /// `(1-based line, whitespace-collapsed text)` of every non-blank line.
    nonblank: Vec<(u32, String)>,
}

/// Bounds the comparison of an evidence pasted at length.
const MAX_EVIDENCE_LINES: usize = 40;

fn collapsed(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 1-based lines where `evidence` starts, compared over non-blank lines with
/// whitespace collapsed: the model rarely keeps indentation or blank lines.
fn evidence_lines(nonblank: &[(u32, String)], evidence: &str) -> Vec<u32> {
    let wanted: Vec<String> = evidence
        .lines()
        .map(collapsed)
        .filter(|line| !line.is_empty())
        .take(MAX_EVIDENCE_LINES)
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    nonblank
        .windows(wanted.len())
        .filter(|window| {
            window
                .iter()
                .zip(&wanted)
                .all(|((_, line), want)| line.contains(want.as_str()))
        })
        .map(|window| window[0].0)
        .collect()
}

fn overlaps(a: &Finding, b: &Finding) -> bool {
    if a.path != b.path || a.category != b.category {
        return false;
    }
    let (Some(a_start), Some(b_start)) = (a.line, b.line) else {
        return b.line.is_none() && a.fingerprint == b.fingerprint;
    };
    let a_end = a.end_line.unwrap_or(a_start);
    let b_end = b.end_line.unwrap_or(b_start);
    a_start <= b_end && b_start <= a_end
}

/// First 16 hex chars of SHA-256 over path, category, rule and whitespace-collapsed evidence.
#[must_use]
pub fn fingerprint(path: &str, category: &str, rule_id: Option<&str>, evidence: &str) -> String {
    let evidence = evidence.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut hasher = Sha256::new();
    for part in [path, category, rule_id.unwrap_or(""), &evidence] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `5 − 2.5·P0 − P1 − min(1, 0.25·P2)`, clamped to `0..=5`, to the nearest 0.5.
#[must_use]
pub fn score(findings: &[Finding]) -> f32 {
    let count = |severity| {
        f64::from(
            u32::try_from(findings.iter().filter(|f| f.severity == severity).count())
                .unwrap_or(u32::MAX),
        )
    };
    let raw = 5.0
        - 2.5 * count(Severity::P0)
        - count(Severity::P1)
        - (0.25 * count(Severity::P2)).min(1.0);
    // A tie rounds down: the score never rounds itself past a finding.
    let rounded = ((raw.clamp(0.0, 5.0) * 2.0 - 0.5).ceil() / 2.0).max(0.0);
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a multiple of 0.5 in 0..=5 is exact in f32"
    )]
    let rounded = rounded as f32;
    rounded
}

#[must_use]
pub fn verdict(findings: &[Finding], score: f32) -> Verdict {
    if findings.iter().any(|f| f.severity == Severity::P0) {
        Verdict::Block
    } else if findings.iter().any(|f| f.severity == Severity::P1) || score < 4.0 {
        Verdict::Review
    } else {
        Verdict::Ok
    }
}

/// Whether `[review.auto_approve]` would let a run with these standing
/// findings through. P0 always blocks, and so does a P1 the run could not
/// fully check.
#[must_use]
pub fn auto_approvable(
    settings: &crate::config::AutoApprove,
    findings: &[Finding],
    score: f32,
    coverage: &domain::Coverage,
) -> bool {
    use crate::config::RiskCeiling;
    let worst = findings.iter().map(|finding| finding.severity).min();
    let within = match (settings.risk_ceiling, worst) {
        (_, None) => true,
        (_, Some(Severity::P0)) | (RiskCeiling::Low, Some(_)) => false,
        (RiskCeiling::Medium, Some(severity)) => severity >= Severity::P2,
        (RiskCeiling::High | RiskCeiling::Critical, Some(_)) => true,
    };
    let unchecked_p1 = coverage.unverified > 0 && worst == Some(Severity::P1);
    settings.enabled
        && score >= settings.min_score
        && within
        && !unchecked_p1
        && (!settings.require_clean_diff || coverage.truncated.is_empty())
}
