//! The Review mode's helpers that have a right and a wrong answer, apart from
//! the view so a test reaches them without a window.

use domain::{Finding, FindingReaction, ReviewRun, ReviewStage, ReviewTarget, Severity};
use rune_kit::diff::{Annotation, LineRef, Side};

use super::{Grouping, PROMPT_BUDGET};

const PROMPT_HEAD: &str = "Forge Review found these issues in your changes. Fix each one, or say \
                           why it is not a problem:\n\n";

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

/// Whole findings, in order, within `budget` bytes, and how many did not fit.
/// Empty when none fits or none is given; `empty_prompt_reason` tells which.
#[must_use]
pub fn findings_prompt(findings: &[&Finding], budget: usize) -> (String, usize) {
    let mut prompt = PROMPT_HEAD.to_string();
    let mut left_out = 0;
    for finding in findings {
        let mut block = format!(
            "- [{:?}] {}{} — {}\n  {}\n",
            finding.severity,
            finding.path,
            finding
                .line
                .map(|line| format!(":{line}"))
                .unwrap_or_default(),
            finding.title,
            finding.body.replace('\n', "\n  "),
        );
        if let Some(suggestion) = &finding.suggestion {
            block.push_str(&format!(
                "  Suggestion:\n  {}\n",
                suggestion.replace('\n', "\n  ")
            ));
        }
        block.push('\n');
        if prompt.len() + block.len() <= budget {
            prompt.push_str(&block);
        } else {
            left_out += 1;
        }
    }
    if left_out == findings.len() {
        return (String::new(), left_out);
    }
    (prompt, left_out)
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
