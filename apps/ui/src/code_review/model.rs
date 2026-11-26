//! The Review mode's helpers that have a right and a wrong answer, apart from
//! the view so a test reaches them without a window.

use domain::{Finding, FindingReaction, Lineage, ReviewRun, ReviewStage, ReviewTarget, Severity};
use rune_kit::diff::{Annotation, LineRef, Side};

pub use domain::review::findings_prompt;

use super::{Grouping, PROMPT_BUDGET};

/// Bytes of a run's summary a chat's first prompt carries; the findings come first.
const CHAT_SUMMARY_BYTES: usize = 2048;
/// Bytes of a typed question that travel, so a pasted log cannot crowd out the findings.
pub const QUESTION_BYTES: usize = 2048;

/// Which findings the Review tab's list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FindingFilter {
    #[default]
    Open,
    New,
    Dismissed,
}

impl FindingFilter {
    #[must_use]
    pub fn keeps(self, finding: &Finding) -> bool {
        let dismissed = finding.reaction == Some(FindingReaction::Dismiss);
        match self {
            Self::Open => !dismissed,
            Self::New => !dismissed && finding.lineage == Lineage::New,
            Self::Dismissed => dismissed,
        }
    }
}

/// A severity group's heading says what it means for the merge, not only its level.
#[must_use]
pub fn group_title(grouping: Grouping, key: &str) -> String {
    match (grouping, key) {
        (Grouping::Severity, "P0" | "P1") => format!("Blocks merge · {key}"),
        (Grouping::Severity, "P2") => format!("Worth fixing · {key}"),
        _ => key.to_owned(),
    }
}

/// The ids the list shows, in its order: `groups` as indices into `findings`.
#[must_use]
pub fn listed(
    findings: &[Finding],
    groups: &[(String, Vec<usize>)],
    filter: FindingFilter,
) -> Vec<u32> {
    groups
        .iter()
        .flat_map(|(_, group)| group.iter())
        .map(|at| &findings[*at])
        .filter(|finding| filter.keeps(finding))
        .map(|finding| finding.id)
        .collect()
}

/// The finding the detail pane shows: the chosen one while it is listed, else the first.
#[must_use]
pub fn shown_finding(order: &[u32], chosen: Option<u32>) -> Option<u32> {
    chosen
        .filter(|id| order.contains(id))
        .or_else(|| order.first().copied())
}

/// The neighbour `j`/`k` moves to; stays put at either end.
#[must_use]
pub fn step(order: &[u32], current: Option<u32>, forward: bool) -> Option<u32> {
    let Some(at) = current.and_then(|id| order.iter().position(|known| *known == id)) else {
        return order.first().copied();
    };
    let next = if forward {
        (at + 1).min(order.len().saturating_sub(1))
    } else {
        at.saturating_sub(1)
    };
    order.get(next).copied()
}

/// `text`'s first `max` bytes, on a char boundary.
#[must_use]
pub fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn place(finding: &Finding) -> String {
    match finding.line {
        Some(line) => format!("{}:{line}", finding.path),
        None => finding.path.clone(),
    }
}

/// A question about one finding names it the way the chat's first prompt listed it.
#[must_use]
pub fn about(finding: &Finding, question: &str) -> String {
    format!(
        "About finding {} ({:?}, {}, \"{}\"): {}",
        finding.id,
        finding.severity,
        place(finding),
        finding.title.trim(),
        head(question.trim(), QUESTION_BYTES)
    )
}

fn finding_block(finding: &Finding) -> String {
    format!(
        "- Finding {} [{:?}] {} — {}\n  {}\n\n",
        finding.id,
        finding.severity,
        place(finding),
        finding.title.trim(),
        finding.body.trim().replace('\n', "\n  "),
    )
}

/// Whole standing findings within `budget` bytes, and how many did not fit.
fn finding_list(findings: &[Finding], budget: usize) -> (String, usize) {
    let mut list = String::new();
    let mut left_out = 0;
    for finding in findings
        .iter()
        .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
    {
        let block = finding_block(finding);
        if list.len() + block.len() <= budget {
            list.push_str(&block);
        } else {
            left_out += 1;
        }
    }
    (list, left_out)
}

/// The first prompt of a review's chat: what was reviewed, the verdict, the
/// standing findings and an optional first question, closed by the read-only
/// rules, within [`PROMPT_BUDGET`] on whole findings.
#[must_use]
pub fn chat_prompt(
    subject: &str,
    run: &ReviewRun,
    findings: &[Finding],
    question: Option<&str>,
) -> String {
    let mut opening = format!(
        "You are the chat for a finished Forge Review of {subject} in this checkout \
         (head {}). The person reading the report will ask about it. Explain a \
         finding, read the code it points at, and say plainly whether it holds. \
         Never fix anything yourself: a fix goes to another agent.\n\n",
        run.head_oid.chars().take(7).collect::<String>()
    );
    if let Some(verdict) = run.verdict {
        opening.push_str(&format!(
            "Verdict: {} · score {}\n\n",
            super::verdict_label(verdict),
            super::score_label(run.score)
        ));
    }
    let summary = run.summary.trim();
    if !summary.is_empty() {
        opening.push_str("## Summary\n\n");
        opening.push_str(head(summary, CHAT_SUMMARY_BYTES));
        opening.push_str("\n\n");
    }
    let question = question
        .map(str::trim)
        .filter(|question| !question.is_empty())
        .map(|question| {
            format!(
                "## First question\n\n{}\n\n",
                head(question, QUESTION_BYTES)
            )
        })
        .unwrap_or_default();
    let rules = domain::pr_review::READ_ONLY_RULES;
    let heading = "## Findings\n\n";
    let fixed = opening.len() + heading.len() + question.len() + rules.len();
    let (list, left_out) = finding_list(findings, PROMPT_BUDGET.saturating_sub(fixed + 64));
    let mut prompt = opening;
    prompt.push_str(heading);
    if list.is_empty() && left_out == 0 {
        prompt.push_str("None standing.\n\n");
    }
    prompt.push_str(&list);
    if left_out > 0 {
        prompt.push_str(&format!("({left_out} more left out to fit.)\n\n"));
    }
    prompt.push_str(&question);
    prompt.push_str(rules);
    prompt
}

/// What a chat is told when its review ran again: the new verdict, what
/// resolved and what is new, within [`PROMPT_BUDGET`].
#[must_use]
pub fn chat_update(run: &ReviewRun, findings: &[Finding], resolved: &[Finding]) -> String {
    let mut update = format!(
        "The review ran again at head {}.",
        run.head_oid.chars().take(7).collect::<String>()
    );
    if let Some(verdict) = run.verdict {
        update.push_str(&format!(
            " Verdict: {} · score {}.",
            super::verdict_label(verdict),
            super::score_label(run.score)
        ));
    }
    update.push('\n');
    for finding in resolved {
        let line = format!(
            "Resolved: [{:?}] {} — {}\n",
            finding.severity,
            place(finding),
            finding.title.trim()
        );
        if update.len() + line.len() > PROMPT_BUDGET / 4 {
            break;
        }
        update.push_str(&line);
    }
    let fresh: Vec<Finding> = findings
        .iter()
        .filter(|finding| finding.lineage == Lineage::New)
        .cloned()
        .collect();
    let (list, left_out) = finding_list(&fresh, PROMPT_BUDGET.saturating_sub(update.len() + 64));
    if !list.is_empty() {
        update.push_str("New findings:\n");
        update.push_str(&list);
    }
    if left_out > 0 {
        update.push_str(&format!("({left_out} more left out to fit.)\n"));
    }
    update.push_str("Answer from this run from now on.");
    update
}

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

    #[test]
    fn the_list_filters_and_keys_move_within_it() {
        let mut dismissed = finding(2, "a.rs", Some(4), Severity::P1);
        dismissed.reaction = Some(FindingReaction::Dismiss);
        let mut persisting = finding(3, "b.rs", Some(1), Severity::P2);
        persisting.lineage = Lineage::Persisting;
        let findings = vec![
            finding(1, "a.rs", Some(2), Severity::P0),
            dismissed,
            persisting,
        ];
        let groups = group_findings(&findings, Grouping::Severity);
        assert_eq!(listed(&findings, &groups, FindingFilter::Open), [1, 3]);
        assert_eq!(listed(&findings, &groups, FindingFilter::New), [1]);
        assert_eq!(listed(&findings, &groups, FindingFilter::Dismissed), [2]);

        let order = [1, 3];
        assert_eq!(shown_finding(&order, Some(3)), Some(3));
        assert_eq!(
            shown_finding(&order, Some(2)),
            Some(1),
            "a hidden choice falls back"
        );
        assert_eq!(shown_finding(&[], Some(2)), None);
        assert_eq!(step(&order, Some(1), true), Some(3));
        assert_eq!(step(&order, Some(3), true), Some(3), "the end stays put");
        assert_eq!(step(&order, Some(1), false), Some(1));
        assert_eq!(step(&order, None, true), Some(1));

        assert_eq!(group_title(Grouping::Severity, "P1"), "Blocks merge · P1");
        assert_eq!(group_title(Grouping::Severity, "P2"), "Worth fixing · P2");
        assert_eq!(group_title(Grouping::File, "P1"), "P1");
    }

    #[test]
    fn a_chat_prompt_ends_with_the_read_only_rules_within_its_budget() {
        let mut review = run(ReviewId::new(), None, true);
        review.summary = "s".repeat(10 * 1024);
        let mut dismissed = finding(2, "gone.rs", Some(1), Severity::P1);
        dismissed.reaction = Some(FindingReaction::Dismiss);
        let mut findings = vec![finding(1, "a.rs", Some(2), Severity::P0), dismissed];
        let prompt = chat_prompt("PR #7", &review, &findings, Some("  is 1 real?  "));
        assert!(prompt.len() <= PROMPT_BUDGET, "{}", prompt.len());
        assert!(prompt.ends_with(domain::pr_review::READ_ONLY_RULES));
        assert!(prompt.contains("Finding 1 [P0] a.rs:2"));
        assert!(!prompt.contains("gone.rs"), "dismissed findings stay out");
        assert!(prompt.contains("## First question\n\nis 1 real?"));

        findings.extend((10..400).map(|id| {
            let mut f = finding(id, "c.rs", Some(id), Severity::P2);
            f.body = "y".repeat(200);
            f
        }));
        let prompt = chat_prompt("PR #7", &review, &findings, Some(&"q".repeat(9000)));
        assert!(prompt.len() <= PROMPT_BUDGET, "{}", prompt.len());
        assert!(prompt.contains("more left out to fit"));
        assert!(prompt.ends_with(domain::pr_review::READ_ONLY_RULES));
    }

    #[test]
    fn a_chat_update_names_what_resolved_and_what_is_new() {
        let review = run(ReviewId::new(), None, true);
        let mut persisting = finding(1, "a.rs", Some(2), Severity::P1);
        persisting.lineage = Lineage::Persisting;
        let update = chat_update(
            &review,
            &[persisting, finding(2, "b.rs", None, Severity::P2)],
            &[finding(9, "gone.rs", Some(3), Severity::P0)],
        );
        assert!(update.contains("Resolved: [P0] gone.rs:3"), "{update}");
        assert!(update.contains("Finding 2 [P2] b.rs"));
        assert!(
            !update.contains("Finding 1 "),
            "a persisting finding is not news"
        );
        assert!(update.len() <= PROMPT_BUDGET);
        assert_eq!(
            about(&finding(4, "a.rs", None, Severity::P2), " why? "),
            "About finding 4 (P2, a.rs, \"finding 4\"): why?"
        );
        assert_eq!(head("aé", 2), "a");
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
