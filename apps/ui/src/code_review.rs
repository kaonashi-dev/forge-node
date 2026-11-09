//! Forge Review, as the original client's `ReviewView`: one workspace's structured review,
//! its presets and its findings. Starting and cancelling are daemon requests;
//! progress arrives as `ReviewUpdated` and findings are read once complete.

use std::collections::{HashMap, HashSet};

use client::ProviderInfo;
use domain::{
    AgentProviderId, Finding, ReviewId, ReviewRun, ReviewSettings, ReviewStage, ReviewStatus,
    Severity, Tier, Timestamp, Verdict, WorkspaceId,
};
use rune::{El, ScrollSpec, TextEdit, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Kit, Markdown, Progress, RadioGroup, RadioItem,
    SegmentedControl, TextArea,
};

use crate::app::AppState;
use crate::workbench::CenterMode;

pub const CUSTOM_PRESET: &str = "custom";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    Severity,
    File,
}

pub struct Report {
    pub run: ReviewId,
    pub findings: Vec<Finding>,
}

pub struct CodeReview {
    pub workspace: Option<WorkspaceId>,
    pub latest: HashMap<WorkspaceId, ReviewRun>,
    pub reports: HashMap<WorkspaceId, Report>,
    pub configs: HashMap<WorkspaceId, ReviewSettings>,
    pub errors: HashMap<WorkspaceId, String>,
    pub grouping: Grouping,
    pub preset: Option<String>,
    pub tier: Option<Tier>,
    pub provider: Option<String>,
    pub instructions: TextEdit,
    pub starting: bool,
    pub asked_config: HashMap<WorkspaceId, u64>,
    pub asked_list: HashMap<WorkspaceId, u64>,
    pub report_asked: HashSet<ReviewId>,
}

impl Default for CodeReview {
    fn default() -> Self {
        Self {
            workspace: None,
            latest: HashMap::new(),
            reports: HashMap::new(),
            configs: HashMap::new(),
            errors: HashMap::new(),
            grouping: Grouping::Severity,
            preset: None,
            tier: None,
            provider: None,
            instructions: TextEdit::new(""),
            starting: false,
            asked_config: HashMap::new(),
            asked_list: HashMap::new(),
            report_asked: HashSet::new(),
        }
    }
}

const SEVERITIES: [Severity; 3] = [Severity::P0, Severity::P1, Severity::P2];
const STAGES: [ReviewStage; 4] = [
    ReviewStage::Diff,
    ReviewStage::Pack,
    ReviewStage::Model,
    ReviewStage::Validate,
];

/// Findings grouped as the original client's `groupFindings`: severity groups are most
/// severe first; file groups are sorted by path and line. Empty groups drop.
#[must_use]
pub fn group_findings(findings: &[Finding], by: Grouping) -> Vec<(String, Vec<&Finding>)> {
    match by {
        Grouping::Severity => SEVERITIES
            .iter()
            .map(|severity| {
                let mut group: Vec<&Finding> = findings
                    .iter()
                    .filter(|finding| finding.severity == *severity)
                    .collect();
                group.sort_by(|a, b| {
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
                    let mut group: Vec<&Finding> = findings
                        .iter()
                        .filter(|finding| finding.path == path)
                        .collect();
                    group.sort_by_key(|finding| finding.line.unwrap_or(0));
                    (path.to_string(), group)
                })
                .collect()
        }
    }
}

/// An unmeasured head is not evidence the checkout moved.
#[must_use]
pub fn is_stale(run: &ReviewRun, head: Option<&str>) -> bool {
    head.is_some_and(|head| head != run.head_oid)
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

#[must_use]
pub fn stage_label(stage: Option<ReviewStage>) -> &'static str {
    match stage {
        Some(ReviewStage::Diff) => "Reading the diff",
        Some(ReviewStage::Pack) => "Building the pack",
        Some(ReviewStage::Model) => "Waiting for the reviewer",
        Some(ReviewStage::Validate) => "Validating findings",
        Some(_) => "Working",
        None => "Queued",
    }
}

/// Percent, halfway into the current stage: none of them reports its own.
#[must_use]
pub fn stage_progress(stage: Option<ReviewStage>) -> f32 {
    let Some(stage) = stage else {
        return 0.0;
    };
    let at = STAGES.iter().position(|known| *known == stage).unwrap_or(0);
    ((at as f32 + 0.5) / STAGES.len() as f32) * 100.0
}

#[must_use]
pub fn duration(created_at: Timestamp, finished_at: Option<Timestamp>, now: i64) -> String {
    let start = created_at.as_offset().unix_timestamp();
    let end = finished_at.map_or(now, |finished| finished.as_offset().unix_timestamp());
    let seconds = (end - start).max(0) as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3_600 {
        return format!("{}m {:02}s", seconds / 60, seconds % 60);
    }
    format!("{}h {:02}m", seconds / 3_600, (seconds % 3_600) / 60)
}

#[must_use]
pub fn score_label(score: Option<f32>) -> String {
    score.map_or_else(|| "—".to_string(), |score| format!("{score:.1} / 5"))
}

#[must_use]
pub fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Block => "Bloquea",
        Verdict::Review => "Revisar",
        Verdict::Ok => "OK",
    }
}

#[must_use]
pub fn status_label(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Queued => "Queued",
        ReviewStatus::Running => "Running",
        ReviewStatus::Completed => "Completed",
        ReviewStatus::Failed => "Failed",
        ReviewStatus::Cancelled => "Cancelled",
        ReviewStatus::Interrupted => "Interrupted",
        _ => "Unknown",
    }
}

#[must_use]
pub fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Base => "base",
        Tier::Plus => "plus",
        Tier::Apex => "apex",
    }
}

fn verdict_ink(k: &Kit, verdict: Verdict) -> rune::Color {
    match verdict {
        Verdict::Block => k.c.danger.fg,
        Verdict::Review => k.c.warning.fg,
        Verdict::Ok => k.c.success.fg,
    }
}

fn severity_ink(k: &Kit, severity: Severity) -> rune::Color {
    match severity {
        Severity::P0 => k.c.danger.fg,
        Severity::P1 => k.c.warning.fg,
        Severity::P2 => k.c.text_muted,
    }
}

/// Installed providers that can answer a structured review, in snapshot order.
#[must_use]
pub fn reviewers(providers: &[ProviderInfo]) -> Vec<(AgentProviderId, String)> {
    providers
        .iter()
        .filter(|info| {
            matches!(
                info.detection.status,
                domain::DetectionStatus::Installed { .. }
            ) && info.descriptor.review.is_some()
                && info.descriptor.structured_review.is_some()
        })
        .map(|info| {
            (
                info.descriptor.id.clone(),
                info.descriptor.display_name.clone(),
            )
        })
        .collect()
}

fn running(run: &ReviewRun) -> bool {
    !run.status.is_finished()
}

pub fn open(s: &mut AppState, workspace: WorkspaceId) {
    s.code_review.workspace = Some(workspace);
    s.code_review.errors.remove(&workspace);
    s.inspect(workspace, CenterMode::Review);
}

pub fn refresh(s: &mut AppState) {
    let Some(workspace) = s.code_review.workspace else {
        return;
    };
    s.code_review.asked_config.remove(&workspace);
    s.code_review.asked_list.remove(&workspace);
    s.code_review.errors.remove(&workspace);
    s.code_review.report_asked.clear();
    sync(s);
}

fn sync(s: &mut AppState) {
    let Some(workspace) = s.code_review.workspace else {
        return;
    };
    if s.workbench.mode != CenterMode::Review {
        return;
    }
    let generation = s.generation;
    if !s.code_review.configs.contains_key(&workspace)
        && s.code_review.asked_config.get(&workspace) != Some(&generation)
    {
        s.code_review.asked_config.insert(workspace, generation);
        let result = match &s.connection {
            Some(connection) => connection.review_config(generation, workspace),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            s.code_review.errors.insert(workspace, error);
        }
    }
    if !s.code_review.latest.contains_key(&workspace)
        && s.code_review.asked_list.get(&workspace) != Some(&generation)
    {
        s.code_review.asked_list.insert(workspace, generation);
        let result = match &s.connection {
            Some(connection) => connection.list_reviews(generation, workspace),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            s.code_review.errors.insert(workspace, error);
        }
    }
    if let Some(run) = s.code_review.latest.get(&workspace).cloned()
        && run.status == ReviewStatus::Completed
        && s.code_review
            .reports
            .get(&workspace)
            .is_none_or(|report| report.run != run.id)
        && s.code_review.report_asked.insert(run.id)
    {
        let result = match &s.connection {
            Some(connection) => connection.get_review(generation, workspace, run.id),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            s.code_review.report_asked.remove(&run.id);
            s.code_review.errors.insert(workspace, error);
        }
    }
}

impl AppState {
    pub fn sync_review(&mut self) {
        sync(self);
    }

    pub fn start_code_review(&mut self) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let Some(config) = self.code_review.configs.get(&workspace) else {
            self.code_review
                .errors
                .insert(workspace, "Review settings have not been read yet.".into());
            return;
        };
        let provider = self.code_review.provider.clone().or_else(|| {
            self.store.as_ref().and_then(|store| {
                reviewers(&store.providers)
                    .first()
                    .map(|(id, _)| id.to_string())
            })
        });
        let preset = self
            .code_review
            .preset
            .clone()
            .unwrap_or_else(|| config.default_preset.clone());
        let instructions = self.code_review.instructions.text.trim().to_string();
        let problem = if provider.is_none() {
            Some("No installed agent can run a Forge Review.")
        } else if !config
            .presets
            .iter()
            .any(|candidate| candidate.id == preset)
        {
            Some("Pick a review preset.")
        } else if preset == CUSTOM_PRESET && instructions.is_empty() {
            Some("Write what the custom review should look for.")
        } else {
            None
        };
        if let Some(problem) = problem {
            self.code_review
                .errors
                .insert(workspace, problem.to_string());
            return;
        }
        let Some(provider) = provider else {
            return;
        };
        let instructions = (!instructions.is_empty()).then_some(instructions);
        let result = match &self.connection {
            Some(connection) => connection.start_review(
                self.generation,
                workspace,
                Some(preset),
                self.code_review.tier,
                Some(provider),
                instructions,
            ),
            None => Err("The client worker is still starting.".to_string()),
        };
        match result {
            Ok(()) => {
                self.code_review.starting = true;
                self.code_review.errors.remove(&workspace);
            }
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
    }

    pub fn cancel_code_review(&mut self) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let Some(run) = self.code_review.latest.get(&workspace) else {
            return;
        };
        let result = match &self.connection {
            Some(connection) => connection.cancel_review(run.id),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            self.code_review.errors.insert(workspace, error);
        }
    }

    pub fn apply_review_run(&mut self, run: &ReviewRun) {
        if supersedes(self.code_review.latest.get(&run.workspace_id), run) {
            self.code_review
                .latest
                .insert(run.workspace_id, run.clone());
        }
        self.code_review.starting = false;
        if self.code_review.workspace == Some(run.workspace_id) {
            self.code_review.errors.remove(&run.workspace_id);
        }
    }

    pub fn apply_review_config(
        &mut self,
        workspace: WorkspaceId,
        result: Result<ReviewSettings, String>,
    ) -> bool {
        match result {
            Ok(config) => {
                let current = self.code_review.workspace == Some(workspace);
                let remembered = self
                    .code_review
                    .preset
                    .as_deref()
                    .filter(|id| config.presets.iter().any(|preset| preset.id == *id));
                if current && remembered.is_none() {
                    self.code_review.preset = Some(config.default_preset.clone());
                }
                let offered: Vec<(AgentProviderId, String)> = self
                    .store
                    .as_ref()
                    .map(|store| reviewers(&store.providers))
                    .unwrap_or_default();
                if current
                    && !self
                        .code_review
                        .provider
                        .as_deref()
                        .is_some_and(|id| offered.iter().any(|(known, _)| known.as_str() == id))
                {
                    self.code_review.provider = offered.first().map(|(id, _)| id.to_string());
                }
                self.code_review.configs.insert(workspace, config);
                self.code_review.errors.remove(&workspace);
            }
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
        true
    }

    pub fn apply_reviews(
        &mut self,
        workspace: WorkspaceId,
        result: Result<Vec<ReviewRun>, String>,
    ) -> bool {
        match result {
            Ok(runs) => {
                if let Some(newest) = runs.first() {
                    self.apply_review_run(newest);
                }
            }
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
        true
    }

    pub fn apply_review_report(
        &mut self,
        workspace: WorkspaceId,
        result: Result<Box<(ReviewRun, Vec<Finding>)>, String>,
    ) -> bool {
        match result {
            Ok(pair) => {
                let (run, findings) = *pair;
                self.code_review.reports.insert(
                    workspace,
                    Report {
                        run: run.id,
                        findings,
                    },
                );
                self.apply_review_run(&run);
                self.code_review.errors.remove(&workspace);
            }
            Err(error) => {
                self.code_review.report_asked.clear();
                self.code_review.errors.insert(workspace, error);
            }
        }
        true
    }

    pub fn review_started(
        &mut self,
        workspace: WorkspaceId,
        result: Result<ReviewId, String>,
    ) -> bool {
        self.code_review.starting = false;
        match result {
            Ok(_) => {
                self.code_review.errors.remove(&workspace);
            }
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
        true
    }

    pub fn review_command_result(&mut self, label: &str, ok: bool, message: &str) {
        if ok {
            return;
        }
        if !matches!(label, "StartReview" | "CancelReview" | "GetReview") {
            return;
        }
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        self.code_review.starting = false;
        self.code_review
            .errors
            .insert(workspace, message.to_string());
    }
}

fn section(k: &Kit, title: impl Into<String>) -> El {
    text(title.into())
        .font(k.ty.small_medium)
        .color(k.c.text_muted)
        .pt(6.0)
}

fn note(k: &Kit, message: impl Into<String>) -> El {
    text(message.into())
        .font(k.ty.text_xsmall)
        .color(k.c.text_faint)
        .wrap()
}

fn setup(s: &AppState, k: &Kit) -> El {
    let Some(workspace) = s.code_review.workspace else {
        return col();
    };
    let config = s.code_review.configs.get(&workspace);
    let offered: Vec<(AgentProviderId, String)> = s
        .store
        .as_ref()
        .map(|store| reviewers(&store.providers))
        .unwrap_or_default();
    let mut body = col().key("review-setup").w_full().gap(8.0);
    if offered.is_empty() {
        body = body.child(note(k, "No installed agent can run a Forge Review."));
    }
    if let Some(config) = config {
        let mut presets = RadioGroup::new().label("What to ask for");
        for preset in &config.presets {
            let id = preset.id.clone();
            let chosen = s.code_review.preset.as_deref() == Some(preset.id.as_str())
                || s.code_review.preset.is_none() && preset.id == config.default_preset;
            let detail = (!preset.detail.is_empty()).then(|| preset.detail.clone());
            let mut item = RadioItem::new(preset.name.clone(), chosen);
            if let Some(detail) = detail {
                item = item.description(detail);
            }
            presets = presets.item(item.on_select(move |s: &mut AppState| {
                s.code_review.preset = Some(id.clone());
            }));
        }
        body = body.child(presets.build(k).key("review-presets"));
        body = body.child(
            SegmentedControl::new(
                &["Default tier", "Base", "Plus", "Apex"],
                tier_index(s.code_review.tier),
            )
            .label("Tier")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                s.code_review.tier = match index {
                    1 => Some(Tier::Base),
                    2 => Some(Tier::Plus),
                    3 => Some(Tier::Apex),
                    _ => None,
                };
            })
            .build(k)
            .key("review-tier"),
        );
        let mut agents = RadioGroup::new().label("Agent");
        for (id, name) in &offered {
            let chosen = s.code_review.provider.as_deref() == Some(id.as_str());
            let id = id.to_string();
            agents = agents.item(RadioItem::new(name.clone(), chosen).on_select(
                move |s: &mut AppState| {
                    s.code_review.provider = Some(id.clone());
                },
            ));
        }
        body = body.child(agents.build(k).key("review-agents"));
        if !config.errors.is_empty() {
            for error in &config.errors {
                body = body.child(
                    text(error.clone())
                        .font(k.ty.text_xsmall)
                        .color(k.c.warning.fg)
                        .wrap(),
                );
            }
        }
        if !config.rules.is_empty() {
            let enabled = config.rules.iter().filter(|rule| rule.enabled).count();
            body = body.child(section(
                k,
                format!("{} rules · {enabled} enabled", config.rules.len()),
            ));
            let mut rules = col().key("review-rules").gap(1.0);
            for rule in &config.rules {
                rules = rules.child(
                    row()
                        .w_full()
                        .gap(6.0)
                        .child(
                            text(format!("{:?}", rule.severity))
                                .font(k.ty.text_xsmall)
                                .color(severity_ink(k, rule.severity)),
                        )
                        .child(
                            text(rule.title.clone())
                                .font(k.ty.text_xsmall)
                                .color(if rule.enabled {
                                    k.c.text_base
                                } else {
                                    k.c.text_faint
                                })
                                .truncate()
                                .min_w(0.0),
                        ),
                );
            }
            body = body.child(rules);
        }
    } else {
        body = body.child(note(k, "Reading review settings…"));
    }
    body = body.child(
        TextArea::new(&s.code_review.instructions)
            .label("Extra instructions")
            .placeholder("What should the agent look for?")
            .on_edit(|s: &mut AppState| &mut s.code_review.instructions)
            .build(k)
            .key("review-instructions")
            .h(80.0),
    );
    let custom = s.code_review.preset.as_deref() == Some(CUSTOM_PRESET);
    let ready = config.is_some()
        && !offered.is_empty()
        && !s.code_review.starting
        && (!custom || !s.code_review.instructions.text.trim().is_empty());
    body = body.child(
        row().w_full().gap(8.0).child(el().grow(1.0)).child(
            Button::new(if s.code_review.starting {
                "Starting…"
            } else {
                "Review changes"
            })
            .variant(ButtonVariant::Contrast)
            .disabled(!ready)
            .on_press(|s: &mut AppState| s.start_code_review())
            .build(k)
            .key("review-start"),
        ),
    );
    body
}

fn tier_index(tier: Option<Tier>) -> Option<usize> {
    match tier {
        None => Some(0),
        Some(Tier::Base) => Some(1),
        Some(Tier::Plus) => Some(2),
        Some(Tier::Apex) => Some(3),
    }
}

fn run_header(k: &Kit, run: &ReviewRun, head: Option<&str>) -> El {
    let mut head_row = row()
        .key("review-run-head")
        .w_full()
        .gap(10.0)
        .items_center();
    match run.verdict {
        Some(verdict) => {
            head_row = head_row.child(
                text(verdict_label(verdict))
                    .font(k.ty.small_medium)
                    .color(verdict_ink(k, verdict)),
            );
        }
        None => {
            head_row = head_row.child(
                text(status_label(run.status))
                    .font(k.ty.small_medium)
                    .color(if running(run) {
                        k.c.warning.fg
                    } else {
                        k.c.text_muted
                    }),
            );
        }
    }
    head_row = head_row
        .child(
            text(score_label(run.score))
                .font(k.ty.small)
                .color(k.c.text_base),
        )
        .child(
            text(tier_label(run.tier))
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        )
        .child(
            text(run.preset.clone())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        )
        .child(
            text(run.head_oid.chars().take(7).collect::<String>())
                .font(k.ty.mono)
                .color(k.c.text_faint),
        )
        .child(
            text(run.provider.clone())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    if is_stale(run, head) {
        head_row = head_row.child(
            text("stale")
                .key("review-stale")
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg),
        );
    }
    head_row = head_row.child(el().grow(1.0)).child(
        text(duration(
            run.created_at,
            run.finished_at,
            Timestamp::now().as_offset().unix_timestamp(),
        ))
        .font(k.ty.text_xsmall)
        .color(k.c.text_faint),
    );
    head_row
}

fn findings(s: &AppState, k: &Kit, run: &ReviewRun) -> El {
    let workspace = run.workspace_id;
    let Some(report) = s.code_review.reports.get(&workspace) else {
        return col();
    };
    if report.run != run.id {
        return col();
    }
    let groups = group_findings(&report.findings, s.code_review.grouping);
    let mut body = col().key("review-findings").w_full().gap(8.0).child(
        row()
            .w_full()
            .items_center()
            .child(section(
                k,
                format!(
                    "{} finding{}",
                    report.findings.len(),
                    if report.findings.len() == 1 { "" } else { "s" }
                ),
            ))
            .child(el().grow(1.0))
            .child(
                Button::new(match s.code_review.grouping {
                    Grouping::Severity => "By severity",
                    Grouping::File => "By file",
                })
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| {
                    s.code_review.grouping = match s.code_review.grouping {
                        Grouping::Severity => Grouping::File,
                        Grouping::File => Grouping::Severity,
                    };
                })
                .build(k)
                .key("review-grouping"),
            ),
    );
    for (key, group) in groups {
        body = body.child(section(k, key));
        for finding in group {
            let line = finding.line;
            let path = finding.path.clone();
            let open = Button::new(match line {
                Some(line) => format!("{path}:{line}"),
                None => path.clone(),
            })
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| {
                if let Some(connection) = &s.connection {
                    let _ = connection.open_editor_at(workspace, path.clone(), line.unwrap_or(1));
                }
                s.workbench.show(CenterMode::Session);
            })
            .build(k)
            .key(format!("review-finding-{}", finding.id));
            let mut item = col()
                .key(format!("review-finding-row-{}", finding.id))
                .w_full()
                .gap(4.0)
                .padding(8.0)
                .radius(5.0)
                .fill(k.c.surface_hover)
                .child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .items_center()
                        .child(
                            text(format!("{:?}", finding.severity))
                                .font(k.ty.small_medium)
                                .color(severity_ink(k, finding.severity)),
                        )
                        .child(
                            text(finding.category.clone())
                                .font(k.ty.text_xsmall)
                                .color(k.c.text_faint),
                        )
                        .children(
                            finding
                                .rule_id
                                .as_ref()
                                .map(|rule| note(k, format!("rule: {rule}"))),
                        )
                        .child(
                            text(format!("{:.0}%", finding.confidence * 100.0))
                                .font(k.ty.text_xsmall)
                                .color(k.c.text_faint),
                        )
                        .child(
                            text(finding.title.clone())
                                .font(k.ty.small_medium)
                                .color(k.c.text_base)
                                .truncate()
                                .min_w(0.0)
                                .grow(1.0),
                        )
                        .child(open),
                )
                .child(
                    text(finding.body.clone())
                        .font(k.ty.small)
                        .color(k.c.text_muted)
                        .wrap(),
                );
            if let Some(suggestion) = &finding.suggestion {
                item = item.child(
                    col()
                        .w_full()
                        .padding(8.0)
                        .radius(4.0)
                        .fill(k.c.bg_deep)
                        .child(
                            text(suggestion.clone())
                                .font(k.ty.mono)
                                .color(k.c.text_base)
                                .wrap(),
                        ),
                );
            }
            body = body.child(item);
        }
    }
    body
}

fn coverage(k: &Kit, run: &ReviewRun) -> El {
    let coverage = &run.coverage;
    let mut body = col()
        .key("review-coverage")
        .w_full()
        .gap(4.0)
        .child(section(k, "Coverage"))
        .child(note(
            k,
            format!(
                "{} included · {} excluded · {} truncated · {} redacted",
                coverage.included.len(),
                coverage.excluded.len(),
                coverage.truncated.len(),
                coverage.redactions
            ),
        ));
    for note_text in &coverage.notes {
        body = body.child(note(k, note_text.clone()));
    }
    body
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let Some(workspace) = s.code_review.workspace else {
        return col()
            .key("review")
            .w_full()
            .h_full()
            .min_h(0.0)
            .padding(24.0)
            .child(
                text("Open a checkout to review its changes.")
                    .font(k.ty.small)
                    .color(k.c.text_faint),
            );
    };
    let store = s.store.as_ref();
    let workspace_row = store.and_then(|store| {
        store
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
    });
    let head = workspace_row.and_then(|row| row.status.head.as_deref());
    let run = s.code_review.latest.get(&workspace);
    let error = s.code_review.errors.get(&workspace);
    let mut top = row()
        .key("review-head")
        .w_full()
        .h(38.0)
        .px(12.0)
        .gap(10.0)
        .items_center()
        .child(text("Review").font(k.ty.small_medium).color(k.c.text_base));
    if let Some(workspace_row) = workspace_row {
        top = top.child(
            text(workspace_row.label())
                .font(k.ty.mono)
                .color(k.c.text_muted)
                .truncate()
                .min_w(0.0),
        );
    }
    top = top.child(el().grow(1.0)).child(
        Button::new("Refresh")
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut AppState| refresh(s))
            .build(k)
            .key("review-refresh"),
    );
    let mut body = col().key("review-body").w_full().gap(10.0);
    if let Some(error) = error {
        body = body.child(
            text(error.clone())
                .key("review-error")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.danger.fg)
                .wrap(),
        );
    }
    body = body.child(setup(s, k));
    match run {
        Some(run) => {
            body = body.child(run_header(k, run, head));
            if running(run) {
                let mut progress = col().key("review-progress").w_full().gap(6.0).child(
                    Progress::new(Some(stage_progress(run.stage) / 100.0))
                        .label(stage_label(run.stage))
                        .show_value()
                        .build(k),
                );
                let mut actions = row().gap(6.0);
                if let Some(session) = run.session_id {
                    actions = actions.child(
                        Button::new("Show the reviewer session")
                            .variant(ButtonVariant::GhostMuted)
                            .control_size(ControlSize::Small)
                            .on_press(move |s: &mut AppState| s.focus_session(session))
                            .build(k)
                            .key("review-session"),
                    );
                }
                actions = actions.child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut AppState| s.cancel_code_review())
                        .build(k)
                        .key("review-cancel"),
                );
                progress = progress.child(actions);
                body = body.child(progress);
            }
            if let Some(error) = &run.error {
                body = body.child(
                    text(error.clone())
                        .font(k.ty.small)
                        .color(k.c.danger.fg)
                        .wrap(),
                );
            }
            if !run.summary.trim().is_empty() {
                body = body.child(section(k, "Summary")).child(
                    Markdown::new(run.summary.clone())
                        .build(k)
                        .key("review-summary"),
                );
            }
            body = body.child(findings(s, k, run));
            body = body.child(coverage(k, run));
        }
        None => {
            body = body.child(
                text("No review of this workspace yet.")
                    .key("review-empty")
                    .font(k.ty.small)
                    .color(k.c.text_faint),
            );
        }
    }
    col()
        .key("review")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(top)
        .child(
            col()
                .key("review-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(12.0)
                .py(6.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::provider;

    fn finding(id: u32, path: &str, line: Option<u32>, severity: Severity) -> Finding {
        Finding {
            id,
            fingerprint: format!("fingerprint-{id}"),
            path: path.into(),
            line,
            end_line: None,
            severity,
            category: "bug".into(),
            rule_id: None,
            confidence: 0.75,
            title: format!("finding {id}"),
            body: "body".into(),
            evidence: "evidence".into(),
            suggestion: None,
        }
    }

    fn run(id: ReviewId, stage: Option<ReviewStage>, finished: bool) -> ReviewRun {
        ReviewRun {
            id,
            workspace_id: WorkspaceId::new(),
            target: domain::ReviewTarget::Workspace {
                workspace_id: WorkspaceId::new(),
                base: None,
            },
            head_oid: "abcdef123".into(),
            base_oid: None,
            tier: Tier::Base,
            preset: "standard".into(),
            provider: "claude".into(),
            status: if finished {
                ReviewStatus::Completed
            } else {
                ReviewStatus::Running
            },
            stage,
            summary: String::new(),
            score: None,
            verdict: None,
            error: None,
            session_id: None,
            coverage: domain::Coverage::default(),
            created_at: Timestamp::now(),
            finished_at: finished.then(Timestamp::now),
        }
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
        let p2: Vec<u32> = groups[2].1.iter().map(|finding| finding.id).collect();
        assert_eq!(p2, vec![4, 1], "path, then a whole-file finding first");
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
        let a: Vec<u32> = groups[0].1.iter().map(|finding| finding.id).collect();
        assert_eq!(a, vec![4, 2]);
        let b: Vec<u32> = groups[1].1.iter().map(|finding| finding.id).collect();
        assert_eq!(b, vec![3, 1]);
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
    fn stages_and_durations_are_human_readable() {
        assert_eq!(stage_label(None), "Queued");
        assert_eq!(
            stage_label(Some(ReviewStage::Model)),
            "Waiting for the reviewer"
        );
        let progress: Vec<f32> = [None, Some(ReviewStage::Diff), Some(ReviewStage::Pack)]
            .into_iter()
            .map(stage_progress)
            .collect();
        assert_eq!(progress[0], 0.0);
        assert!(progress[1] < progress[2]);
        assert_eq!(score_label(Some(3.5)), "3.5 / 5");
        assert_eq!(score_label(None), "—");
        let start = Timestamp::parse_rfc3339("2026-11-02T10:00:00Z").unwrap();
        let end = Timestamp::parse_rfc3339("2026-11-02T10:03:05Z").unwrap();
        assert_eq!(duration(start, Some(end), 0), "3m 05s");
        assert_eq!(
            duration(start, None, start.as_offset().unix_timestamp() + 42),
            "42s"
        );
        assert_eq!(verdict_label(Verdict::Block), "Bloquea");
    }

    #[test]
    fn the_review_view_renders_the_setup_and_the_empty_state() {
        use crate::connection::{Metrics, fixture_store};
        use crate::terminal::SharedTerminal;
        use rune::{Env, Runtime};
        use std::sync::Arc;

        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let workspace = store.workspaces[0].id;
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        open(&mut state, workspace);
        assert!(
            state.apply_review_config(workspace, Ok(crate::connection::fixture_review_config()))
        );
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys.iter().any(|key| key == "review-presets"), "{keys:?}");
        assert!(keys.iter().any(|key| key == "review-start"));
        assert!(keys.iter().any(|key| key == "review-empty"));
    }

    #[test]
    fn only_structured_review_providers_are_offered() {
        let style = || domain::StructuredReviewStyle {
            session_id_flag: "--session-id".into(),
            add_dir_flag: "--add-dir".into(),
        };
        let mut supported = provider("claude", true, true, true);
        supported.descriptor.structured_review = Some(style());
        let mut wrong_mode = provider("cursor", true, true, true);
        wrong_mode.descriptor.structured_review = None;
        let mut missing = provider("gone", false, true, true);
        missing.descriptor.structured_review = Some(style());
        let offered: Vec<String> = reviewers(&[supported, wrong_mode, missing])
            .into_iter()
            .map(|(id, _)| id.to_string())
            .collect();
        assert_eq!(offered, ["claude"]);
    }
}
