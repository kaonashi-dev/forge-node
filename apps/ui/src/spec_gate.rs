//! What a spec run's gate offers and what each choice sends; the panel in
//! `spec_run_view` only draws it.

use domain::Timestamp;
use domain::spec_run::model::{
    Delivery, GateDecision, GateKind, NextAction, Placement, Route, RunOutcome, SpecOptions,
    SpecRunView, SpecStage,
};
use domain::spec_run::schema::{CriterionState, ReportCriterion, report_block};

const EXCERPT_CHARS: usize = 160;

#[must_use]
pub fn stage_label(stage: SpecStage) -> &'static str {
    match stage {
        SpecStage::Drafting => "Drafting the spec",
        SpecStage::SpecGate => "Spec gate",
        SpecStage::Planning => "Planning",
        SpecStage::PlanGate => "Plan gate",
        SpecStage::Executing => "Coding",
        SpecStage::Verifying => "Running checks",
        SpecStage::Reviewing => "Reviewing",
        SpecStage::ResultGate => "Result gate",
        SpecStage::Done => "Done",
        SpecStage::Rejected => "Rejected",
        SpecStage::Exited => "Ended without a spec",
        _ => stage.as_str(),
    }
}

#[must_use]
pub fn progress(run: &SpecRunView) -> String {
    let stage = stage_label(run.stage);
    if run.round == 0 {
        format!("{stage} · up to {} rounds", run.options.rounds)
    } else {
        format!("{stage} · round {}/{}", run.round, run.options.rounds)
    }
}

#[must_use]
pub fn outcome_label(outcome: RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Done => "Done",
        RunOutcome::DoneWithConcerns => "Done, with concerns",
        RunOutcome::NeedsDecision => "Needs your decision",
        RunOutcome::Blocked => "Blocked",
    }
}

#[must_use]
pub fn gate(stage: SpecStage) -> Option<GateKind> {
    match stage {
        SpecStage::SpecGate => Some(GateKind::Spec),
        SpecStage::PlanGate => Some(GateKind::Plan),
        SpecStage::ResultGate => Some(GateKind::Result),
        _ => None,
    }
}

/// G2, and G1 on the short route, where the two gates are one.
#[must_use]
pub fn edits_options(stage: SpecStage, route: Route) -> bool {
    stage == SpecStage::PlanGate || (stage == SpecStage::SpecGate && route == Route::Short)
}

/// Whether a run updated at `updated` is newer than the read last asked for.
#[must_use]
pub fn needs_reload(asked: Option<Timestamp>, updated: Timestamp) -> bool {
    asked.is_none_or(|asked| updated > asked)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Finish,
    OpenDraftPr,
    AnotherRound,
    DiscardWorktree,
}

impl Next {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Next::Finish => "Finish",
            Next::OpenDraftPr => "Open draft PR",
            Next::AnotherRound => "Another round",
            Next::DiscardWorktree => "Discard worktree",
        }
    }
}

#[must_use]
pub fn next_choices(options: &SpecOptions) -> Vec<Next> {
    let mut out = vec![Next::Finish];
    if options.delivery == Delivery::DraftPr {
        out.push(Next::OpenDraftPr);
    }
    out.push(Next::AnotherRound);
    if matches!(options.placement, Placement::Worktree { .. }) {
        out.push(Next::DiscardWorktree);
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices<'a> {
    pub options: Option<&'a SpecOptions>,
    pub save_gate_action: bool,
    pub manual_checked: &'a [String],
    pub next: Option<Next>,
    pub reason: &'a str,
}

/// The Approve decision for `stage`, or `None` while it cannot be sent: off
/// a gate, or another round with no text for the coder.
#[must_use]
pub fn approve(stage: SpecStage, route: Route, choices: &Choices<'_>) -> Option<GateDecision> {
    let gate = gate(stage)?;
    let reason = choices.reason.trim();
    let next = match (gate, choices.next) {
        (GateKind::Result, None | Some(Next::Finish)) => Some(NextAction::Finish),
        (GateKind::Result, Some(Next::OpenDraftPr)) => Some(NextAction::OpenDraftPr),
        (GateKind::Result, Some(Next::DiscardWorktree)) => Some(NextAction::DiscardWorktree),
        (GateKind::Result, Some(Next::AnotherRound)) if reason.is_empty() => return None,
        (GateKind::Result, Some(Next::AnotherRound)) => Some(NextAction::AnotherRound {
            text: reason.to_owned(),
        }),
        _ => None,
    };
    let result = gate == GateKind::Result;
    Some(GateDecision::Approve {
        options: choices
            .options
            .filter(|_| edits_options(stage, route))
            .cloned(),
        save_gate_action: choices.save_gate_action && edits_options(stage, route),
        manual_checked: if result {
            choices.manual_checked.to_vec()
        } else {
            Vec::new()
        },
        next,
    })
}

/// Request changes or reject; the daemon refuses either without a reason.
#[must_use]
pub fn refuse(reject: bool, reason: &str) -> Option<GateDecision> {
    let reason = reason.trim();
    (!reason.is_empty()).then(|| {
        if reject {
            GateDecision::Reject {
                reason: reason.to_owned(),
            }
        } else {
            GateDecision::Changes {
                reason: reason.to_owned(),
            }
        }
    })
}

/// The placement G2 sends: a worktree keeps a branch even when the field is
/// left empty.
#[must_use]
pub fn placement(worktree: bool, branch: &str, fallback: &str) -> Placement {
    if !worktree {
        return Placement::Checkout;
    }
    let branch = branch.trim();
    Placement::Worktree {
        branch: if branch.is_empty() { fallback } else { branch }.to_owned(),
        base: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CriterionRow {
    pub id: String,
    pub state: &'static str,
    pub proof: String,
    pub result: String,
}

#[must_use]
pub fn state_label(state: CriterionState) -> &'static str {
    match state {
        CriterionState::Met => "met",
        CriterionState::NotMet => "not met",
        CriterionState::NotVerified => "not verified",
        CriterionState::NeedsDecision => "needs decision",
    }
}

fn row(criterion: &ReportCriterion) -> CriterionRow {
    let evidence = criterion.evidence.as_ref();
    let excerpt = evidence
        .and_then(|e| e.excerpt.as_deref())
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| e.chars().take(EXCERPT_CHARS).collect::<String>());
    let exit = evidence
        .and_then(|e| e.exit_code)
        .map(|code| format!("exit {code}"));
    let result = match (exit, excerpt) {
        (Some(exit), Some(excerpt)) => format!("{exit} · {excerpt}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => criterion.reason.clone().unwrap_or_default(),
    };
    CriterionRow {
        id: criterion.id.clone(),
        state: state_label(criterion.state),
        proof: evidence
            .and_then(|e| e.command.clone())
            .unwrap_or_else(|| "—".to_owned()),
        result,
    }
}

/// The report's criteria and its `gate` row, or nothing when the report has
/// no readable block.
#[must_use]
pub fn criteria_rows(report: &str) -> Vec<CriterionRow> {
    report_block(report).map_or_else(Vec::new, |block| {
        block
            .criteria
            .iter()
            .chain(block.gate.as_ref())
            .map(row)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(placement: Placement, delivery: Delivery) -> SpecOptions {
        SpecOptions {
            placement,
            delivery,
            rounds: 2,
            forge_review: true,
            session_per_stage: false,
            publish_spec: false,
            publish_report: false,
        }
    }

    fn worktree() -> Placement {
        Placement::Worktree {
            branch: "spec/x".into(),
            base: None,
        }
    }

    #[test]
    fn the_result_gate_offers_a_draft_pr_and_a_discard_only_when_they_apply() {
        assert_eq!(
            next_choices(&options(Placement::Checkout, Delivery::NoCommit)),
            [Next::Finish, Next::AnotherRound]
        );
        assert_eq!(
            next_choices(&options(worktree(), Delivery::DraftPr)),
            [
                Next::Finish,
                Next::OpenDraftPr,
                Next::AnotherRound,
                Next::DiscardWorktree
            ]
        );
    }

    #[test]
    fn options_travel_only_where_the_gate_decides_them() {
        let edited = options(Placement::Checkout, Delivery::Commit);
        let choices = Choices {
            options: Some(&edited),
            save_gate_action: true,
            manual_checked: &["Log in by hand".to_owned()],
            next: None,
            reason: "",
        };
        assert_eq!(
            approve(SpecStage::PlanGate, Route::Standard, &choices),
            Some(GateDecision::Approve {
                options: Some(edited.clone()),
                save_gate_action: true,
                manual_checked: Vec::new(),
                next: None,
            })
        );
        assert_eq!(
            approve(SpecStage::SpecGate, Route::Standard, &choices),
            Some(GateDecision::Approve {
                options: None,
                save_gate_action: false,
                manual_checked: Vec::new(),
                next: None,
            })
        );
        assert!(
            matches!(
                approve(SpecStage::SpecGate, Route::Short, &choices),
                Some(GateDecision::Approve {
                    options: Some(_),
                    ..
                })
            ),
            "the short route decides the plan at the spec gate"
        );
        assert_eq!(
            approve(SpecStage::Executing, Route::Standard, &choices),
            None
        );
    }

    #[test]
    fn another_round_needs_text_and_carries_it_with_the_manual_checks() {
        let checked = ["Log in by hand".to_owned()];
        let mut choices = Choices {
            manual_checked: &checked,
            next: Some(Next::AnotherRound),
            reason: "  ",
            ..Choices::default()
        };
        assert_eq!(
            approve(SpecStage::ResultGate, Route::Standard, &choices),
            None
        );
        choices.reason = " Cover the empty list ";
        assert_eq!(
            approve(SpecStage::ResultGate, Route::Standard, &choices),
            Some(GateDecision::Approve {
                options: None,
                save_gate_action: false,
                manual_checked: checked.to_vec(),
                next: Some(NextAction::AnotherRound {
                    text: "Cover the empty list".into()
                }),
            })
        );
        choices.next = None;
        assert!(matches!(
            approve(SpecStage::ResultGate, Route::Standard, &choices),
            Some(GateDecision::Approve {
                next: Some(NextAction::Finish),
                ..
            })
        ));
    }

    #[test]
    fn changes_and_reject_need_a_reason() {
        assert_eq!(refuse(false, " \n"), None);
        assert_eq!(
            refuse(true, " too broad "),
            Some(GateDecision::Reject {
                reason: "too broad".into()
            })
        );
        assert_eq!(
            refuse(false, "split AC-2"),
            Some(GateDecision::Changes {
                reason: "split AC-2".into()
            })
        );
    }

    #[test]
    fn a_worktree_never_loses_its_branch_and_a_read_is_asked_once_per_update() {
        assert_eq!(placement(false, "spec/x", "spec/y"), Placement::Checkout);
        assert_eq!(
            placement(true, "  ", "spec/y"),
            Placement::Worktree {
                branch: "spec/y".into(),
                base: None
            }
        );
        let earlier = Timestamp::now();
        let later = Timestamp(earlier.0 + std::time::Duration::from_secs(1));
        assert!(needs_reload(None, earlier));
        assert!(needs_reload(Some(earlier), later));
        assert!(!needs_reload(Some(later), later));
        assert!(!needs_reload(Some(later), earlier));
    }

    #[test]
    fn progress_names_the_round_once_one_has_started() {
        let mut run = crate::test_support::spec_run(SpecStage::Planning, Timestamp::now());
        assert_eq!(progress(&run), "Planning · up to 3 rounds");
        run.stage = SpecStage::Executing;
        run.round = 2;
        assert_eq!(progress(&run), "Coding · round 2/3");
    }

    #[test]
    fn the_criteria_table_reads_the_report_block_and_its_gate() {
        let report = r#"# Report

```json
{"schema": "forge.spec_run/v1",
 "criteria": [
   {"id": "AC-1", "state": "met",
    "evidence": {"command": "cargo test -p x", "exit_code": 0, "excerpt": "1 passed"}},
   {"id": "AC-2", "state": "not_verified", "reason": "needs a browser"}
 ],
 "gate": {"id": "gate", "state": "not_met",
          "evidence": {"command": "scripts/dev check", "exit_code": 101}}}
```
"#;
        assert_eq!(
            criteria_rows(report),
            [
                CriterionRow {
                    id: "AC-1".into(),
                    state: "met",
                    proof: "cargo test -p x".into(),
                    result: "exit 0 · 1 passed".into(),
                },
                CriterionRow {
                    id: "AC-2".into(),
                    state: "not verified",
                    proof: "—".into(),
                    result: "needs a browser".into(),
                },
                CriterionRow {
                    id: "gate".into(),
                    state: "not met",
                    proof: "scripts/dev check".into(),
                    result: "exit 101".into(),
                },
            ]
        );
        assert!(criteria_rows("no block here").is_empty());
    }
}
