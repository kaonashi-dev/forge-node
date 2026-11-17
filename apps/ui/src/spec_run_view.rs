//! One spec run's panel: its stage, what the gate shows, and the decision.
//!
//! The content is read through the daemon (`GetSpecRun`) when the panel
//! opens and again when the run's `updated_at` moves, never per frame.

use domain::spec_run::model::{
    Delivery, GateDecision, Placement, Route, SpecOptions, SpecRunDetail, SpecRunView, SpecStage,
};
use domain::{SpecRunId, Timestamp};
use rune::{El, ScrollSpec, TextEdit, col, row, text};
use rune_kit::{
    Button, ButtonVariant, Checkbox, ControlSize, Kit, SegmentedControl, Switch, TextArea,
    TextInput,
};

use crate::app::AppState;
use crate::spec_gate::{self, Choices, CriterionRow, Next};

const DOCUMENT_MAX_H: f32 = 320.0;
const ID_W: f32 = 64.0;
const STATE_W: f32 = 96.0;
const DELIVERIES: [(Delivery, &str); 3] = [
    (Delivery::NoCommit, "No commit"),
    (Delivery::Commit, "Commit on its branch"),
    (Delivery::DraftPr, "Draft PR after G3"),
];

pub struct SpecPanel {
    pub run: SpecRunId,
    pub detail: Option<Box<SpecRunDetail>>,
    pub criteria: Vec<CriterionRow>,
    /// The `updated_at` the last read was asked for.
    pub asked: Option<Timestamp>,
    pub load_error: Option<String>,
    /// The stage the editable copy below was seeded for; a reload inside the
    /// same stage keeps what the person has typed.
    pub seeded: Option<SpecStage>,
    pub options: Option<SpecOptions>,
    pub worktree: bool,
    pub branch: TextEdit,
    pub save_gate_action: bool,
    pub manual_checked: Vec<String>,
    pub next: Next,
    pub reason: TextEdit,
    pub sending: bool,
    pub error: Option<String>,
}

impl SpecPanel {
    #[must_use]
    pub fn new(run: SpecRunId) -> Self {
        Self {
            run,
            detail: None,
            criteria: Vec::new(),
            asked: None,
            load_error: None,
            seeded: None,
            options: None,
            worktree: true,
            branch: TextEdit::new(""),
            save_gate_action: false,
            manual_checked: Vec::new(),
            next: Next::Finish,
            reason: TextEdit::new(""),
            sending: false,
            error: None,
        }
    }

    fn seed(&mut self, run: &SpecRunView) {
        self.seeded = Some(run.stage);
        self.options = Some(run.options.clone());
        self.worktree = matches!(run.options.placement, Placement::Worktree { .. });
        self.branch = TextEdit::new(worktree_branch(&run.options).unwrap_or_default());
        self.save_gate_action = false;
        self.manual_checked.clone_from(&run.manual_checked);
        self.next = Next::Finish;
        self.reason = TextEdit::new("");
        self.error = None;
    }

    /// What the person typed, with the placement taken from the branch field.
    fn edited_options(&self, run: &SpecRunView) -> Option<SpecOptions> {
        let mut options = self.options.clone()?;
        let fallback = worktree_branch(&run.options)
            .map_or_else(|| crate::spec_run::default_branch(&run.name), str::to_owned);
        options.placement = spec_gate::placement(self.worktree, &self.branch.text, &fallback);
        Some(options)
    }
}

fn worktree_branch(options: &SpecOptions) -> Option<&str> {
    match &options.placement {
        Placement::Worktree { branch, .. } => Some(branch.as_str()),
        Placement::Checkout => None,
    }
}

fn live_run(s: &AppState, id: SpecRunId) -> Option<&SpecRunView> {
    s.store
        .as_ref()
        .and_then(|store| store.spec_runs.iter().find(|run| run.id == id))
        .or_else(|| {
            s.spec_panel
                .as_ref()
                .and_then(|panel| panel.detail.as_deref())
                .map(|detail| &detail.run)
                .filter(|run| run.id == id)
        })
}

impl AppState {
    pub fn open_spec_run(&mut self, id: SpecRunId) {
        self.spec_panel = Some(SpecPanel::new(id));
        self.load_spec_run();
    }

    fn load_spec_run(&mut self) {
        let Some(id) = self.spec_panel.as_ref().map(|panel| panel.run) else {
            return;
        };
        let updated = live_run(self, id).map(|run| run.updated_at);
        let result = match &self.connection {
            Some(connection) => connection.get_spec_run(self.generation, id),
            None => Err("Not connected to the daemon.".to_string()),
        };
        if let Some(panel) = self.spec_panel.as_mut() {
            panel.asked = updated.or(panel.asked);
            panel.load_error = result.err();
        }
    }

    /// A `SpecRunUpdated` for the open run re-reads it once per update.
    pub fn spec_run_changed(&mut self, run: &SpecRunView) {
        let stale = self.spec_panel.as_ref().is_some_and(|panel| {
            panel.run == run.id && spec_gate::needs_reload(panel.asked, run.updated_at)
        });
        if stale {
            self.load_spec_run();
        }
    }

    pub fn apply_spec_run(
        &mut self,
        id: SpecRunId,
        result: Result<Box<SpecRunDetail>, String>,
    ) -> bool {
        let Some(panel) = self.spec_panel.as_mut().filter(|panel| panel.run == id) else {
            return false;
        };
        match result {
            Ok(detail) => {
                if panel.seeded != Some(detail.run.stage) {
                    panel.seed(&detail.run);
                }
                panel.criteria = detail
                    .report
                    .as_deref()
                    .map(spec_gate::criteria_rows)
                    .unwrap_or_default();
                panel.detail = Some(detail);
                panel.load_error = None;
            }
            Err(error) => panel.load_error = Some(error),
        }
        true
    }

    fn send_spec_gate(&mut self, decision: Option<GateDecision>) {
        let Some(id) = self.spec_panel.as_ref().map(|panel| panel.run) else {
            return;
        };
        let reject = matches!(decision, Some(GateDecision::Reject { .. }));
        let gate = live_run(self, id).and_then(|run| {
            if reject {
                spec_gate::reject_gate(run.stage)
            } else {
                spec_gate::gate(run.stage)
            }
        });
        let (Some(gate), Some(decision)) = (gate, decision) else {
            return;
        };
        let result = match &self.connection {
            Some(connection) => connection.spec_gate(id, gate, decision),
            None => Err("Not connected to the daemon.".to_string()),
        };
        if let Some(panel) = self.spec_panel.as_mut() {
            panel.sending = result.is_ok();
            panel.error = result.err();
        }
    }

    fn approve_spec_gate(&mut self) {
        let decision = self.spec_panel.as_ref().and_then(|panel| {
            let run = live_run(self, panel.run)?;
            approval(panel, run)
        });
        self.send_spec_gate(decision);
    }

    fn refuse_spec_gate(&mut self, reject: bool) {
        let decision = self
            .spec_panel
            .as_ref()
            .and_then(|panel| spec_gate::refuse(reject, &panel.reason.text));
        self.send_spec_gate(decision);
    }

    /// The daemon's answer to a gate decision: the run moves on and the panel
    /// closes, or the panel says why it did not.
    pub fn spec_gate_result(&mut self, ok: bool, message: &str) {
        if ok {
            self.spec_panel = None;
        } else if let Some(panel) = self.spec_panel.as_mut() {
            panel.sending = false;
            panel.error = Some(message.to_owned());
        }
    }
}

fn approval(panel: &SpecPanel, run: &SpecRunView) -> Option<GateDecision> {
    let options = panel.edited_options(run);
    spec_gate::approve(
        run.stage,
        run.route,
        &Choices {
            options: options.as_ref(),
            save_gate_action: panel.save_gate_action,
            manual_checked: &panel.manual_checked,
            next: (run.stage == SpecStage::ResultGate).then_some(panel.next),
            reason: &panel.reason.text,
        },
    )
}

fn route_label(route: Route) -> &'static str {
    match route {
        Route::Short => "short: the spec carries the plan",
        Route::Standard => "standard",
        Route::Full => "full: one task per capability",
    }
}

fn with_panel(s: &mut AppState, f: impl FnOnce(&mut SpecPanel)) {
    if let Some(panel) = s.spec_panel.as_mut() {
        f(panel);
    }
}

fn heading(k: &Kit, words: impl Into<String>) -> El {
    text(words.into())
        .font(k.ty.small_medium)
        .color(k.c.text_base)
}

fn faint(k: &Kit, words: impl Into<String>) -> El {
    text(words.into())
        .font(k.ty.text_xsmall)
        .color(k.c.text_faint)
        .wrap()
}

fn document(k: &Kit, key: &str, body: Option<&str>, missing: &str) -> El {
    let Some(body) = body.filter(|body| !body.trim().is_empty()) else {
        return faint(k, missing.to_owned()).key(key.to_owned());
    };
    col()
        .key(key.to_owned())
        .w_full()
        .max_h(DOCUMENT_MAX_H)
        .min_h(0.0)
        .padding(crate::ds::SPACE_4)
        .radius(k.radius.md)
        .fill(k.c.surface_weak)
        .border(1.0, k.c.border_weak)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            text(body.to_owned())
                .font(k.ty.mono)
                .color(k.c.text_base)
                .wrap(),
        )
}

fn link(k: &Kit, key: &str, label: &'static str, url: Option<&String>) -> Option<El> {
    let url = url?.clone();
    Some(
        Button::new(label)
            .icon(&crate::glyphs::GLOBE)
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .help(url.clone())
            .on_press_tx(move |_s: &mut AppState, tx| {
                let _ = tx.open_url(&url);
            })
            .build(k)
            .key(key.to_owned()),
    )
}

fn cells(k: &Kit, key: String, columns: [(String, Option<f32>, bool); 4], ink: rune::Color) -> El {
    let mut line = row()
        .key(key)
        .w_full()
        .gap(crate::ds::SPACE_4)
        .items_start();
    for (words, width, mono) in columns {
        let cell = text(words)
            .font(if mono { k.ty.mono } else { k.ty.small })
            .color(ink)
            .wrap();
        line = line.child(match width {
            Some(width) => cell.w(width),
            None => cell.grow(1.0).min_w(0.0),
        });
    }
    line
}

fn planned_checks(k: &Kit, checks: &[(String, String)]) -> El {
    let mut table = col()
        .key("spec-planned-checks")
        .role("table")
        .label("The commands Forge will run")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .child(heading(k, "The commands Forge will run"));
    if checks.is_empty() {
        return table.child(faint(k, "The plan names no command."));
    }
    for (index, (criterion, command)) in checks.iter().enumerate() {
        table = table.child(
            row()
                .key(format!("spec-check-{index}"))
                .w_full()
                .gap(crate::ds::SPACE_4)
                .items_start()
                .child(
                    text(criterion.clone())
                        .font(k.ty.small_medium)
                        .color(k.c.text_base)
                        .w(ID_W),
                )
                .child(
                    text(command.clone())
                        .font(k.ty.mono)
                        .color(k.c.text_base)
                        .wrap()
                        .grow(1.0)
                        .min_w(0.0),
                ),
        );
    }
    table
}

fn criteria_table(k: &Kit, rows: &[CriterionRow]) -> El {
    let mut table = col()
        .key("spec-criteria")
        .role("table")
        .label("Acceptance criteria")
        .w_full()
        .gap(crate::ds::SPACE_2);
    if rows.is_empty() {
        return table.child(faint(k, "The report has no criteria block."));
    }
    let header = |words: &str| (words.to_owned(), None, false);
    table = table.child(cells(
        k,
        "spec-criteria-head".into(),
        [
            ("ID".into(), Some(ID_W), false),
            ("State".into(), Some(STATE_W), false),
            header("How it was proven"),
            header("Result"),
        ],
        k.c.text_muted,
    ));
    for criterion in rows {
        table = table.child(cells(
            k,
            format!("spec-criterion-{}", criterion.id),
            [
                (criterion.id.clone(), Some(ID_W), false),
                (criterion.state.to_owned(), Some(STATE_W), false),
                (criterion.proof.clone(), None, true),
                (criterion.result.clone(), None, false),
            ],
            k.c.text_base,
        ));
    }
    table
}

fn options_editor(k: &Kit, panel: &SpecPanel, stage: SpecStage) -> Option<El> {
    let options = panel.options.as_ref()?;
    let mut editor = col()
        .key("spec-options")
        .w_full()
        .gap(crate::ds::SPACE_4)
        .child(heading(k, "How the coder works"))
        .child(
            SegmentedControl::new(
                &["New worktree", "This checkout"],
                Some(usize::from(!panel.worktree)),
            )
            .label("Where the coder works")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                with_panel(s, |panel| panel.worktree = index == 0);
            })
            .build(k)
            .key("spec-options-placement"),
        );
    if panel.worktree {
        editor = editor.child(
            TextInput::new(&panel.branch)
                .label("Worktree branch")
                .placeholder("Empty: spec/<the run's name>")
                .width(None)
                .on_edit(|s: &mut AppState| match s.spec_panel.as_mut() {
                    Some(panel) => &mut panel.branch,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("spec-options-branch"),
        );
    }
    let delivery_labels: Vec<&str> = DELIVERIES.iter().map(|(_, label)| *label).collect();
    let rounds_labels = crate::spec_run::ROUNDS.map(|count| count.to_string());
    let (forge_review, per_stage, save) = (
        options.forge_review,
        options.session_per_stage,
        panel.save_gate_action,
    );
    editor = editor
        .child(
            SegmentedControl::new(
                &delivery_labels,
                DELIVERIES.iter().position(|(d, _)| *d == options.delivery),
            )
            .label("Delivery")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                with_panel(s, |panel| {
                    if let (Some(options), Some((delivery, _))) =
                        (panel.options.as_mut(), DELIVERIES.get(index))
                    {
                        options.delivery = *delivery;
                    }
                });
            })
            .build(k)
            .key("spec-options-delivery"),
        )
        .child(
            SegmentedControl::new(
                &rounds_labels,
                crate::spec_run::ROUNDS
                    .iter()
                    .position(|count| *count == options.rounds),
            )
            .label("Review rounds")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                with_panel(s, |panel| {
                    if let (Some(options), Some(count)) =
                        (panel.options.as_mut(), crate::spec_run::ROUNDS.get(index))
                    {
                        options.rounds = *count;
                    }
                });
            })
            .build(k)
            .key("spec-options-rounds"),
        )
        .child(
            Switch::new(forge_review)
                .label("Forge Review")
                .on_toggle(move |s: &mut AppState| {
                    with_panel(s, |panel| {
                        if let Some(options) = panel.options.as_mut() {
                            options.forge_review = !forge_review;
                        }
                    });
                })
                .build(k)
                .key("spec-options-review"),
        )
        .child(
            Switch::new(per_stage)
                .label("New session per stage")
                .on_toggle(move |s: &mut AppState| {
                    with_panel(s, |panel| {
                        if let Some(options) = panel.options.as_mut() {
                            options.session_per_stage = !per_stage;
                        }
                    });
                })
                .build(k)
                .key("spec-options-per-stage"),
        );
    if stage == SpecStage::PlanGate || stage == SpecStage::SpecGate {
        editor = editor.child(
            Checkbox::new(save)
                .label("Save the gate as the project's gate action")
                .on_toggle(move |s: &mut AppState| {
                    with_panel(s, |panel| panel.save_gate_action = !save);
                })
                .build(k)
                .key("spec-options-save-gate"),
        );
    }
    Some(editor)
}

fn result_gate(k: &Kit, panel: &SpecPanel, run: &SpecRunView, detail: &SpecRunDetail) -> El {
    let mut body = col().key("spec-result").w_full().gap(crate::ds::SPACE_6);
    body = body.child(heading(
        k,
        format!(
            "Outcome: {}",
            run.outcome.map_or("not reported", spec_gate::outcome_label)
        ),
    ));
    body = body.child(criteria_table(k, &panel.criteria));
    if let Some(link) = link(
        k,
        "spec-report-link",
        "Open the report artifact",
        run.report_artifact_url.as_ref(),
    ) {
        body = body.child(link);
    }
    if !detail.manual_checks.is_empty() {
        let mut checks = col()
            .key("spec-manual")
            .w_full()
            .gap(crate::ds::SPACE_3)
            .child(heading(k, "Manual checks you ran"));
        for (index, check) in detail.manual_checks.iter().enumerate() {
            let on = panel.manual_checked.contains(check);
            let named = check.clone();
            checks = checks.child(
                Checkbox::new(on)
                    .label(check.clone())
                    .on_toggle(move |s: &mut AppState| {
                        with_panel(s, |panel| {
                            if on {
                                panel.manual_checked.retain(|c| *c != named);
                            } else {
                                panel.manual_checked.push(named.clone());
                            }
                        });
                    })
                    .build(k)
                    .key(format!("spec-manual-{index}")),
            );
        }
        body = body.child(checks);
    }
    let choices = spec_gate::next_choices(&run.options);
    let labels: Vec<&str> = choices.iter().map(|next| next.label()).collect();
    let picks = choices.clone();
    body.child(heading(k, "What next")).child(
        SegmentedControl::new(&labels, choices.iter().position(|next| *next == panel.next))
            .label("What next")
            .fit()
            .on_pick(move |s: &mut AppState, index: usize, _| {
                if let Some(next) = picks.get(index).copied() {
                    with_panel(s, |panel| panel.next = next);
                }
            })
            .build(k)
            .key("spec-next"),
    )
}

fn stage_body(k: &Kit, panel: &SpecPanel, run: &SpecRunView, detail: &SpecRunDetail) -> El {
    let mut body = col().key("spec-stage").w_full().gap(crate::ds::SPACE_6);
    match run.stage {
        SpecStage::SpecGate => {
            body = body
                .child(faint(k, format!("Route: {}", route_label(run.route))))
                .child(document(
                    k,
                    "spec-document",
                    detail.spec.as_deref(),
                    "The spec has not been saved yet.",
                ));
            if let Some(link) = link(
                k,
                "spec-artifact-link",
                "Open the spec artifact",
                run.spec_artifact_url.as_ref(),
            ) {
                body = body.child(link);
            }
            if spec_gate::edits_options(run.stage, run.route)
                && let Some(editor) = options_editor(k, panel, run.stage)
            {
                body = body.child(editor);
            }
        }
        SpecStage::PlanGate => {
            body = body
                .child(heading(k, "Plan"))
                .child(document(
                    k,
                    "spec-plan",
                    detail.plan.as_deref(),
                    "The plan has not been saved yet.",
                ))
                .child(planned_checks(k, &detail.planned_checks));
            if let Some(editor) = options_editor(k, panel, run.stage) {
                body = body.child(editor);
            }
        }
        SpecStage::ResultGate => body = body.child(result_gate(k, panel, run, detail)),
        _ => {
            body = body
                .child(faint(
                    k,
                    format!(
                        "{} · {} session{}",
                        spec_gate::stage_label(run.stage),
                        run.sessions.len(),
                        if run.sessions.len() == 1 { "" } else { "s" }
                    ),
                ))
                .children(
                    (run.stage == SpecStage::Exited)
                        .then(|| document(k, "spec-exit", detail.exit.as_deref(), "")),
                )
                .children(link(
                    k,
                    "spec-artifact-link",
                    "Open the spec artifact",
                    run.spec_artifact_url.as_ref(),
                ))
                .children(link(
                    k,
                    "spec-report-link",
                    "Open the report artifact",
                    run.report_artifact_url.as_ref(),
                ));
        }
    }
    body
}

pub fn panel(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(panel) = &s.spec_panel else {
        return;
    };
    let close = |s: &mut AppState| s.spec_panel = None;
    let Some(run) = live_run(s, panel.run) else {
        crate::modal::Modal::new("spec-run", "Spec run")
            .width(crate::modal::WIDTH_STANDARD)
            .on_close(close)
            .child(faint(
                k,
                panel
                    .load_error
                    .clone()
                    .unwrap_or_else(|| "This spec run is no longer listed.".into()),
            ))
            .build(s, k, layer);
        return;
    };
    let roles = crate::theme::roles(k);
    let mut body = col().key("spec-run-body").w_full().gap(crate::ds::SPACE_6);
    if let Some(attention) = &run.attention {
        body = body.child(
            text(attention.clone())
                .key("spec-attention")
                .role("status")
                .font(k.ty.small)
                .color(roles.needs_you.text)
                .wrap(),
        );
    }
    body = match (&panel.detail, &panel.load_error) {
        (_, Some(error)) => body.child(
            text(error.clone())
                .key("spec-load-error")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.danger.fg)
                .wrap(),
        ),
        (Some(detail), None) => body.child(stage_body(k, panel, run, detail)),
        (None, None) => body.child(faint(k, "Reading the run…")),
    };
    let gate = spec_gate::gate(run.stage).filter(|_| panel.detail.is_some());
    let mut footer = vec![
        Button::new("Close")
            .variant(ButtonVariant::GhostMuted)
            .on_press(close)
            .build(k)
            .key("spec-close"),
    ];
    if gate.is_some() {
        let another = run.stage == SpecStage::ResultGate && panel.next == Next::AnotherRound;
        body = body.child(
            TextArea::new(&panel.reason)
                .label("Reason")
                .placeholder(if another {
                    "What the coder should do in the next round"
                } else {
                    "Why: what to change, or why to stop (needed to request changes or reject)"
                })
                .width(None)
                .on_edit(|s: &mut AppState| match s.spec_panel.as_mut() {
                    Some(panel) => &mut panel.reason,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("spec-reason")
                .h(72.0),
        );
        let blank = panel.reason.text.trim().is_empty() || panel.sending;
        let approve_label = if run.stage == SpecStage::ResultGate {
            panel.next.label()
        } else {
            "Approve"
        };
        footer.extend([
            Button::new("Reject")
                .variant(ButtonVariant::Danger)
                .disabled(blank)
                .on_press(|s: &mut AppState| s.refuse_spec_gate(true))
                .build(k)
                .key("spec-reject"),
            Button::new("Request changes")
                .variant(ButtonVariant::Neutral)
                .disabled(blank)
                .on_press(|s: &mut AppState| s.refuse_spec_gate(false))
                .build(k)
                .key("spec-changes"),
            Button::new(if panel.sending {
                "Sending…"
            } else {
                approve_label
            })
            .variant(ButtonVariant::Contrast)
            .disabled(panel.sending || approval(panel, run).is_none())
            .on_press(|s: &mut AppState| s.approve_spec_gate())
            .build(k)
            .key("spec-approve"),
        ]);
    } else if !run.stage.is_closed() && panel.detail.is_some() {
        body = body.child(
            TextArea::new(&panel.reason)
                .label("Reason")
                .placeholder("Why to stop this run (needed to reject it)")
                .width(None)
                .on_edit(|s: &mut AppState| match s.spec_panel.as_mut() {
                    Some(panel) => &mut panel.reason,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("spec-reason")
                .h(72.0),
        );
        footer.push(
            Button::new("Reject run")
                .variant(ButtonVariant::Danger)
                .disabled(panel.reason.text.trim().is_empty() || panel.sending)
                .on_press(|s: &mut AppState| s.refuse_spec_gate(true))
                .build(k)
                .key("spec-reject"),
        );
    }
    if let Some(error) = &panel.error {
        body = body.child(
            text(error.clone())
                .key("spec-error")
                .role("alert")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg)
                .wrap(),
        );
    }
    crate::modal::Modal::new("spec-run", run.name.clone())
        .description(spec_gate::progress(run))
        .width(crate::modal::WIDTH_WIDE)
        .on_close(close)
        .child(body)
        .footer(footer)
        .build(s, k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::spec_run::model::GateDecision;

    #[test]
    fn a_reload_in_the_same_stage_keeps_the_edits_that_approve_sends() {
        let mut store = crate::connection::fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let run = crate::test_support::spec_run(SpecStage::PlanGate, Timestamp::now());
        store.spec_runs = vec![run.clone()];
        let mut s = AppState::for_bench(
            store,
            std::sync::Arc::new(crate::connection::Metrics::default()),
            crate::terminal::SharedTerminal::new(),
        );
        s.open_spec_run(run.id);
        let detail = SpecRunDetail {
            run: run.clone(),
            spec: None,
            research: None,
            plan: Some("1. Add the route".into()),
            report: None,
            exit: None,
            planned_checks: vec![("AC-1".into(), "cargo test".into())],
            manual_checks: Vec::new(),
        };
        assert!(s.apply_spec_run(run.id, Ok(Box::new(detail.clone()))));
        with_panel(&mut s, |panel| {
            panel.branch = TextEdit::new("  ");
            panel.save_gate_action = true;
            if let Some(options) = panel.options.as_mut() {
                options.delivery = Delivery::DraftPr;
            }
        });
        assert!(s.apply_spec_run(run.id, Ok(Box::new(detail))));
        let panel = s.spec_panel.as_ref().expect("the panel stays open");
        assert_eq!(
            panel.load_error, None,
            "the read cleared the missing-worker note"
        );
        assert_eq!(
            approval(panel, &run),
            Some(GateDecision::Approve {
                options: Some(SpecOptions {
                    delivery: Delivery::DraftPr,
                    ..run.options.clone()
                }),
                save_gate_action: true,
                manual_checked: Vec::new(),
                next: None,
            }),
            "an emptied branch falls back to the run's own"
        );
        assert!(!s.apply_spec_run(domain::SpecRunId::new(), Err("other".into())));
    }
}
