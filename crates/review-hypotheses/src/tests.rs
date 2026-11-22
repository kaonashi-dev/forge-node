use domain::{DiffStatus, Severity};

use super::*;

fn file(path: &str, patch: &str) -> DiffFile {
    DiffFile {
        path: path.into(),
        status: DiffStatus::Modified,
        additions: 1,
        deletions: 0,
        binary: false,
        truncated: false,
        patch: patch.into(),
    }
}

#[test]
fn the_default_order_starts_with_the_three_default_reviewers() {
    use Hypothesis as H;
    assert_eq!(select(1, &[]), [H::AuthBypass]);
    assert_eq!(
        select(3, &[]),
        [H::AuthBypass, H::BrokenContract, H::ErrorSwallow]
    );
    assert_eq!(
        select(5, &[]),
        [
            H::AuthBypass,
            H::BrokenContract,
            H::ErrorSwallow,
            H::RaceCondition,
            H::NPlusOneQuery
        ]
    );
    assert_eq!(select(8, &[]).len(), 8);
    assert_eq!(
        select(8, &[])[5..],
        [H::MissingValidation, H::RetryNesting, H::DefaultMismatch]
    );
    assert_eq!(select(0, &[]).len(), 1);
    assert_eq!(select(20, &[]).len(), 8);
    let mut every: Vec<Hypothesis> = ORDER.to_vec();
    every.sort();
    let mut all = Hypothesis::ALL.to_vec();
    all.sort();
    assert_eq!(every, all, "the order names each hypothesis once");
}

#[test]
fn explicit_hypotheses_override_the_size() {
    use Hypothesis as H;
    assert_eq!(
        select(3, &[H::MigrationGap, H::MigrationGap, H::DeadCode]),
        [H::MigrationGap, H::DeadCode]
    );
    assert_eq!(select(1, &ORDER).len(), MAX_AGENTS);
}

#[test]
fn a_slice_is_the_files_matching_the_hypothesis_or_else_every_file() {
    let files = [
        file("src/worker.rs", "+let guard = state.lock();\n"),
        file("db/migrations/2.sql", "+ALTER TABLE users ADD COLUMN x;\n"),
        file("README.md", "+words\n"),
    ];
    let plan = plan(
        &[
            Hypothesis::RaceCondition,
            Hypothesis::MigrationGap,
            Hypothesis::DeadCode,
        ],
        &files,
    );
    assert_eq!(plan.agents[0].slice_paths, ["src/worker.rs"]);
    assert_eq!(plan.agents[1].slice_paths, ["db/migrations/2.sql"]);
    assert_eq!(
        plan.agents[2].slice_paths.len(),
        3,
        "nothing matched: every file"
    );
    assert_eq!(
        plan.agents[0].extra_instructions,
        instruction(Hypothesis::RaceCondition)
    );
}

#[test]
fn a_slice_file_names_its_files_redacted_and_the_prompt_asks_for_v2() {
    let spec = AgentSpec {
        hypothesis: Hypothesis::ErrorSwallow,
        slice_paths: vec!["src/ghp_0123456789abcdefghijklmnopqrstuvwxyz.rs".into()],
        extra_instructions: instruction(Hypothesis::ErrorSwallow).into(),
    };
    let text = slice_file(&spec, "n0nce");
    assert!(text.starts_with("# Hypothesis: ErrorSwallow\n"));
    assert!(
        text.contains("<slice-n0nce>\n- src/«redacted».rs\n</slice-n0nce>"),
        "{text}"
    );
    let resolved = review::resolve(
        &review::Config::default(),
        &[],
        &review::builtins(),
        None,
        None,
        None,
    )
    .expect("standard");
    let prompt = prompt(
        &resolved,
        "/p/pack.md".as_ref(),
        "/p/slice-ErrorSwallow.md".as_ref(),
        &spec,
        "n0nce",
    );
    assert!(
        prompt.contains("\"schema\":\"forge.review/v2\""),
        "{prompt}"
    );
    assert!(prompt.contains("\"hypothesis\":\"ErrorSwallow\""));
    assert!(prompt.contains("\"patterns\":[]"));
    assert!(!prompt.contains("\"diagram\""));
    assert!(prompt.contains("/p/slice-ErrorSwallow.md"));
    assert!(prompt.contains("Do not emit `diagrams` or `pr_confidence`"));
}

fn finding(fingerprint: &str, severity: Severity, rule: Option<&str>) -> Finding {
    Finding {
        id: 9,
        fingerprint: fingerprint.into(),
        path: "src/a.rs".into(),
        line: Some(1),
        end_line: None,
        severity,
        category: "logic".into(),
        rule_id: rule.map(str::to_owned),
        confidence: 0.9,
        title: "t".into(),
        body: "b".into(),
        evidence: "e".into(),
        suggestion: None,
        lineage: domain::Lineage::New,
        reaction: None,
        hypothesis: None,
        kb_ref: None,
    }
}

fn validated(findings: Vec<Finding>) -> AgentAnswer {
    AgentAnswer::Validated(review::Validated {
        summary: "s".into(),
        findings,
        invalid_anchors: 0,
        unverified: 0,
        total: 0,
        low_confidence: 0,
        trimmed: 0,
        suppressed: 0,
        withheld: Vec::new(),
    })
}

#[test]
fn the_same_fingerprint_keeps_the_more_severe_and_then_the_ruled_one() {
    let out = consolidate(
        vec![
            (
                Hypothesis::AuthBypass,
                validated(vec![
                    finding("f1", Severity::P2, None),
                    finding("f2", Severity::P1, None),
                ]),
            ),
            (
                Hypothesis::BrokenContract,
                validated(vec![
                    finding("f1", Severity::P1, None),
                    finding("f2", Severity::P1, Some("rule")),
                ]),
            ),
            (Hypothesis::ErrorSwallow, validated(Vec::new())),
        ],
        40,
    )
    .expect("answered");
    assert_eq!(out.findings.len(), 2);
    let f1 = out
        .findings
        .iter()
        .find(|f| f.fingerprint == "f1")
        .expect("f1");
    assert_eq!(
        (f1.severity, f1.hypothesis),
        (Severity::P1, Some(Hypothesis::BrokenContract))
    );
    let f2 = out
        .findings
        .iter()
        .find(|f| f.fingerprint == "f2")
        .expect("f2");
    assert_eq!(f2.rule_id.as_deref(), Some("rule"));
    assert_eq!(
        out.findings.iter().map(|f| f.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(out
        .notes
        .contains(&"hypothesis discarded: ErrorSwallow".to_owned()));
    assert_eq!(out.covered, [Hypothesis::BrokenContract]);
}

#[test]
fn a_rejected_agent_is_a_note_and_max_findings_applies_once() {
    let out = consolidate(
        vec![
            (
                Hypothesis::AuthBypass,
                AgentAnswer::Uncovered("timed out".into()),
            ),
            (
                Hypothesis::BrokenContract,
                validated(vec![
                    finding("a", Severity::P2, None),
                    finding("b", Severity::P0, None),
                ]),
            ),
            (
                Hypothesis::ErrorSwallow,
                validated(vec![finding("c", Severity::P1, None)]),
            ),
        ],
        2,
    )
    .expect("answered");
    assert_eq!(
        out.findings
            .iter()
            .map(|f| f.fingerprint.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    assert!(out
        .notes
        .iter()
        .any(|n| n == "hypothesis_uncovered: AuthBypass: timed out"));
    assert!(
        out.withheld.contains(&"a".to_owned()),
        "the cut one is withheld, not resolved"
    );
    assert!(consolidate(
        vec![(Hypothesis::AuthBypass, AgentAnswer::Uncovered("x".into()))],
        40
    )
    .is_none());
}
