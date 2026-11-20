//! The Review mode's helpers that have a right and a wrong answer, apart from
//! the view so a test reaches them without a window.

use domain::{Finding, FindingReaction, ReviewRun, ReviewStage, ReviewTarget, Severity};
use rune_kit::diff::{Annotation, LineRef, Side};

pub use domain::review::findings_prompt;

use super::{Grouping, PROMPT_BUDGET};

const SEVERITIES: [Severity; 3] = [Severity::P0, Severity::P1, Severity::P2];
pub const STAGES: [ReviewStage; 4] = [
    ReviewStage::Diff,
    ReviewStage::Pack,
    ReviewStage::Model,
    ReviewStage::Validate,
];

/// Findings grouped, as indices into `findings`: severity groups are most
/// severe first; file groups are sorted by path and line. Empty groups drop.
#[must_use]
pub fn group_findings(findings: &[Finding], by: Grouping) -> Vec<(String, Vec<usize>)> {
    match by {
        Grouping::Severity => SEVERITIES
            .iter()
            .map(|severity| {
                let mut group: Vec<usize> = (0..findings.len())
                    .filter(|at| findings[*at].severity == *severity)
                    .collect();
                group.sort_by(|a, b| {
                    let (a, b) = (&findings[*a], &findings[*b]);
                    a.path
                        .cmp(&b.path)
                        .then_with(|| a.line.unwrap_or(0).cmp(&b.line.unwrap_or(0)))
                });
                (format!("{severity:?}"), group)
            })
            .filter(|(_, group)| !group.is_empty())
            .collect(),
        Grouping::File => {
            let mut paths: Vec<&str> = findings
                .iter()
                .map(|finding| finding.path.as_str())
                .collect();
            paths.sort_unstable();
            paths.dedup();
            paths
                .into_iter()
                .map(|path| {
                    let mut group: Vec<usize> = (0..findings.len())
                        .filter(|at| findings[*at].path == path)
                        .collect();
                    group.sort_by_key(|at| findings[*at].line.unwrap_or(0));
                    (path.to_string(), group)
                })
                .collect()
        }
    }
}

/// An unmeasured head is not evidence the checkout moved, and a pull
/// request's head is not the checkout's.
#[must_use]
pub fn is_stale(run: &ReviewRun, head: Option<&str>) -> bool {
    matches!(run.target, ReviewTarget::Workspace { .. })
        && head.is_some_and(|head| head != run.head_oid)
}

/// One annotation per anchored, standing finding, on the new side of the file
/// at its index in `file_paths`.
#[must_use]
pub fn annotations(findings: &[Finding], file_paths: &[&str]) -> Vec<Annotation> {
    findings
        .iter()
        .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
        .filter_map(|finding| {
            let line = finding.line?;
            let file = file_paths.iter().position(|path| *path == finding.path)?;
            Some(Annotation::new(
                file,
                LineRef::new(Side::New, line),
                finding.id.to_string(),
            ))
        })
        .collect()
}

/// Every finding that is not dismissed, as one prompt within [`PROMPT_BUDGET`].
#[must_use]
pub fn standing_prompt(findings: &[Finding]) -> (String, usize) {
    let standing: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
        .collect();
    findings_prompt(&standing, PROMPT_BUDGET)
}

fn progress(run: &ReviewRun) -> usize {
    if run.finished_at.is_some() {
        return STAGES.len() + 1;
    }
    run.stage.map_or(0, |stage| {
        STAGES
            .iter()
            .position(|known| *known == stage)
            .map_or(1, |at| at + 1)
    })
}

/// Whether `next` replaces `current`: a newer run wins, and an earlier read of
/// the same run never undoes a later one.
#[must_use]
pub fn supersedes(current: Option<&ReviewRun>, next: &ReviewRun) -> bool {
    let Some(current) = current else {
        return true;
    };
    if current.id != next.id {
        return next.id > current.id;
    }
    progress(next) >= progress(current)
}

#[cfg(test)]
mod kb_tests {
    use super::*;

    #[test]
    fn cited_sections_are_listed_once_and_read_by_slug() {
        let mut a = crate::code_review::tests::finding(1, "a.rs", None, Severity::P1);
        a.kb_ref = Some("architecture.md#crates-daemon".into());
        let b = Finding { id: 2, ..a.clone() };
        assert_eq!(kb_refs(&[a, b]), ["architecture.md#crates-daemon"]);
        let files = vec![domain::KbFile {
            file: "architecture.md".into(),
            content: "# Architecture\n\n## crates/daemon\n\n- 2 file(s)\n\n## web\n\n- 1 file(s)\n"
                .into(),
            source: "daemon".into(),
            updated_at: None,
        }];
        assert_eq!(
            kb_excerpt(&files, "architecture.md#crates-daemon").as_deref(),
            Some("- 2 file(s)")
        );
        assert_eq!(kb_excerpt(&files, "architecture.md#gone"), None);
        assert!(
            kb_excerpt(&files, "architecture.md")
                .is_some_and(|text| text.starts_with("# Architecture"))
        );
        assert_eq!(kb_excerpt(&files, "patterns.json#x"), None);
    }

    #[test]
    fn a_tail_keeps_the_end_on_a_char_boundary() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
        assert_eq!(tail("aé", 1), "");
    }
}

#[cfg(test)]
mod tests {
    use domain::ReviewId;

    use super::super::tests::{finding, run};
    use super::*;

    fn ids(findings: &[Finding], group: &[usize]) -> Vec<u32> {
        group.iter().map(|at| findings[*at].id).collect()
    }

    #[test]
    fn findings_group_by_severity_then_path_and_line() {
        let findings = vec![
            finding(1, "b.rs", Some(9), Severity::P2),
            finding(2, "a.rs", Some(4), Severity::P1),
            finding(3, "b.rs", Some(2), Severity::P0),
            finding(4, "a.rs", None, Severity::P2),
        ];
        let groups = group_findings(&findings, Grouping::Severity);
        let keys: Vec<&str> = groups.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["P0", "P1", "P2"]);
        assert_eq!(
            ids(&findings, &groups[2].1),
            vec![4, 1],
            "path, then a whole-file finding first"
        );
        assert_eq!(
            group_findings(&findings[..1], Grouping::Severity)
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            ["P2"]
        );
    }

    #[test]
    fn findings_group_by_file_with_general_findings_first() {
        let findings = vec![
            finding(1, "b.rs", Some(9), Severity::P2),
            finding(2, "a.rs", Some(4), Severity::P1),
            finding(3, "b.rs", Some(2), Severity::P0),
            finding(4, "a.rs", None, Severity::P2),
        ];
        let groups = group_findings(&findings, Grouping::File);
        let keys: Vec<&str> = groups.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["a.rs", "b.rs"]);
        assert_eq!(ids(&findings, &groups[0].1), vec![4, 2]);
        assert_eq!(ids(&findings, &groups[1].1), vec![3, 1]);
    }

    #[test]
    fn a_run_is_stale_only_when_a_known_head_moved() {
        let run = run(ReviewId::new(), None, false);
        assert!(!is_stale(&run, Some(run.head_oid.as_str())));
        assert!(is_stale(&run, Some("different")));
        assert!(!is_stale(&run, None), "unmeasured is not evidence");
    }

    #[test]
    fn a_newer_run_or_a_later_stage_replaces_the_one_on_screen() {
        let first = ReviewId::new();
        let second = ReviewId::new();
        assert!(supersedes(None, &run(first, None, false)));
        assert!(supersedes(
            Some(&run(first, None, false)),
            &run(second, None, false)
        ));
        assert!(!supersedes(
            Some(&run(second, None, false)),
            &run(first, None, false)
        ));
        let done = run(first, Some(ReviewStage::Validate), true);
        assert!(!supersedes(Some(&done), &run(first, None, false)));
        assert!(supersedes(
            Some(&run(first, Some(ReviewStage::Pack), false)),
            &run(first, Some(ReviewStage::Model), false)
        ));
    }

    #[test]
    fn annotations_index_new_side_lines_of_standing_anchored_findings() {
        let mut dismissed = finding(3, "b.rs", Some(7), Severity::P1);
        dismissed.reaction = Some(FindingReaction::Dismiss);
        let findings = vec![
            finding(1, "b.rs", Some(4), Severity::P1),
            finding(2, "a.rs", None, Severity::P2),
            dismissed,
            finding(4, "elsewhere.rs", Some(1), Severity::P0),
        ];
        let built = annotations(&findings, &["a.rs", "b.rs"]);
        assert_eq!(
            built,
            vec![Annotation::new(1, LineRef::new(Side::New, 4), "1")]
        );
    }

    #[test]
    fn a_findings_prompt_fits_its_budget_on_whole_findings() {
        let mut big = finding(1, "a.rs", Some(2), Severity::P0);
        big.body = "x".repeat(PROMPT_BUDGET);
        let small = finding(2, "a.rs", Some(4), Severity::P1);
        let (prompt, left_out) = findings_prompt(&[&big, &small], PROMPT_BUDGET);
        assert!(prompt.len() <= PROMPT_BUDGET);
        assert_eq!(left_out, 1);
        assert!(prompt.contains("a.rs:4 — finding 2"), "{prompt}");
        assert!(!prompt.contains("finding 1"));

        let many: Vec<Finding> = (0..200)
            .map(|id| {
                let mut f = finding(id, "a.rs", Some(id), Severity::P2);
                f.body = "y".repeat(100);
                f
            })
            .collect();
        let refs: Vec<&Finding> = many.iter().collect();
        let (prompt, left_out) = findings_prompt(&refs, PROMPT_BUDGET);
        assert!(prompt.len() <= PROMPT_BUDGET);
        assert!(left_out > 0 && left_out < 200);

        assert_eq!(findings_prompt(&[&big], PROMPT_BUDGET), (String::new(), 1));
    }
}

/// The knowledge-base sections the findings cite, each once, in order.
#[must_use]
pub fn kb_refs(findings: &[Finding]) -> Vec<String> {
    let mut cited: Vec<String> = Vec::new();
    for reference in findings
        .iter()
        .filter_map(|finding| finding.kb_ref.as_ref())
    {
        if !cited.contains(reference) {
            cited.push(reference.clone());
        }
    }
    cited
}

/// The text a `file#section` reference names: the `## ` heading whose slug
/// is the section, or the whole file without one.
#[must_use]
pub fn kb_excerpt(files: &[domain::KbFile], reference: &str) -> Option<String> {
    let (name, section) = reference.split_once('#').unwrap_or((reference, ""));
    let file = files.iter().find(|file| file.file == name)?;
    if section.is_empty() {
        return Some(file.content.clone());
    }
    let slug = |heading: &str| {
        heading
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_owned()
    };
    file.content.split("\n## ").skip(1).find_map(|part| {
        let (heading, body) = part.split_once('\n').unwrap_or((part, ""));
        (slug(heading.trim()) == section).then(|| body.trim().to_owned())
    })
}

/// The last `max` bytes of `text`, on a char boundary.
#[must_use]
pub fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}
