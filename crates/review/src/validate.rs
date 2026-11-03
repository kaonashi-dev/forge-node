use domain::{Finding, Severity, Verdict};
use sha2::{Digest, Sha256};

use crate::clip;
use crate::output::{RawFinding, ReviewOutput};
use crate::presets::Resolved;
use crate::rules::Rule;

pub const MAX_FINDINGS: usize = 200;
pub const MAX_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_SUGGESTION_BYTES: usize = 8 * 1024;

/// How far the model's line may sit from where its evidence actually is.
const ANCHOR_SLACK: u32 = 3;

/// A path in the reviewed diff. `text` is the new side, `None` when it could
/// not be read, which leaves its findings unverifiable and so dropped.
#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: String,
    pub patch: String,
    pub text: Option<String>,
}

#[derive(Debug)]
pub struct Validated {
    /// With the diagram appended, when the model drew one.
    pub summary: String,
    pub findings: Vec<Finding>,
    /// Lines outside every new-side hunk, demoted to general.
    pub invalid_anchors: u32,
    /// Findings considered, after the caps, the category and the confidence
    /// filters, before dedupe.
    pub total: u32,
    /// Dropped below `min_confidence`.
    pub low_confidence: u32,
    /// Cut by `max_findings` after dedupe.
    pub trimmed: u32,
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
) -> Result<Validated, String> {
    let mut findings = output.findings;
    findings.truncate(MAX_FINDINGS);
    let findings: Vec<RawFinding> = findings
        .into_iter()
        .map(|mut raw| {
            let rule = raw
                .rule_id
                .as_deref()
                .and_then(|id| rules.iter().find(|rule| rule.id == id));
            match rule {
                Some(rule) => raw.severity = raw.severity.max(rule.severity),
                None => raw.rule_id = None,
            }
            raw
        })
        .filter(|raw| {
            raw.rule_id.is_some() || resolved.categories.get(&raw.category) != Some(false)
        })
        .collect();
    let (findings, low): (Vec<RawFinding>, Vec<RawFinding>) = findings
        .into_iter()
        .partition(|raw| raw.confidence >= resolved.min_confidence);
    let low_confidence = u32::try_from(low.len()).unwrap_or(u32::MAX);
    let total = u32::try_from(findings.len()).unwrap_or(u32::MAX);
    let mut invalid_anchors = 0u32;
    let mut kept: Vec<Finding> = Vec::new();

    for raw in findings {
        let raw = capped(raw);
        if raw.path.contains("..") || raw.path.starts_with('/') {
            continue;
        }
        let Some(file) = files.iter().find(|file| file.path == raw.path) else {
            continue;
        };
        let Some(text) = file.text.as_deref() else {
            continue;
        };
        let occurrences = evidence_lines(text, &raw.evidence);
        if occurrences.is_empty() {
            continue;
        }
        let mut line = None;
        let mut end_line = None;
        if let Some(asked) = raw.line {
            let anchored = occurrences
                .iter()
                .copied()
                .filter(|found| found.abs_diff(asked) <= ANCHOR_SLACK)
                .min_by_key(|found| found.abs_diff(asked))
                .unwrap_or(asked);
            if hunks(&file.patch)
                .iter()
                .any(|(first, last)| (*first..=*last).contains(&anchored))
            {
                line = Some(anchored);
                end_line = raw
                    .end_line
                    .and_then(|end| {
                        let shifted = i64::from(end) + i64::from(anchored) - i64::from(asked);
                        u32::try_from(shifted).ok()
                    })
                    .filter(|end| *end >= anchored && *end <= last_line(text));
            } else {
                invalid_anchors += 1;
            }
        }

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
            evidence: raw.evidence,
            suggestion: raw.suggestion,
        };
        match kept.iter_mut().find(|other| overlaps(other, &finding)) {
            Some(other) if finding.severity < other.severity => *other = finding,
            Some(_) => {}
            None => kept.push(finding),
        }
    }

    if invalid_anchors * 2 > total {
        return Err(format!(
            "{invalid_anchors} of {total} findings were anchored outside the diff"
        ));
    }
    kept.sort_by_key(|finding| finding.severity);
    let trimmed =
        u32::try_from(kept.len().saturating_sub(resolved.max_findings)).unwrap_or(u32::MAX);
    kept.truncate(resolved.max_findings);
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
    Ok(Validated {
        summary,
        findings: kept,
        invalid_anchors,
        total,
        low_confidence,
        trimmed,
    })
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

/// 1-based lines where `evidence` starts, compared line by line with
/// surrounding whitespace ignored: the model rarely keeps indentation.
fn evidence_lines(text: &str, evidence: &str) -> Vec<u32> {
    let wanted: Vec<&str> = evidence
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    (0..lines.len())
        .filter(|start| {
            wanted.iter().enumerate().all(|(offset, want)| {
                lines
                    .get(start + offset)
                    .is_some_and(|line| line.contains(want))
            })
        })
        .filter_map(|start| u32::try_from(start + 1).ok())
        .collect()
}

fn overlaps(a: &Finding, b: &Finding) -> bool {
    if a.path != b.path || a.category != b.category {
        return false;
    }
    let (Some(a_start), Some(b_start)) = (a.line, b.line) else {
        return false;
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

fn last_line(text: &str) -> u32 {
    u32::try_from(text.lines().count()).unwrap_or(u32::MAX)
}
