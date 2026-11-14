//! Forge Review: one workspace's structured review,
//! its presets and its findings. Starting and cancelling are daemon requests;
//! progress arrives as `ReviewUpdated` and findings are read once complete.

use std::collections::{HashMap, HashSet};

use client::ProviderInfo;
use domain::{
    AgentProviderId, Finding, FindingReaction, Lineage, ReviewId, ReviewRun, ReviewSettings,
    ReviewStage, ReviewStatus, ReviewSuppression, ReviewTarget, Session, SessionId, SessionKind,
    Severity, Tier, Timestamp, Verdict, WorkspaceId,
};
use rune::{El, ScrollSpec, TextEdit, col, el, row, text};
use rune_kit::diff::{Annotation, DiffState, DiffView, LineRef, Side};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Kit, Markdown, Progress, RadioGroup, RadioItem,
    SegmentedControl, TextArea,
};

use crate::app::AppState;
use crate::workbench::CenterMode;

pub const CUSTOM_PRESET: &str = "custom";

/// What the daemon accepts in one `SendSessionPrompt`.
pub const PROMPT_BUDGET: usize = 8_192;

const PROMPT_HEAD: &str = "Forge Review found these issues in your changes. Fix each one, or say \
                           why it is not a problem:\n\n";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    Severity,
    File,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewView {
    #[default]
    Findings,
    Diff,
}

/// One of a checkout's review slots: the daemon runs a workspace review and
/// each pull request's side by side, so neither replaces the other here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lane {
    Workspace,
    PullRequest(u32),
}

#[must_use]
pub fn lane(target: &ReviewTarget) -> Lane {
    match target {
        ReviewTarget::PullRequest { number, .. } => Lane::PullRequest(*number),
        _ => Lane::Workspace,
    }
}

type Slot = (WorkspaceId, Lane);

pub struct Report {
    pub run: ReviewId,
    pub findings: Vec<Finding>,
    pub resolved: Vec<Finding>,
    pub previous: Option<ReviewId>,
}

/// The range one run reviewed, as the Diff tab draws it.
pub struct ReviewedDiff {
    pub run: ReviewId,
    /// The daemon's path for each of `CodeReview::diff_state`'s files, in order.
    pub paths: Vec<String>,
    pub loaded: bool,
    pub error: Option<String>,
    /// The `CodeReview::revision` its annotations were built from.
    annotated: Option<u64>,
}

/// What the Changes view's annotations were last built from.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ChangesKey {
    review: Option<ReviewId>,
    selected: Option<usize>,
    files: u64,
    revision: u64,
}

pub struct CodeReview {
    pub workspace: Option<WorkspaceId>,
    pub latest: HashMap<Slot, ReviewRun>,
    pub reports: HashMap<Slot, Report>,
    /// The lane each checkout's Review tab shows; a run of another lane
    /// landing never switches it.
    pub shown: HashMap<WorkspaceId, Lane>,
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
    pub view: ReviewView,
    pub show_resolved: bool,
    pub diff: Option<ReviewedDiff>,
    pub diff_state: DiffState,
    /// A pull-request finding's `(path, line)` to scroll the Diff tab to once
    /// the reviewed diff holds it: the checkout's own file is not that head.
    pub reveal: Option<(String, u32)>,
    /// Bumped whenever a report or a reaction changes, so annotations are
    /// rebuilt then and never per frame.
    pub revision: u64,
    changes_key: Option<ChangesKey>,
    /// Each reviewed workspace's `send_target`, recomputed when the sessions or
    /// the runs change, never per frame.
    pub send_to: HashMap<WorkspaceId, SessionId>,
}

impl Default for CodeReview {
    fn default() -> Self {
        Self {
            workspace: None,
            latest: HashMap::new(),
            reports: HashMap::new(),
            shown: HashMap::new(),
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
            view: ReviewView::Findings,
            show_resolved: false,
            diff: None,
            diff_state: DiffState::default(),
            reveal: None,
            revision: 0,
            changes_key: None,
            send_to: HashMap::new(),
        }
    }
}

impl CodeReview {
    #[must_use]
    pub fn shown_lane(&self, workspace: WorkspaceId) -> Lane {
        self.shown
            .get(&workspace)
            .copied()
            .unwrap_or(Lane::Workspace)
    }

    #[must_use]
    pub fn shown_run(&self, workspace: WorkspaceId) -> Option<&ReviewRun> {
        self.latest.get(&(workspace, self.shown_lane(workspace)))
    }

    /// The shown run's report, never an older run's of the same lane.
    #[must_use]
    pub fn shown_report(&self, workspace: WorkspaceId) -> Option<&Report> {
        let run = self.shown_run(workspace)?;
        self.reports
            .get(&(workspace, lane(&run.target)))
            .filter(|report| report.run == run.id)
    }

    /// The workspace review's report the Changes view annotates with.
    #[must_use]
    pub fn workspace_report(&self, workspace: WorkspaceId) -> Option<&Report> {
        self.reports.get(&(workspace, Lane::Workspace))
    }

    fn lanes(&self, workspace: WorkspaceId) -> Vec<Lane> {
        let mut lanes: Vec<Lane> = self
            .latest
            .keys()
            .filter(|(owner, _)| *owner == workspace)
            .map(|(_, lane)| *lane)
            .collect();
        lanes.sort_unstable();
        lanes
    }

    fn knows(&self, workspace: WorkspaceId) -> bool {
        self.latest.keys().any(|(owner, _)| *owner == workspace)
    }
}

const SEVERITIES: [Severity; 3] = [Severity::P0, Severity::P1, Severity::P2];
const STAGES: [ReviewStage; 4] = [
    ReviewStage::Diff,
    ReviewStage::Pack,
    ReviewStage::Model,
    ReviewStage::Validate,
];

/// Findings grouped: severity groups are most
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

/// An unmeasured head is not evidence the checkout moved, and a pull
/// request's head is not the checkout's.
#[must_use]
pub fn is_stale(run: &ReviewRun, head: Option<&str>) -> bool {
    matches!(run.target, ReviewTarget::Workspace { .. })
        && head.is_some_and(|head| head != run.head_oid)
}

#[must_use]
pub fn is_pull_request(run: &ReviewRun) -> bool {
    matches!(run.target, ReviewTarget::PullRequest { .. })
}

#[must_use]
pub fn target_label(target: &ReviewTarget) -> String {
    lane_label(lane(target))
}

#[must_use]
pub fn lane_label(lane: Lane) -> String {
    match lane {
        Lane::PullRequest(number) => format!("PR #{number}"),
        Lane::Workspace => "Workspace".into(),
    }
}

/// Whether the Changes view, which shows the live working tree, should carry
/// `run`'s findings: a completed workspace review of the head it is on.
#[must_use]
pub fn annotates_changes(run: &ReviewRun, head: Option<&str>) -> bool {
    run.status == ReviewStatus::Completed
        && matches!(run.target, ReviewTarget::Workspace { .. })
        && !is_stale(run, head)
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

/// The workspace's live agent that was active last, never a reviewer or a
/// background one.
#[must_use]
pub fn send_target(
    sessions: &[Session],
    workspace: WorkspaceId,
    reviewers: &[SessionId],
) -> Option<SessionId> {
    sessions
        .iter()
        .filter(|session| {
            session.workspace_id == workspace
                && session.kind == SessionKind::Agent
                && session.state.is_active()
                && !session.background
                && !reviewers.contains(&session.id)
        })
        .max_by_key(|session| session.last_activity_at)
        .map(|session| session.id)
}

/// Whole findings, in order, within `budget` bytes, and how many did not fit.
/// Empty when none fits or none is given; [`empty_prompt_reason`] tells which.
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

#[must_use]
pub fn empty_prompt_reason(left_out: usize) -> &'static str {
    if left_out == 0 {
        "No finding is left to send: none was reported, or every one is dismissed."
    } else {
        "The findings are longer than one prompt can carry."
    }
}

#[must_use]
pub fn delivery_message(queued: bool, left_out: usize) -> String {
    let mut message = if queued {
        "Queued for the agent's next idle".to_string()
    } else {
        "Sent to the agent".to_string()
    };
    if left_out > 0 {
        message.push_str(&format!(" · {left_out} finding(s) left out to fit 8 KiB"));
    }
    message
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
    s.code_review.diff = None;
    sync(s);
}

/// Replies asked on the old connection are dropped as stale, so what was
/// waiting on one is asked again on the next sync.
pub fn reconnected(s: &mut AppState) {
    s.code_review.report_asked.clear();
    if s.code_review.diff.as_ref().is_some_and(|diff| !diff.loaded) {
        s.code_review.diff = None;
    }
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
    if !s.code_review.knows(workspace)
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
    let shown = s.code_review.shown_lane(workspace);
    let mut lanes = vec![shown];
    if shown != Lane::Workspace {
        lanes.push(Lane::Workspace);
    }
    for lane in lanes {
        let Some(run) = s.code_review.latest.get(&(workspace, lane)) else {
            continue;
        };
        let (run, completed) = (run.id, run.status == ReviewStatus::Completed);
        let read = s
            .code_review
            .reports
            .get(&(workspace, lane))
            .is_some_and(|report| report.run == run);
        if !completed || read || !s.code_review.report_asked.insert(run) {
            continue;
        }
        let result = match &s.connection {
            Some(connection) => connection.get_review(generation, workspace, run),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            s.code_review.report_asked.remove(&run);
            s.code_review.errors.insert(workspace, error);
        }
    }
    if s.code_review.view == ReviewView::Diff
        && let Some(run) = s.code_review.shown_run(workspace)
        && run.status == ReviewStatus::Completed
        && s.code_review
            .diff
            .as_ref()
            .is_none_or(|diff| diff.run != run.id)
    {
        let run = run.id;
        s.code_review.diff_state = DiffState::default();
        s.code_review.diff = Some(ReviewedDiff {
            run,
            paths: Vec::new(),
            error: None,
            loaded: false,
            annotated: None,
        });
        let result = match &s.connection {
            Some(connection) => connection.get_review_diff(generation, run),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result
            && let Some(diff) = s.code_review.diff.as_mut()
        {
            diff.error = Some(error);
        }
    }
}

/// Rebuilds the Review tab's and the Changes view's annotations when their
/// inputs moved; every check here is a comparison of copied values.
fn sync_annotations(s: &mut AppState) {
    let revision = s.code_review.revision;
    if let Some(diff) = s.code_review.diff.as_mut()
        && diff.loaded
        && diff.annotated != Some(revision)
        && let Some(report) = s
            .code_review
            .reports
            .values()
            .find(|report| report.run == diff.run)
    {
        let paths: Vec<&str> = diff.paths.iter().map(String::as_str).collect();
        s.code_review
            .diff_state
            .set_annotations(annotations(&report.findings, &paths));
        diff.annotated = Some(revision);
    }

    let changes = &s.workbench.changes;
    let review = changes.workspace.and_then(|workspace| {
        let run = s.code_review.latest.get(&(workspace, Lane::Workspace))?;
        let head = s.store.as_ref().and_then(|store| {
            store
                .workspaces
                .iter()
                .find(|candidate| candidate.id == workspace)
                .and_then(|row| row.status.head.as_deref())
        });
        let report = s.code_review.workspace_report(workspace)?;
        (annotates_changes(run, head) && report.run == run.id).then_some(run.id)
    });
    let key = ChangesKey {
        review,
        selected: changes.selected,
        files: changes.key_revision,
        revision,
    };
    if s.code_review.changes_key == Some(key) {
        return;
    }
    s.code_review.changes_key = Some(key);
    let selected = changes
        .selected
        .and_then(|index| changes.files.get(index))
        .map(|file| file.path.clone());
    let built = match (review, selected) {
        (Some(review), Some(path)) => s
            .code_review
            .reports
            .values()
            .find(|report| report.run == review)
            .map(|report| {
                let paths = vec![path.as_str(); s.workbench.changes.state.files().len()];
                annotations(&report.findings, &paths)
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let state = &mut s.workbench.changes.state;
    if !(built.is_empty() && state.annotations().is_empty()) {
        state.set_annotations(built);
    }
}

impl AppState {
    pub fn sync_review(&mut self) {
        sync_annotations(self);
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
                ReviewTarget::Workspace {
                    workspace_id: workspace,
                    base: None,
                },
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
                self.code_review.shown.insert(workspace, Lane::Workspace);
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
        let Some(run) = self.code_review.shown_run(workspace) else {
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
        let slot = (run.workspace_id, lane(&run.target));
        if supersedes(self.code_review.latest.get(&slot), run) {
            self.code_review.latest.insert(slot, run.clone());
            self.code_review.shown.entry(slot.0).or_insert(slot.1);
            self.refresh_send_targets();
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
                for run in &runs {
                    self.apply_review_run(run);
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
        result: Result<Box<client::ReviewReport>, String>,
    ) -> bool {
        match result {
            Ok(report) => {
                let client::ReviewReport {
                    run,
                    findings,
                    resolved,
                    previous,
                } = *report;
                self.code_review.reports.insert(
                    (workspace, lane(&run.target)),
                    Report {
                        run: run.id,
                        findings,
                        resolved,
                        previous,
                    },
                );
                self.code_review.revision += 1;
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
        if matches!(label, "SetFindingFeedback" | "ClearReviewSuppression") {
            self.review_feedback_settled(ok);
        }
        if label == "SendSessionPrompt" && ok {
            self.toasts.success(message.to_string(), None);
        }
        if ok || !matches!(label, "StartReview" | "CancelReview" | "GetReview") {
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

    /// A refused reaction re-reads the report the optimistic edit changed; any
    /// answer re-reads the settings, whose suppressions it may have moved.
    fn review_feedback_settled(&mut self, ok: bool) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        if !ok {
            let lane = self.code_review.shown_lane(workspace);
            self.code_review.reports.remove(&(workspace, lane));
            self.code_review.report_asked.clear();
        }
        if let Some(connection) = &self.connection
            && let Err(error) = connection.review_config(self.generation, workspace)
        {
            self.code_review.errors.insert(workspace, error);
        }
        sync(self);
    }

    pub fn apply_review_diff(
        &mut self,
        review_id: ReviewId,
        result: Result<domain::WorkspaceDiff, String>,
    ) -> bool {
        let split = self.diff_split;
        let Some(diff) = self
            .code_review
            .diff
            .as_mut()
            .filter(|diff| diff.run == review_id)
        else {
            return false;
        };
        diff.loaded = true;
        match result {
            Ok(reviewed) => {
                diff.error = None;
                let mut files = Vec::new();
                for file in &reviewed.files {
                    for parsed in rune_kit::diff::parse(&file.patch) {
                        diff.paths.push(file.path.clone());
                        files.push(parsed);
                    }
                }
                self.code_review.diff_state.set_files(files);
                self.code_review.diff_state.set_mode(if split {
                    rune_kit::diff::DiffMode::Split
                } else {
                    rune_kit::diff::DiffMode::Unified
                });
                diff.annotated = None;
            }
            Err(error) => diff.error = Some(error),
        }
        true
    }

    /// Scrolls the Diff tab to a pull-request finding once the reviewed diff
    /// has loaded; dropped when the tab is left or the file is not in it.
    pub fn settle_review_reveal(&mut self, tx: &mut rune::Tx) -> bool {
        let review = &mut self.code_review;
        let Some((path, line)) = &review.reveal else {
            return false;
        };
        if self.workbench.mode != CenterMode::Review || review.view != ReviewView::Diff {
            review.reveal = None;
            return false;
        }
        let Some(diff) = review.diff.as_ref().filter(|diff| diff.loaded) else {
            return false;
        };
        let file = diff.paths.iter().position(|known| known == path);
        if let Some(file) = file {
            let at = LineRef::new(Side::New, *line);
            review.diff_state.scroll_to_line(file, at, tx);
        }
        review.reveal = None;
        file.is_some()
    }

    /// Toggles `reaction` on one finding of the report on screen: pressing the
    /// active reaction clears it.
    pub fn react_to_finding(&mut self, finding_id: u32, reaction: FindingReaction) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let Some(run) = self.code_review.shown_run(workspace).map(|run| run.id) else {
            return;
        };
        let lane = self.code_review.shown_lane(workspace);
        let Some(report) = self
            .code_review
            .reports
            .get_mut(&(workspace, lane))
            .filter(|report| report.run == run)
        else {
            return;
        };
        let Some(finding) = report.findings.iter_mut().find(|f| f.id == finding_id) else {
            return;
        };
        let next = (finding.reaction != Some(reaction)).then_some(reaction);
        finding.reaction = next;
        let review = report.run;
        self.code_review.revision += 1;
        let result = match &self.connection {
            Some(connection) => connection.set_finding_feedback(review, finding_id, next),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            self.code_review.errors.insert(workspace, error);
        }
    }

    pub fn clear_review_suppression(&mut self, suppression: ReviewSuppression) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        if let Some(config) = self.code_review.configs.get_mut(&workspace) {
            config.suppressions.retain(|known| *known != suppression);
        }
        let result = match &self.connection {
            Some(connection) => connection.clear_review_suppression(workspace, suppression),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            self.code_review.errors.insert(workspace, error);
        }
    }

    /// Why Send cannot run for the run on screen, or the session it reaches.
    fn send_destination(&self, run: &ReviewRun) -> Result<SessionId, &'static str> {
        if is_pull_request(run) {
            return Err("Send works on a workspace review, not a pull request's.");
        }
        self.code_review
            .send_to
            .get(&run.workspace_id)
            .copied()
            .ok_or("No running agent in this checkout to send to.")
    }

    pub(crate) fn refresh_send_targets(&mut self) {
        let reviewers: Vec<SessionId> = self
            .code_review
            .latest
            .values()
            .filter_map(|run| run.session_id)
            .collect();
        let sessions = self
            .store
            .as_ref()
            .map_or(&[][..], |store| store.sessions.as_slice());
        let workspaces: HashSet<WorkspaceId> = self
            .code_review
            .latest
            .keys()
            .map(|(workspace, _)| *workspace)
            .collect();
        self.code_review.send_to = workspaces
            .into_iter()
            .filter_map(|workspace| {
                send_target(sessions, workspace, &reviewers).map(|target| (workspace, target))
            })
            .collect();
    }

    /// Hands one finding, or every standing one when `only` is `None`, to the
    /// checkout's agent that was active last.
    pub fn send_findings(&mut self, only: Option<u32>) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let (Some(run), Some(report)) = (
            self.code_review.shown_run(workspace),
            self.code_review.shown_report(workspace),
        ) else {
            return;
        };
        let target = match self.send_destination(run) {
            Ok(target) => target,
            Err(reason) => {
                self.code_review.errors.insert(workspace, reason.into());
                return;
            }
        };
        let (text, left_out) = match only {
            Some(id) => {
                let one: Vec<&Finding> = report.findings.iter().filter(|f| f.id == id).collect();
                findings_prompt(&one, PROMPT_BUDGET)
            }
            None => standing_prompt(&report.findings),
        };
        if text.is_empty() {
            self.toasts
                .danger("Nothing was sent", empty_prompt_reason(left_out));
            return;
        }
        let result = match &self.connection {
            Some(connection) => connection.send_session_prompt(target, text, left_out),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            self.code_review.errors.insert(workspace, error);
        }
    }

    /// A new writable agent in the checkout, given every standing finding.
    pub fn fix_with_agent(&mut self) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let (Some(run), Some(report)) = (
            self.code_review.shown_run(workspace),
            self.code_review.shown_report(workspace),
        ) else {
            return;
        };
        if is_pull_request(run) {
            return;
        }
        let (prompt, left_out) = standing_prompt(&report.findings);
        if prompt.is_empty() {
            self.toasts
                .danger("No agent was started", empty_prompt_reason(left_out));
            return;
        }
        let (provider, profile) = match self.prompt_agent() {
            Ok(agent) => agent,
            Err(reason) => {
                self.code_review.errors.insert(workspace, reason);
                return;
            }
        };
        let Some(connection) = &self.connection else {
            return;
        };
        match connection.create_agent(workspace, provider, profile, Some(prompt), false) {
            Ok(()) => {
                if left_out > 0 {
                    self.toasts.neutral(
                        "Started an agent with the findings",
                        Some(format!("{left_out} finding(s) left out to fit 8 KiB")),
                    );
                }
                self.workbench.show(CenterMode::Session);
            }
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
    }

    /// Forge Review of a pull request's head, in the checkout `review_workspace`
    /// picks; the checkout itself is not moved.
    pub fn start_pull_request_review(&mut self, repository: &str, number: u32) {
        let Some(pr) = self.pull_request(repository, number) else {
            return;
        };
        let base = (!pr.base_ref.is_empty()).then(|| pr.base_ref.clone());
        let workspace = self.store.as_ref().and_then(|store| {
            crate::pr_review::review_workspace(
                &store.workspaces,
                pr.project_id,
                self.workbench.workspace,
            )
        });
        let Some(workspace) = workspace else {
            self.toasts.danger(
                "Forge Review did not start",
                "No checkout of this repository is open here.",
            );
            return;
        };
        let provider = self.code_review.provider.clone().or_else(|| {
            self.store.as_ref().and_then(|store| {
                reviewers(&store.providers)
                    .first()
                    .map(|(id, _)| id.to_string())
            })
        });
        let result = match &self.connection {
            Some(connection) => connection.start_review(
                self.generation,
                ReviewTarget::PullRequest {
                    workspace_id: workspace,
                    number,
                    base,
                },
                None,
                None,
                provider,
                None,
            ),
            None => Err("The client worker is still starting.".to_string()),
        };
        open(self, workspace);
        self.code_review
            .shown
            .insert(workspace, Lane::PullRequest(number));
        match result {
            Ok(()) => self.code_review.starting = true,
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
    }
}

fn section(k: &Kit, title: impl Into<String>) -> El {
    crate::ds::section_header(k, &title.into(), None, None)
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
    let mut body = col().key("review-setup").w_full().gap(crate::ds::SPACE_4);
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
                        .gap(crate::ds::SPACE_3)
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
        row()
            .w_full()
            .gap(crate::ds::SPACE_4)
            .child(el().grow(1.0))
            .child(
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
            text(target_label(&run.target))
                .key("review-target")
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted),
        )
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

fn reaction_button(
    k: &Kit,
    finding: &Finding,
    reaction: FindingReaction,
    label: &'static str,
    key: &str,
) -> El {
    let id = finding.id;
    let active = finding.reaction == Some(reaction);
    let help = match reaction {
        FindingReaction::Up => Some("Helpful"),
        FindingReaction::Down => Some("Not helpful"),
        FindingReaction::Dismiss => None,
    };
    let mut button = Button::new(label);
    if let Some(help) = help {
        button = button.help(help);
    }
    button
        .variant(if active {
            ButtonVariant::Contrast
        } else {
            ButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| s.react_to_finding(id, reaction))
        .build(k)
        .key(format!("{key}-{id}"))
}

fn lineage_badge(k: &Kit, finding: &Finding) -> El {
    let (label, ink) = match finding.lineage {
        Lineage::New => ("new", k.c.warning.fg),
        Lineage::Persisting => ("persisting", k.c.text_faint),
    };
    text(label)
        .key(format!("review-lineage-{}", finding.id))
        .font(k.ty.text_xsmall)
        .color(ink)
}

fn dismissed_row(k: &Kit, finding: &Finding) -> El {
    row()
        .key(format!("review-finding-row-{}", finding.id))
        .w_full()
        .gap(crate::ds::SPACE_4)
        .items_center()
        .padding(6.0)
        .child(
            text(format!("Dismissed · {}", finding.title))
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate()
                .min_w(0.0)
                .grow(1.0),
        )
        .child(reaction_button(
            k,
            finding,
            FindingReaction::Dismiss,
            "Undo",
            "review-undo",
        ))
}

fn finding_row(
    k: &Kit,
    run: &ReviewRun,
    finding: &Finding,
    lineage: bool,
    send: Option<&'static str>,
) -> El {
    if finding.reaction == Some(FindingReaction::Dismiss) {
        return dismissed_row(k, finding);
    }
    let line = finding.line;
    let path = finding.path.clone();
    let workspace = run.workspace_id;
    let pull_request = is_pull_request(run);
    let open = Button::new(match line {
        Some(line) => format!("{path}:{line}"),
        None => path.clone(),
    })
    .variant(ButtonVariant::GhostMuted)
    .control_size(ControlSize::Small)
    .on_press(move |s: &mut AppState| {
        if pull_request {
            s.code_review.view = ReviewView::Diff;
            s.code_review.reveal = Some((path.clone(), line.unwrap_or(1)));
            return;
        }
        if let Some(connection) = &s.connection {
            let _ = connection.open_editor_at(workspace, path.clone(), line.unwrap_or(1));
        }
        s.workbench.show(CenterMode::Session);
    })
    .build(k)
    .key(format!("review-finding-{}", finding.id));
    let id = finding.id;
    let mut actions = row()
        .w_full()
        .gap(crate::ds::SPACE_3)
        .items_center()
        .child(reaction_button(
            k,
            finding,
            FindingReaction::Up,
            "👍",
            "review-up",
        ))
        .child(reaction_button(
            k,
            finding,
            FindingReaction::Down,
            "👎",
            "review-down",
        ))
        .child(reaction_button(
            k,
            finding,
            FindingReaction::Dismiss,
            "Dismiss",
            "review-dismiss",
        ))
        .child(el().grow(1.0))
        .child(
            Button::new("Send to agent")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .disabled(send.is_some())
                .on_press(move |s: &mut AppState| s.send_findings(Some(id)))
                .build(k)
                .key(format!("review-send-{id}")),
        );
    if let Some(reason) = send {
        actions = actions.child(note(k, reason));
    }
    let mut head = row().w_full().gap(crate::ds::SPACE_4).items_center().child(
        text(format!("{:?}", finding.severity))
            .font(k.ty.small_medium)
            .color(severity_ink(k, finding.severity)),
    );
    if lineage {
        head = head.child(lineage_badge(k, finding));
    }
    head = head
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
        .child(open);
    let mut item = col()
        .key(format!("review-finding-row-{}", finding.id))
        .w_full()
        .gap(crate::ds::SPACE_2)
        .padding(8.0)
        .radius(k.radius.sm)
        .fill(k.c.surface_hover)
        .child(head)
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
                .radius(k.radius.sm)
                .fill(k.c.bg_deep)
                .child(
                    text(suggestion.clone())
                        .font(k.ty.mono)
                        .color(k.c.text_base)
                        .wrap(),
                ),
        );
    }
    item.child(actions)
}

fn resolved_group(s: &AppState, k: &Kit, report: &Report) -> El {
    let open = s.code_review.show_resolved;
    let mut group = col()
        .key("review-resolved")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .child(
            Button::new(format!(
                "{} Resolved ({})",
                if open { "▾" } else { "▸" },
                report.resolved.len()
            ))
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut AppState| s.code_review.show_resolved = !s.code_review.show_resolved)
            .build(k)
            .key("review-resolved-toggle"),
        );
    if open {
        for (index, finding) in report.resolved.iter().enumerate() {
            group = group.child(
                text(format!(
                    "{:?} · {} · {}",
                    finding.severity, finding.path, finding.title
                ))
                .key(format!("review-resolved-{index}"))
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate()
                .min_w(0.0),
            );
        }
    }
    group
}

fn findings(s: &AppState, k: &Kit, run: &ReviewRun) -> El {
    let workspace = run.workspace_id;
    let Some(report) = s
        .code_review
        .reports
        .get(&(workspace, lane(&run.target)))
        .filter(|report| report.run == run.id)
    else {
        return col();
    };
    let lineage = report.previous.is_some();
    let send = s.send_destination(run).err();
    let groups = group_findings(&report.findings, s.code_review.grouping);
    let mut body = col()
        .key("review-findings")
        .w_full()
        .gap(crate::ds::SPACE_4)
        .child(
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
            body = body.child(finding_row(k, run, finding, lineage, send));
        }
    }
    if !report.resolved.is_empty() {
        body = body.child(resolved_group(s, k, report));
    }
    body
}

/// A finding under its diff line, in the Review tab's Diff and in Changes alike.
pub fn finding_note(k: &Kit, finding: &Finding) -> El {
    row()
        .key(format!("review-note-{}", finding.id))
        .w_full()
        .gap(crate::ds::SPACE_3)
        .items_center()
        .padding(6.0)
        .radius(k.radius.sm)
        .fill(k.c.surface_hover)
        .child(
            text(format!("{:?}", finding.severity))
                .font(k.ty.text_xsmall)
                .color(severity_ink(k, finding.severity)),
        )
        .child(
            text(finding.title.clone())
                .font(k.ty.small)
                .color(k.c.text_base)
                .wrap()
                .min_w(0.0)
                .grow(1.0),
        )
}

fn note_for(k: &Kit, report: Option<&Report>, annotation: &Annotation) -> El {
    report
        .and_then(|report| {
            report
                .findings
                .iter()
                .find(|finding| finding.id.to_string() == annotation.id)
        })
        .map_or_else(col, |finding| finding_note(k, finding))
}

/// The Changes view's annotation builder: the report of the checkout it shows.
pub fn changes_note(s: &AppState, k: &Kit, annotation: &Annotation) -> El {
    let report = s
        .workbench
        .changes
        .workspace
        .and_then(|workspace| s.code_review.workspace_report(workspace));
    note_for(k, report, annotation)
}

fn reviewed_diff(s: &AppState, k: &Kit) -> El {
    let Some(diff) = &s.code_review.diff else {
        return note(k, "The diff is read once the review completes.");
    };
    if let Some(error) = &diff.error {
        return text(error.clone())
            .key("review-diff-error")
            .font(k.ty.small)
            .color(k.c.danger.fg)
            .wrap();
    }
    if !diff.loaded {
        return note(k, "Reading the reviewed diff…");
    }
    let kit = k.clone();
    DiffView::new(
        |s: &AppState| &s.code_review.diff_state,
        |s: &mut AppState| &mut s.code_review.diff_state,
    )
    .annotation(move |s: &AppState, annotation| {
        let report = s
            .code_review
            .workspace
            .and_then(|workspace| s.code_review.shown_report(workspace));
        note_for(&kit, report, annotation)
    })
    .key("review-diff")
    .build(s, k)
    .h(560.0)
}

fn suppressions(s: &AppState, k: &Kit, workspace: WorkspaceId) -> El {
    let Some(config) = s.code_review.configs.get(&workspace) else {
        return col();
    };
    if config.suppressions.is_empty() {
        return col();
    }
    let mut body = col()
        .key("review-suppressed")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .child(section(
            k,
            format!("Suppressed ({})", config.suppressions.len()),
        ));
    for (index, suppression) in config.suppressions.iter().enumerate() {
        let kind = match suppression.kind {
            domain::SuppressionKind::Rule => "rule",
            domain::SuppressionKind::Category => "category",
            domain::SuppressionKind::Fingerprint => "finding",
        };
        let undo = suppression.clone();
        body = body.child(
            row()
                .w_full()
                .gap(crate::ds::SPACE_3)
                .items_center()
                .child(
                    text(format!(
                        "{kind} {} · {}",
                        suppression.value, suppression.reason
                    ))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted)
                    .truncate()
                    .min_w(0.0)
                    .grow(1.0),
                )
                .child(
                    Button::new("Undo")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut AppState| {
                            s.clear_review_suppression(undo.clone());
                        })
                        .build(k)
                        .key(format!("review-suppression-undo-{index}")),
                ),
        );
    }
    body
}

fn report_actions(s: &AppState, k: &Kit, run: &ReviewRun) -> El {
    let send = s.send_destination(run).err();
    let fix_blocked = is_pull_request(run);
    let standing = s
        .code_review
        .shown_report(run.workspace_id)
        .is_some_and(|report| {
            report
                .findings
                .iter()
                .any(|finding| finding.reaction != Some(FindingReaction::Dismiss))
        });
    let mut actions = row()
        .key("review-actions")
        .w_full()
        .gap(crate::ds::SPACE_3)
        .items_center()
        .child(
            SegmentedControl::new(
                &["Findings", "Diff"],
                Some(match s.code_review.view {
                    ReviewView::Findings => 0,
                    ReviewView::Diff => 1,
                }),
            )
            .label("Show")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                s.code_review.view = if index == 1 {
                    ReviewView::Diff
                } else {
                    ReviewView::Findings
                };
            })
            .build(k)
            .key("review-view"),
        )
        .child(el().grow(1.0))
        .child(
            Button::new("Send all")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .disabled(send.is_some() || !standing)
                .on_press(|s: &mut AppState| s.send_findings(None))
                .build(k)
                .key("review-send-all"),
        )
        .child(
            Button::new("Fix with a new agent")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .disabled(fix_blocked || !standing)
                .on_press(|s: &mut AppState| s.fix_with_agent())
                .build(k)
                .key("review-fix"),
        );
    if let Some(reason) = send {
        actions = actions.child(note(k, reason));
    }
    actions
}

fn coverage(k: &Kit, run: &ReviewRun) -> El {
    let coverage = &run.coverage;
    let mut body = col()
        .key("review-coverage")
        .w_full()
        .gap(crate::ds::SPACE_2)
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
    let run = s.code_review.shown_run(workspace);
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
    let lanes = s.code_review.lanes(workspace);
    if lanes.len() > 1 {
        let labels: Vec<String> = lanes.iter().map(|lane| lane_label(*lane)).collect();
        let shown = s.code_review.shown_lane(workspace);
        let at = lanes.iter().position(|lane| *lane == shown);
        top = top.child(
            SegmentedControl::new(&labels, at)
                .label("Review")
                .fit()
                .on_pick(move |s: &mut AppState, index: usize, _| {
                    if let Some(lane) = lanes.get(index) {
                        s.code_review.shown.insert(workspace, *lane);
                    }
                })
                .build(k)
                .key("review-lanes"),
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
                let mut progress = col()
                    .key("review-progress")
                    .w_full()
                    .gap(crate::ds::SPACE_3)
                    .child(
                        Progress::new(Some(stage_progress(run.stage) / 100.0))
                            .label(stage_label(run.stage))
                            .show_value()
                            .build(k),
                    );
                let mut actions = row().gap(crate::ds::SPACE_3);
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
            let completed = run.status == ReviewStatus::Completed;
            if completed {
                body = body.child(report_actions(s, k, run));
            }
            body = match s.code_review.view {
                ReviewView::Diff if completed => body.child(reviewed_diff(s, k)),
                _ => body.child(findings(s, k, run)),
            };
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
    body = body.child(suppressions(s, k, workspace));
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
            lineage: Lineage::New,
            reaction: None,
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
            branch: Some("main".into()),
            tree_oid: None,
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

    fn fixture_state() -> (AppState, WorkspaceId) {
        use crate::connection::{Metrics, fixture_store};
        use crate::terminal::SharedTerminal;
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
        (state, workspace)
    }

    fn completed(workspace: WorkspaceId, target: ReviewTarget) -> ReviewRun {
        let mut run = run(ReviewId::new(), Some(ReviewStage::Validate), true);
        run.workspace_id = workspace;
        run.target = target;
        run
    }

    fn with_report(
        state: &mut AppState,
        run: &ReviewRun,
        findings: Vec<Finding>,
        resolved: Vec<Finding>,
        previous: Option<ReviewId>,
    ) {
        state.apply_review_run(run);
        assert!(state.apply_review_report(
            run.workspace_id,
            Ok(Box::new(client::ReviewReport {
                run: run.clone(),
                findings,
                resolved,
                previous,
            })),
        ));
    }

    fn keys(state: AppState) -> Vec<String> {
        use rune::{Env, Runtime};
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
        rt.elements().into_iter().filter_map(|e| e.key).collect()
    }

    fn has(keys: &[String], key: &str) -> bool {
        keys.iter().any(|candidate| candidate == key)
    }

    fn workspace_target(workspace: WorkspaceId) -> ReviewTarget {
        ReviewTarget::Workspace {
            workspace_id: workspace,
            base: None,
        }
    }

    #[test]
    fn lineage_badges_and_resolved_show_only_against_a_previous_run() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        let mut persisting = finding(1, "a.rs", Some(2), Severity::P1);
        persisting.lineage = Lineage::Persisting;
        with_report(
            &mut state,
            &run,
            vec![persisting, finding(2, "a.rs", Some(4), Severity::P2)],
            vec![finding(9, "gone.rs", Some(1), Severity::P1)],
            Some(ReviewId::new()),
        );
        let shown = keys(state);
        assert!(has(&shown, "review-lineage-1"), "{shown:?}");
        assert!(has(&shown, "review-lineage-2"));
        assert!(has(&shown, "review-resolved"));
        assert!(
            !has(&shown, "review-resolved-0"),
            "the group starts collapsed"
        );

        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        let shown = keys(state);
        assert!(has(&shown, "review-finding-row-1"));
        assert!(
            !has(&shown, "review-lineage-1"),
            "no previous run, no badge"
        );
        assert!(!has(&shown, "review-resolved"));
    }

    #[test]
    fn feedback_controls_collapse_a_dismissed_finding_and_list_suppressions() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        let mut dismissed = finding(2, "a.rs", Some(4), Severity::P2);
        dismissed.reaction = Some(FindingReaction::Dismiss);
        let mut liked = finding(1, "a.rs", Some(2), Severity::P1);
        liked.reaction = Some(FindingReaction::Up);
        with_report(&mut state, &run, vec![liked, dismissed], Vec::new(), None);
        if let Some(config) = state.code_review.configs.get_mut(&workspace) {
            config.suppressions = vec![ReviewSuppression {
                kind: domain::SuppressionKind::Rule,
                value: "money".into(),
                severity: None,
                reason: "3 dismissals in 30 days".into(),
            }];
        }
        let shown = keys(state);
        for key in [
            "review-up-1",
            "review-down-1",
            "review-dismiss-1",
            "review-send-1",
        ] {
            assert!(has(&shown, key), "{key}: {shown:?}");
        }
        assert!(has(&shown, "review-undo-2"));
        assert!(
            !has(&shown, "review-up-2"),
            "a dismissed finding is collapsed"
        );
        assert!(has(&shown, "review-suppressed"));
        assert!(has(&shown, "review-suppression-undo-0"));
        assert!(has(&shown, "review-view"));
        assert!(has(&shown, "review-send-all"));
        assert!(has(&shown, "review-fix"));
    }

    #[test]
    fn pressing_the_active_reaction_clears_it() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        let before = state.code_review.revision;
        state.react_to_finding(1, FindingReaction::Dismiss);
        let reaction = |state: &AppState| {
            state.code_review.reports[&(workspace, Lane::Workspace)].findings[0].reaction
        };
        assert_eq!(reaction(&state), Some(FindingReaction::Dismiss));
        assert!(state.code_review.revision > before, "annotations rebuild");
        state.react_to_finding(1, FindingReaction::Dismiss);
        assert_eq!(reaction(&state), None);
    }

    #[test]
    fn the_diff_tab_draws_the_reviewed_range_with_its_findings() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        state.code_review.view = ReviewView::Diff;
        state.sync_review();
        assert_eq!(state.code_review.diff.as_ref().map(|d| d.run), Some(run.id));
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@\n one\n+two\n three\n";
        assert!(state.apply_review_diff(
            run.id,
            Ok(domain::WorkspaceDiff {
                workspace_id: workspace,
                branch: None,
                files: vec![domain::DiffFile {
                    path: "a.rs".into(),
                    status: domain::DiffStatus::Modified,
                    additions: 1,
                    deletions: 0,
                    binary: false,
                    truncated: false,
                    patch: patch.into(),
                }],
                truncated: false,
            }),
        ));
        state.sync_review();
        assert_eq!(state.code_review.diff_state.annotations().len(), 1);
        let shown = keys(state);
        assert!(has(&shown, "review-diff"), "{shown:?}");
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
    fn only_a_fresh_completed_workspace_review_annotates_changes() {
        let workspace = WorkspaceId::new();
        let run = completed(workspace, workspace_target(workspace));
        assert!(annotates_changes(&run, Some(run.head_oid.as_str())));
        assert!(annotates_changes(&run, None));
        assert!(!annotates_changes(&run, Some("moved")), "stale");
        let mut running = run.clone();
        running.status = ReviewStatus::Running;
        assert!(!annotates_changes(&running, None));
        let pr = completed(
            workspace,
            ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: 7,
                base: None,
            },
        );
        assert!(
            !annotates_changes(&pr, None),
            "a PR head is not the working tree"
        );
    }

    #[test]
    fn a_pull_request_run_is_never_stale_and_names_its_number() {
        let workspace = WorkspaceId::new();
        let pr = completed(
            workspace,
            ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: 7,
                base: Some("main".into()),
            },
        );
        assert!(!is_stale(&pr, Some("anything")));
        assert_eq!(target_label(&pr.target), "PR #7");
        assert_eq!(target_label(&workspace_target(workspace)), "Workspace");
    }

    #[test]
    fn send_and_fix_are_disabled_for_a_pull_request_run() {
        let (mut state, workspace) = fixture_state();
        let run = completed(
            workspace,
            ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: 7,
                base: None,
            },
        );
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        assert_eq!(
            state.send_destination(&run),
            Err("Send works on a workspace review, not a pull request's.")
        );
        let shown = keys(state);
        assert!(has(&shown, "review-target"));
    }

    fn agent(workspace: WorkspaceId, at: &str) -> Session {
        let store = crate::connection::fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let mut session = store.sessions[0].clone();
        session.id = SessionId::new();
        session.workspace_id = workspace;
        session.kind = SessionKind::Agent;
        session.state = domain::SessionState::Running;
        session.background = false;
        session.last_activity_at = Timestamp::parse_rfc3339(at).unwrap();
        session
    }

    #[test]
    fn the_send_target_is_the_last_active_live_agent_of_the_checkout() {
        let workspace = WorkspaceId::new();
        let older = agent(workspace, "2026-11-01T10:00:00Z");
        let newer = agent(workspace, "2026-11-01T11:00:00Z");
        let mut background = agent(workspace, "2026-11-01T12:00:00Z");
        background.background = true;
        let mut exited = agent(workspace, "2026-11-01T13:00:00Z");
        exited.state = domain::SessionState::Exited {
            code: Some(0),
            signal: None,
        };
        let mut shell = agent(workspace, "2026-11-01T14:00:00Z");
        shell.kind = SessionKind::Shell;
        let reviewer = agent(workspace, "2026-11-01T15:00:00Z");
        let elsewhere = agent(WorkspaceId::new(), "2026-11-01T16:00:00Z");
        let sessions = vec![
            older.clone(),
            newer.clone(),
            background,
            exited,
            shell,
            reviewer.clone(),
            elsewhere,
        ];
        assert_eq!(
            send_target(&sessions, workspace, &[reviewer.id]),
            Some(newer.id)
        );
        assert_eq!(
            send_target(std::slice::from_ref(&older), workspace, &[older.id]),
            None,
            "a reviewer is never the target"
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
    fn send_all_and_fix_leave_dismissed_findings_out() {
        let mut dismissed = finding(1, "a.rs", Some(2), Severity::P0);
        dismissed.reaction = Some(FindingReaction::Dismiss);
        let kept = finding(2, "b.rs", Some(4), Severity::P1);
        let (prompt, left_out) = standing_prompt(&[dismissed, kept]);
        assert_eq!(left_out, 0);
        assert!(prompt.contains("b.rs:4"));
        assert!(!prompt.contains("a.rs:2"));
        assert_eq!(
            delivery_message(true, 2),
            "Queued for the agent's next idle · 2 finding(s) left out to fit 8 KiB"
        );
        assert_eq!(delivery_message(false, 0), "Sent to the agent");
    }

    #[test]
    fn reactions_are_thumbs_and_dismiss_stays_a_word() {
        use rune::{Env, Runtime};
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
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
        let elements = rt.elements();
        let text_under = |key: &str| -> Vec<String> {
            let Some(button) = elements.iter().find(|e| e.key.as_deref() == Some(key)) else {
                return Vec::new();
            };
            elements
                .iter()
                .filter(|e| e.path.starts_with(&button.path))
                .filter_map(|e| e.text.clone())
                .collect()
        };
        assert_eq!(text_under("review-up-1"), ["👍"]);
        assert_eq!(text_under("review-down-1"), ["👎"]);
        assert_eq!(text_under("review-dismiss-1"), ["Dismiss"]);
    }

    #[test]
    fn the_send_destination_is_stored_when_sessions_or_runs_change() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        let mut store = state.store.clone().expect("store");
        store
            .sessions
            .retain(|session| session.workspace_id != workspace);
        let target = agent(workspace, "2026-11-01T10:00:00Z");
        store.sessions.push(target.clone());
        let snapshot = |store: client::Store| crate::connection::UiEvent::Snapshot {
            generation: 1,
            store: Box::new(store),
            took_us: 0,
        };
        state.apply_events(vec![snapshot(store.clone())], &mut rune::Tx::default());
        assert_eq!(state.code_review.send_to.get(&workspace), Some(&target.id));
        assert_eq!(state.send_destination(&run), Ok(target.id));

        let mut reviewed = run.clone();
        reviewed.id = ReviewId::new();
        reviewed.session_id = Some(target.id);
        state.apply_review_run(&reviewed);
        assert_eq!(
            state.code_review.send_to.get(&workspace),
            None,
            "a session a run names as its reviewer is never the target"
        );

        store.sessions.retain(|session| session.id != target.id);
        state.apply_events(vec![snapshot(store)], &mut rune::Tx::default());
        assert_eq!(state.code_review.send_to.get(&workspace), None);
    }

    fn pull_request(workspace: WorkspaceId, number: u32) -> ReviewTarget {
        ReviewTarget::PullRequest {
            workspace_id: workspace,
            number,
            base: None,
        }
    }

    #[test]
    fn a_pull_request_review_keeps_its_own_lane_beside_the_workspace_one() {
        let (mut state, workspace) = fixture_state();
        let mine = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &mine,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        let theirs = completed(workspace, pull_request(workspace, 7));
        with_report(
            &mut state,
            &theirs,
            vec![finding(2, "b.rs", Some(4), Severity::P2)],
            Vec::new(),
            None,
        );
        let review = &state.code_review;
        assert_eq!(
            review.lanes(workspace),
            [Lane::Workspace, Lane::PullRequest(7)]
        );
        assert_eq!(
            review.shown_run(workspace).map(|run| run.id),
            Some(mine.id),
            "a run landing in another lane does not switch the tab"
        );
        assert_eq!(
            review.workspace_report(workspace).map(|r| r.run),
            Some(mine.id)
        );

        state
            .code_review
            .shown
            .insert(workspace, Lane::PullRequest(7));
        assert_eq!(
            state.code_review.shown_report(workspace).map(|r| r.run),
            Some(theirs.id)
        );
        assert_eq!(
            state.code_review.workspace_report(workspace).map(|r| r.run),
            Some(mine.id),
            "the Changes view keeps the workspace review's findings"
        );
        let shown = keys(state);
        assert!(has(&shown, "review-lanes"), "{shown:?}");
    }

    #[test]
    fn a_reviewed_diff_lost_to_a_reconnect_is_asked_again() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &run, Vec::new(), Vec::new(), None);
        state.code_review.view = ReviewView::Diff;
        state.sync_review();
        assert!(state.code_review.diff.as_ref().is_some_and(|d| !d.loaded));
        reconnected(&mut state);
        assert!(state.code_review.diff.is_none());
        state.sync_review();
        assert_eq!(state.code_review.diff.as_ref().map(|d| d.run), Some(run.id));
    }

    #[test]
    fn no_standing_finding_is_not_reported_as_too_long() {
        assert_eq!(findings_prompt(&[], PROMPT_BUDGET), (String::new(), 0));
        assert_ne!(empty_prompt_reason(0), empty_prompt_reason(1));
        assert!(empty_prompt_reason(0).contains("dismissed"));
    }

    #[test]
    fn a_pull_request_finding_scrolls_the_reviewed_diff_to_its_line() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, pull_request(workspace, 7));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        state.code_review.view = ReviewView::Diff;
        state.code_review.reveal = Some(("a.rs".into(), 2));
        state.sync_review();
        let mut tx = rune::Tx::default();
        assert!(!state.settle_review_reveal(&mut tx), "waits for the diff");
        assert!(state.code_review.reveal.is_some());
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@\n one\n+two\n three\n";
        assert!(state.apply_review_diff(
            run.id,
            Ok(domain::WorkspaceDiff {
                workspace_id: workspace,
                branch: None,
                files: vec![domain::DiffFile {
                    path: "a.rs".into(),
                    status: domain::DiffStatus::Modified,
                    additions: 1,
                    deletions: 0,
                    binary: false,
                    truncated: false,
                    patch: patch.into(),
                }],
                truncated: false,
            }),
        ));
        assert!(state.settle_review_reveal(&mut tx));
        assert!(state.code_review.reveal.is_none());
    }
}
