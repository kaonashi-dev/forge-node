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
    SegmentedControl, TextArea, TextInput,
};

use crate::app::AppState;
use crate::pr_url::PullRequestLink;
use crate::review_subject::{Resolution, Subject};
use crate::workbench::CenterMode;

mod model;

pub use client::CUSTOM_PRESET;
pub use client::MAX_PROMPT_BYTES as PROMPT_BUDGET;
use model::{
    STAGES, annotations, findings_prompt, group_findings, is_stale, standing_prompt, supersedes,
};

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
    Diagrams,
    Graph,
}

/// Files of a run whose graph neighbours the Graph tab lists.
const MAX_GRAPH_FILES: usize = 20;
/// Bytes of a sandbox target's output the TREX panel shows.
const MAX_SANDBOX_TAIL: usize = 2048;
const FIX_LOOP_ITERATIONS: u32 = 3;

/// One reviewed file's neighbours in the codebase graph.
#[derive(Clone, Debug, Default)]
pub struct GraphFile {
    pub path: String,
    pub imports: Option<Result<Vec<domain::GraphNode>, String>>,
    pub external: Option<Result<Vec<domain::GraphNode>, String>>,
}

/// The Graph tab's read of one run.
#[derive(Clone, Debug)]
pub struct ReviewGraph {
    pub run: ReviewId,
    pub files: Vec<GraphFile>,
}

/// A workspace's fix loop as its events report it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixLoopProgress {
    pub iteration: u32,
    pub findings: u32,
    pub done: bool,
    pub error: Option<String>,
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
    /// `findings` grouped by `grouped_by`, as indices into it.
    groups: Vec<(String, Vec<usize>)>,
    grouped_by: Grouping,
    /// A finding's index by its id as an annotation names it.
    by_id: HashMap<String, usize>,
}

impl Report {
    fn new(
        run: ReviewId,
        findings: Vec<Finding>,
        resolved: Vec<Finding>,
        previous: Option<ReviewId>,
        grouping: Grouping,
    ) -> Self {
        let by_id = findings
            .iter()
            .enumerate()
            .map(|(at, finding)| (finding.id.to_string(), at))
            .collect();
        Self {
            groups: group_findings(&findings, grouping),
            grouped_by: grouping,
            run,
            findings,
            resolved,
            previous,
            by_id,
        }
    }

    fn regroup(&mut self, grouping: Grouping) {
        if self.grouped_by != grouping {
            self.groups = group_findings(&self.findings, grouping);
            self.grouped_by = grouping;
        }
    }

    fn finding(&self, annotation: &Annotation) -> Option<&Finding> {
        self.findings.get(*self.by_id.get(annotation.id.as_str())?)
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedPullRequest {
    pub workspace: WorkspaceId,
    pub repository: String,
    pub number: u32,
    pub title: String,
    pub base: Option<String>,
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
    pub subject: Subject,
    pub pull_request: TextEdit,
    /// The link last copied out of the instructions, so switching back to the
    /// changes is not undone by the next keystroke.
    pub detected: Option<PullRequestLink>,
    /// A linked pull request the daemon is reading before the review starts.
    pub awaiting: Option<PullRequestLink>,
    /// Set when the tab was opened for one pull request: it replaces the
    /// what-to-review choice until the person asks to change it.
    pub pinned: Option<PinnedPullRequest>,
    pub starting: bool,
    /// The run this window started, until its reviewer session is shown.
    pub follow: Option<ReviewId>,
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
    /// `reviewers` of the store's providers, recomputed when they change.
    pub offered: Vec<(AgentProviderId, String)>,
    /// Each checkout's lanes in order, extended when a run of a new lane lands.
    lanes: HashMap<WorkspaceId, Vec<Lane>>,
    pub graph: Option<ReviewGraph>,
    /// The knowledge base of the workspace whose run cites it, once read.
    pub kb: Option<(WorkspaceId, Result<Vec<domain::KbFile>, String>)>,
    pub kb_open: bool,
    pub sandbox: Option<(ReviewId, Result<domain::SandboxRun, String>)>,
    /// The run a person pressed Fix Loop on, until they confirm or cancel.
    pub fix_loop_confirm: Option<ReviewId>,
    pub fix_loops: HashMap<WorkspaceId, FixLoopProgress>,
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
            subject: Subject::Changes,
            pull_request: TextEdit::new(""),
            detected: None,
            awaiting: None,
            pinned: None,
            starting: false,
            follow: None,
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
            offered: Vec::new(),
            lanes: HashMap::new(),
            graph: None,
            kb: None,
            kb_open: false,
            sandbox: None,
            fix_loop_confirm: None,
            fix_loops: HashMap::new(),
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

    fn lanes(&self, workspace: WorkspaceId) -> &[Lane] {
        self.lanes.get(&workspace).map_or(&[], Vec::as_slice)
    }

    fn set_grouping(&mut self, grouping: Grouping) {
        self.grouping = grouping;
        for report in self.reports.values_mut() {
            report.regroup(grouping);
        }
    }

    fn knows(&self, workspace: WorkspaceId) -> bool {
        self.latest.keys().any(|(owner, _)| *owner == workspace)
    }

    #[must_use]
    pub fn pinned_to(&self, workspace: WorkspaceId) -> Option<&PinnedPullRequest> {
        self.pinned
            .as_ref()
            .filter(|pin| pin.workspace == workspace)
    }

    /// The lane the start button would run in, `None` while the typed pull
    /// request does not parse.
    fn target_lane(&self, workspace: WorkspaceId) -> Option<Lane> {
        if let Some(pin) = self.pinned_to(workspace) {
            return Some(Lane::PullRequest(pin.number));
        }
        match self.subject {
            Subject::Changes => Some(Lane::Workspace),
            Subject::PullRequest => match crate::review_subject::parse(&self.pull_request.text) {
                Ok(crate::review_subject::Reference::Number(number)) => {
                    Some(Lane::PullRequest(number))
                }
                Ok(crate::review_subject::Reference::Link(link)) => {
                    Some(Lane::PullRequest(link.number))
                }
                Err(_) => None,
            },
        }
    }

    /// Whether pressing start again would send a second review of the same
    /// target; the running one has to be cancelled first.
    #[must_use]
    pub fn busy(&self, workspace: WorkspaceId) -> bool {
        self.starting
            || self.awaiting.is_some()
            || self.follow.is_some()
            || self
                .target_lane(workspace)
                .and_then(|lane| self.latest.get(&(workspace, lane)))
                .is_some_and(running)
    }
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
    s.code_review.pinned = None;
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
    s.code_review.starting = false;
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
            s.code_review.errors.insert(workspace, error);
        }
    }
    sync_reads(s, workspace, generation);
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

/// The Graph tab's and the TREX panel's reads, once per shown run.
fn sync_reads(s: &mut AppState, workspace: WorkspaceId, generation: u64) {
    let Some(run) = s.code_review.shown_run(workspace).cloned() else {
        return;
    };
    if run.status != ReviewStatus::Completed {
        return;
    }
    if s.code_review.view == ReviewView::Graph
        && s.code_review
            .graph
            .as_ref()
            .is_none_or(|graph| graph.run != run.id)
    {
        let files: Vec<GraphFile> = run
            .coverage
            .included
            .iter()
            .take(MAX_GRAPH_FILES)
            .map(|path| GraphFile {
                path: path.clone(),
                ..GraphFile::default()
            })
            .collect();
        if let Some(connection) = &s.connection {
            for file in &files {
                for query in [
                    domain::GraphQuery::Imports(file.path.clone()),
                    domain::GraphQuery::ExternalCalls(file.path.clone()),
                ] {
                    let _ = connection.query_graph(
                        generation,
                        run.id,
                        workspace,
                        file.path.clone(),
                        query,
                    );
                }
            }
        }
        s.code_review.graph = Some(ReviewGraph { run: run.id, files });
    }
    if let Some(id) = &run.sandbox_run_id
        && s.code_review
            .sandbox
            .as_ref()
            .is_none_or(|(read, _)| *read != run.id)
        && let Some(connection) = &s.connection
    {
        s.code_review.sandbox = Some((run.id, Err("Reading the sandbox run…".into())));
        let _ = connection.get_sandbox_run(generation, run.id, id.clone());
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
            self.code_review
                .offered
                .first()
                .map(|(id, _)| id.to_string())
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
        let target = match (
            self.code_review.pinned_to(workspace),
            self.code_review.subject,
        ) {
            (Some(pin), _) => ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: pin.number,
                base: pin.base.clone(),
            },
            (None, Subject::Changes) => ReviewTarget::Workspace {
                workspace_id: workspace,
                base: None,
            },
            (None, Subject::PullRequest) => match self.pull_request_target(workspace) {
                Some(target) => target,
                None => return,
            },
        };
        self.send_review(
            target,
            Some(preset),
            self.code_review.tier,
            Some(provider),
            instructions,
        );
    }

    /// `None` when the review cannot start yet: the reason is on screen, or a
    /// linked pull request is being read first.
    fn pull_request_target(&mut self, workspace: WorkspaceId) -> Option<ReviewTarget> {
        let reference = match crate::review_subject::parse(&self.code_review.pull_request.text) {
            Ok(reference) => reference,
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
                return None;
            }
        };
        let resolution = match self.store.as_ref() {
            Some(store) => crate::review_subject::resolve(
                &reference,
                &store.pull_requests.pull_requests,
                &store.workspaces,
                workspace,
            ),
            None => Resolution::Refused("Not connected to the daemon.".into()),
        };
        let problem = match resolution {
            Resolution::Ready(target) => {
                return Some(ReviewTarget::PullRequest {
                    workspace_id: target.workspace,
                    number: target.number,
                    base: target.base,
                });
            }
            Resolution::Refused(problem) => problem,
            Resolution::Unlisted(link) => match &self.connection {
                Some(connection) => match connection.fetch_pull_request(link.clone()) {
                    Ok(()) => {
                        self.code_review.awaiting = Some(link);
                        self.code_review.errors.remove(&workspace);
                        return None;
                    }
                    Err(error) => error,
                },
                None => "The client worker is still starting.".into(),
            },
        };
        self.code_review.errors.insert(workspace, problem);
        None
    }

    /// `PullRequestFetched` for the link a review is waiting on: start it now
    /// that the daemon knows which repository the pull request lives in.
    pub fn review_link_fetched(
        &mut self,
        host: &str,
        repository: &str,
        number: u32,
        error: Option<String>,
    ) {
        let Some(link) = self.code_review.awaiting.clone() else {
            return;
        };
        if !link.is(host, repository, number) {
            return;
        }
        self.code_review.awaiting = None;
        let listed = self.store.as_ref().is_some_and(|store| {
            store
                .pull_requests
                .pull_requests
                .iter()
                .any(|pr| link.names(pr))
        });
        let problem = match error {
            Some(error) => Some(error),
            None if listed => None,
            None => Some("GitHub answered, but the pull request did not arrive; try again.".into()),
        };
        match (problem, self.code_review.workspace) {
            (None, _) if self.code_review.subject == Subject::PullRequest => {
                self.start_code_review();
            }
            (None, _) => {}
            (Some(problem), Some(workspace)) => {
                self.code_review.errors.insert(workspace, problem);
            }
            (Some(_), None) => {}
        }
    }

    /// The daemon refused to read the awaited link, e.g. a host not in `[github]`.
    pub fn review_link_refused(&mut self, message: &str) {
        if self.code_review.awaiting.take().is_none() {
            return;
        }
        if let Some(workspace) = self.code_review.workspace {
            self.code_review
                .errors
                .insert(workspace, message.to_owned());
        }
    }

    /// Copies a pull-request link typed into the instructions into the subject,
    /// once per link, so the target is visible before the review starts.
    pub fn review_instructions_edited(&mut self) {
        if self.code_review.pinned.is_some() {
            return;
        }
        let found = crate::review_subject::find_link(&self.code_review.instructions.text);
        let Some(link) = found else {
            return;
        };
        if self.code_review.detected.as_ref() == Some(&link) {
            return;
        }
        self.code_review.pull_request = TextEdit::new(format!(
            "https://{}/{}/pull/{}",
            link.host, link.repository, link.number
        ));
        self.code_review.subject = Subject::PullRequest;
        self.code_review.detected = Some(link);
    }

    fn send_review(
        &mut self,
        target: ReviewTarget,
        preset: Option<String>,
        tier: Option<Tier>,
        provider: Option<String>,
        instructions: Option<String>,
    ) {
        let workspace = target.workspace_id();
        let shown = lane(&target);
        let result = match &self.connection {
            Some(connection) => connection.start_review(
                self.generation,
                target,
                preset,
                tier,
                provider,
                instructions,
            ),
            None => Err("The client worker is still starting.".to_string()),
        };
        if self.code_review.workspace != Some(workspace) {
            open(self, workspace);
        }
        self.code_review.shown.insert(workspace, shown);
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
            let lanes = self.code_review.lanes.entry(slot.0).or_default();
            if !lanes.contains(&slot.1) {
                lanes.push(slot.1);
                lanes.sort_unstable();
            }
            self.refresh_send_targets();
        }
        if self.code_review.follow == Some(run.id) {
            self.follow_reviewer(run.workspace_id, run.session_id, run.status.is_finished());
        }
        if self.code_review.workspace == Some(run.workspace_id) {
            self.code_review.errors.remove(&run.workspace_id);
        }
    }

    /// The reviewer session only exists once the diff and pack stages are
    /// done, so the switch waits for it. A person who has left the Review tab
    /// meanwhile keeps where they are: their keystrokes must not land in it.
    fn follow_reviewer(&mut self, workspace: WorkspaceId, session: Option<SessionId>, ended: bool) {
        if let Some(session) = session {
            self.code_review.follow = None;
            if self.workbench.mode == CenterMode::Review
                && self.code_review.workspace == Some(workspace)
            {
                self.focus_session(session);
            }
        } else if ended {
            self.code_review.follow = None;
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
                let offered = &self.code_review.offered;
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
                    Report::new(
                        run.id,
                        findings,
                        resolved,
                        previous,
                        self.code_review.grouping,
                    ),
                );
                self.code_review.revision += 1;
                self.apply_review_run(&run);
                self.code_review.errors.remove(&workspace);
            }
            Err(error) => {
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
            Ok(id) => {
                self.code_review.errors.remove(&workspace);
                self.code_review.follow = Some(id);
                let landed = self
                    .code_review
                    .latest
                    .values()
                    .find(|run| run.id == id)
                    .map(|run| (run.workspace_id, run.session_id, run.status.is_finished()));
                if let Some((workspace, session, ended)) = landed {
                    self.follow_reviewer(workspace, session, ended);
                }
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

    pub fn apply_graph_nodes(
        &mut self,
        review_id: ReviewId,
        path: &str,
        query: &domain::GraphQuery,
        result: Result<Vec<domain::GraphNode>, String>,
    ) -> bool {
        let Some(graph) = self
            .code_review
            .graph
            .as_mut()
            .filter(|graph| graph.run == review_id)
        else {
            return false;
        };
        let Some(file) = graph.files.iter_mut().find(|file| file.path == path) else {
            return false;
        };
        match query {
            domain::GraphQuery::Imports(_) => file.imports = Some(result),
            _ => file.external = Some(result),
        }
        true
    }

    pub fn apply_kb(
        &mut self,
        workspace: WorkspaceId,
        result: Result<Vec<domain::KbFile>, String>,
    ) -> bool {
        self.code_review.kb = Some((workspace, result));
        true
    }

    pub fn apply_sandbox_run(
        &mut self,
        review_id: ReviewId,
        result: Result<Box<domain::SandboxRun>, String>,
    ) -> bool {
        if self
            .code_review
            .sandbox
            .as_ref()
            .is_none_or(|(run, _)| *run != review_id)
        {
            return false;
        }
        self.code_review.sandbox = Some((review_id, result.map(|run| *run)));
        true
    }

    pub fn apply_fix_loop(&mut self, workspace: WorkspaceId, progress: FixLoopProgress) -> bool {
        if progress.done {
            if let Some(error) = &progress.error {
                self.toasts.danger("The fix loop stopped", error.clone());
            } else {
                self.toasts.neutral(
                    "The fix loop finished",
                    Some(format!(
                        "{} finding(s) left after {} review(s)",
                        progress.findings, progress.iteration
                    )),
                );
            }
        }
        self.code_review.fix_loops.insert(workspace, progress);
        true
    }

    pub fn toggle_review_kb(&mut self, workspace: WorkspaceId) {
        self.code_review.kb_open = !self.code_review.kb_open;
        if self.code_review.kb_open
            && self
                .code_review
                .kb
                .as_ref()
                .is_none_or(|(read, _)| *read != workspace)
            && let Some(connection) = &self.connection
            && let Err(error) = connection.get_kb(self.generation, workspace)
        {
            self.code_review.kb = Some((workspace, Err(error)));
        }
    }

    /// The confirmed fix loop, with the agent "Fix with a new agent" would start.
    pub fn start_fix_loop(&mut self) {
        let Some(review_id) = self.code_review.fix_loop_confirm.take() else {
            return;
        };
        let (provider, profile) = match self.prompt_agent() {
            Ok(agent) => agent,
            Err(reason) => {
                self.toasts.danger("The fix loop did not start", reason);
                return;
            }
        };
        let Some(connection) = &self.connection else {
            return;
        };
        if let Err(error) = connection.start_fix_loop(
            review_id,
            Some(FIX_LOOP_ITERATIONS),
            Some(provider.to_string()),
            profile,
        ) {
            self.toasts.danger("The fix loop did not start", error);
        }
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

    pub(crate) fn refresh_reviewers(&mut self) {
        self.code_review.offered = self
            .store
            .as_ref()
            .map(|store| reviewers(&store.providers))
            .unwrap_or_default();
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

    /// The Review tab set up for one listed pull request, in the checkout
    /// `review_workspace` picks; nothing starts until the person presses it.
    ///
    /// # Errors
    /// A sentence for the person when the pull request is not listed or no
    /// checkout of its repository is open.
    pub fn open_pull_request_review(
        &mut self,
        repository: &str,
        number: u32,
    ) -> Result<(), String> {
        let Some(pr) = self.pull_request(repository, number) else {
            return Err(format!(
                "{repository}#{number} is not listed yet; refresh and try again."
            ));
        };
        let workspace = self.store.as_ref().and_then(|store| {
            crate::pr_review::review_workspace(
                &store.workspaces,
                pr.project_id,
                self.workbench.workspace,
            )
        });
        let Some(workspace) = workspace else {
            return Err(format!("No checkout of {repository} is open here."));
        };
        let pin = PinnedPullRequest {
            workspace,
            repository: pr.repository.clone(),
            number,
            title: pr.title.clone(),
            base: (!pr.base_ref.is_empty()).then(|| pr.base_ref.clone()),
        };
        open(self, workspace);
        self.code_review.pinned = Some(pin);
        self.code_review
            .shown
            .insert(workspace, Lane::PullRequest(number));
        Ok(())
    }

    /// Back to the what-to-review choice, with the pinned pull request filled in.
    pub fn unpin_review(&mut self) {
        let Some(pin) = self.code_review.pinned.take() else {
            return;
        };
        self.code_review.subject = Subject::PullRequest;
        self.code_review.pull_request = TextEdit::new(format!("#{}", pin.number));
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
    let offered = &s.code_review.offered;
    let mut body = col().key("review-setup").w_full().gap(crate::ds::SPACE_4);
    let pinned = s.code_review.pinned_to(workspace);
    body = body.child(match pinned {
        Some(pin) => pinned_subject(k, pin),
        None => subject(s, k),
    });
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
        for (id, name) in offered {
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
            .on_change(|s: &mut AppState, _| s.review_instructions_edited())
            .build(k)
            .key("review-instructions")
            .h(80.0),
    );
    let custom = s.code_review.preset.as_deref() == Some(CUSTOM_PRESET);
    let busy = s.code_review.busy(workspace);
    let ready = config.is_some()
        && !offered.is_empty()
        && !busy
        && (!custom || !s.code_review.instructions.text.trim().is_empty())
        && (pinned.is_some()
            || s.code_review.subject == Subject::Changes
            || !s.code_review.pull_request.text.trim().is_empty());
    let mut actions = row()
        .w_full()
        .gap(crate::ds::SPACE_4)
        .items_center()
        .child(el().grow(1.0));
    if busy {
        actions = actions.child(crate::ui::busy(s, k));
    }
    body = body.child(
        actions.child(
            Button::new(start_label(&s.code_review, workspace))
                .variant(ButtonVariant::Contrast)
                .disabled(!ready)
                .on_press(|s: &mut AppState| s.start_code_review())
                .build(k)
                .key("review-start"),
        ),
    );
    body
}

fn start_label(review: &CodeReview, workspace: WorkspaceId) -> String {
    if review.starting {
        return "Starting…".into();
    }
    if let Some(link) = &review.awaiting {
        return format!("Reading #{}…", link.number);
    }
    if review.follow.is_some() {
        return "Starting the reviewer…".into();
    }
    if review.busy(workspace) {
        return "Reviewing…".into();
    }
    if let Some(pin) = review.pinned_to(workspace) {
        return format!("Review PR #{}", pin.number);
    }
    match review.subject {
        Subject::Changes => "Review changes".into(),
        Subject::PullRequest => match crate::review_subject::parse(&review.pull_request.text) {
            Ok(crate::review_subject::Reference::Number(number)) => format!("Review PR #{number}"),
            Ok(crate::review_subject::Reference::Link(link)) => {
                format!("Review PR #{}", link.number)
            }
            Err(_) => "Review pull request".into(),
        },
    }
}

fn subject(s: &AppState, k: &Kit) -> El {
    let review = &s.code_review;
    let mut what = RadioGroup::new().label("What to review");
    for (label, detail, choice) in [
        (
            "Changes in this checkout",
            "The working tree against its base branch, uncommitted changes included.",
            Subject::Changes,
        ),
        (
            "A pull request",
            "Its head against its base, fetched without moving this checkout.",
            Subject::PullRequest,
        ),
    ] {
        what = what.item(
            RadioItem::new(label, review.subject == choice)
                .description(detail)
                .on_select(move |s: &mut AppState| s.code_review.subject = choice),
        );
    }
    let mut column = col()
        .key("review-subject")
        .w_full()
        .gap(crate::ds::SPACE_3)
        .child(what.build(k).key("review-subject-kind"));
    if review.subject == Subject::PullRequest {
        column = column.child(
            TextInput::new(&review.pull_request)
                .label("Pull request")
                .placeholder("96, #96 or https://github.com/owner/repo/pull/96")
                .width(None)
                .on_edit(|s: &mut AppState| &mut s.code_review.pull_request)
                .build(k)
                .key("review-pull-request"),
        );
    }
    column
}

fn pinned_subject(k: &Kit, pin: &PinnedPullRequest) -> El {
    col()
        .key("review-pinned")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .child(note(k, "Reviewing"))
        .child(
            row()
                .w_full()
                .gap(crate::ds::SPACE_3)
                .items_center()
                .child(
                    text(format!("#{}", pin.number))
                        .font(k.ty.mono)
                        .color(k.c.text_muted),
                )
                .child(
                    text(pin.title.clone())
                        .font(k.ty.small_medium)
                        .color(k.c.text_base)
                        .truncate()
                        .min_w(0.0)
                        .grow(1.0),
                )
                .child(
                    Button::new("Review something else")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut AppState| s.unpin_review())
                        .build(k)
                        .key("review-unpin"),
                ),
        )
        .child(note(
            k,
            format!(
                "{} · its head against its base, fetched without moving this checkout.",
                pin.repository
            ),
        ))
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
    if let Some(hypothesis) = finding.hypothesis {
        head = head.child(
            crate::ds::chip(
                k,
                &crate::theme::roles(k),
                hypothesis.name(),
                crate::ds::Tone::Info,
            )
            .key(format!("review-hypothesis-{}", finding.id)),
        );
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
                        s.code_review.set_grouping(match s.code_review.grouping {
                            Grouping::Severity => Grouping::File,
                            Grouping::File => Grouping::Severity,
                        });
                    })
                    .build(k)
                    .key("review-grouping"),
                ),
        );
    for (key, group) in &report.groups {
        body = body.child(section(k, key.clone()));
        for at in group {
            body = body.child(finding_row(k, run, &report.findings[*at], lineage, send));
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
        .and_then(|report| report.finding(annotation))
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
                &["Findings", "Diff", "Diagrams", "Graph"],
                Some(match s.code_review.view {
                    ReviewView::Findings => 0,
                    ReviewView::Diff => 1,
                    ReviewView::Diagrams => 2,
                    ReviewView::Graph => 3,
                }),
            )
            .label("Show")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                s.code_review.view = match index {
                    1 => ReviewView::Diff,
                    2 => ReviewView::Diagrams,
                    3 => ReviewView::Graph,
                    _ => ReviewView::Findings,
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
        )
        .child({
            let looping = s
                .code_review
                .fix_loops
                .get(&run.workspace_id)
                .is_some_and(|progress| !progress.done);
            let review_id = run.id;
            Button::new(if looping {
                "Fix loop running…"
            } else {
                "Fix Loop"
            })
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(fix_blocked || !standing || looping)
            .on_press(move |s: &mut AppState| s.code_review.fix_loop_confirm = Some(review_id))
            .build(k)
            .key("review-fix-loop")
        });
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

fn verdict_tone(verdict: Verdict) -> crate::ds::Tone {
    match verdict {
        Verdict::Block => crate::ds::Tone::Danger,
        Verdict::Review => crate::ds::Tone::Warning,
        Verdict::Ok => crate::ds::Tone::Success,
    }
}

fn score_badge(k: &Kit, run: &ReviewRun) -> Option<El> {
    let verdict = run.verdict?;
    let approvable = if run.auto_approvable {
        "auto-approvable"
    } else {
        "needs a person"
    };
    let label = format!(
        "Score {} · {} · {approvable}",
        score_label(run.score),
        verdict_label(verdict)
    );
    Some(
        row()
            .key("review-score-badge")
            .gap(crate::ds::SPACE_3)
            .items_center()
            .child(
                crate::ds::chip(
                    k,
                    &crate::theme::roles(k),
                    score_label(run.score),
                    verdict_tone(verdict),
                )
                .label(label),
            )
            .child(note(
                k,
                format!("{} · {approvable}", verdict_label(verdict)),
            )),
    )
}

fn palette(k: &Kit) -> crate::diagram_svg::Palette {
    let roles = crate::theme::roles(k);
    crate::diagram_svg::Palette {
        text: roles.text.hex(),
        muted: roles.muted.hex(),
        line: roles.border_strong.hex(),
        fill: roles.surface.hex(),
        stroke: roles.border_hi.hex(),
    }
}

/// Points the widest diagram is rasterized for; a wider drawing is scaled down.
const DIAGRAM_PT: f32 = 1200.0;

fn diagrams(s: &AppState, k: &Kit, run: &ReviewRun) -> El {
    let mut body = col()
        .key("review-diagrams")
        .w_full()
        .gap(crate::ds::SPACE_4);
    if run.diagrams.is_empty() {
        return body.child(note(k, "This change drew no diagram."));
    }
    let palette = palette(k);
    for (at, diagram) in run.diagrams.iter().enumerate() {
        let key = format!("review-diagram-{at}");
        let version = (run.id.to_string(), at, palette.clone());
        let picture = rune::memo(key.clone(), version, || {
            let drawn = crate::diagram_svg::svg(diagram, &palette);
            match &s.images {
                Some(cache) => {
                    let size = drawn.width.max(drawn.height).min(DIAGRAM_PT);
                    let image = cache.svg(drawn.text.into_bytes(), size, 2.0);
                    el().key(key.clone())
                        .aspect_fit(drawn.width / drawn.height.max(1.0))
                        .max_w(drawn.width)
                        .pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
                }
                None => note(k, "This window has no image decoder."),
            }
        });
        body = body
            .child(section(k, diagram.title.clone()))
            .child(picture)
            .child(note(
                k,
                "Export: forgectl review show <id> --diagram <kind> --format mermaid",
            ));
    }
    body
}

fn neighbours(k: &Kit, label: &str, nodes: Option<&Result<Vec<domain::GraphNode>, String>>) -> El {
    let text_value = match nodes {
        None => "reading…".to_owned(),
        Some(Err(error)) => error.clone(),
        Some(Ok(nodes)) if nodes.is_empty() => "none".to_owned(),
        Some(Ok(nodes)) => nodes
            .iter()
            .map(|node| {
                if node.path.is_empty() {
                    node.name.clone()
                } else if node.kind == domain::NodeKind::File {
                    node.path.clone()
                } else {
                    format!("{}:{} {}", node.path, node.line, node.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    };
    note(k, format!("{label}: {text_value}"))
}

fn graph(s: &AppState, k: &Kit) -> El {
    let mut body = col().key("review-graph").w_full().gap(crate::ds::SPACE_3);
    let Some(graph) = &s.code_review.graph else {
        return body.child(note(k, "Reading the codebase graph…"));
    };
    for file in &graph.files {
        body = body
            .child(
                text(file.path.clone())
                    .font(k.ty.mono)
                    .color(k.c.text_base)
                    .truncate(),
            )
            .child(neighbours(k, "imports", file.imports.as_ref()))
            .child(neighbours(k, "external calls", file.external.as_ref()));
    }
    body
}

fn kb_section(s: &AppState, k: &Kit, run: &ReviewRun) -> Option<El> {
    let report = s
        .code_review
        .reports
        .get(&(run.workspace_id, lane(&run.target)))
        .filter(|report| report.run == run.id)?;
    let cited = model::kb_refs(&report.findings);
    if cited.is_empty() {
        return None;
    }
    let workspace = run.workspace_id;
    let open = s.code_review.kb_open;
    let mut body = col()
        .key("review-kb")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .child(
            Button::new(format!(
                "{} Knowledge base · {} cited",
                if open { "▾" } else { "▸" },
                cited.len()
            ))
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| s.toggle_review_kb(workspace))
            .build(k)
            .key("review-kb-toggle"),
        );
    if !open {
        return Some(body);
    }
    let files = s
        .code_review
        .kb
        .as_ref()
        .filter(|(read, _)| *read == workspace)
        .map(|(_, files)| files);
    for (at, reference) in cited.iter().enumerate() {
        let excerpt = match files {
            None => "Reading the knowledge base…".to_owned(),
            Some(Err(error)) => error.clone(),
            Some(Ok(files)) => model::kb_excerpt(files, reference)
                .unwrap_or_else(|| "The knowledge base no longer holds this section.".to_owned()),
        };
        body = body.child(
            col()
                .key(format!("review-kb-{at}"))
                .w_full()
                .gap(crate::ds::SPACE_2)
                .padding(8.0)
                .radius(k.radius.sm)
                .fill(k.c.surface_hover)
                .child(text(reference.clone()).font(k.ty.mono).color(k.c.text_base))
                .child(note(k, excerpt)),
        );
    }
    Some(body)
}

fn sandbox_panel(s: &AppState, k: &Kit, run: &ReviewRun) -> Option<El> {
    run.sandbox_run_id.as_ref()?;
    let mut body = col()
        .key("review-sandbox")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .child(section(k, "Sandbox (TREX)"));
    let read = s
        .code_review
        .sandbox
        .as_ref()
        .filter(|(read, _)| *read == run.id)
        .map(|(_, result)| result);
    let sandbox = match read {
        None => return Some(body.child(note(k, "Reading the sandbox run…"))),
        Some(Err(error)) => return Some(body.child(note(k, error.clone()))),
        Some(Ok(sandbox)) => sandbox,
    };
    if let Some(error) = &sandbox.error {
        body = body.child(note(k, error.clone()));
    }
    for target in &sandbox.targets {
        let outcome = match (target.timed_out, target.exit_code) {
            (true, _) => "timed out".to_owned(),
            (false, Some(0)) => "passed".to_owned(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "killed".to_owned(),
        };
        let mut item = col()
            .key(format!("review-sandbox-{}", target.name))
            .w_full()
            .gap(crate::ds::SPACE_2)
            .child(
                row()
                    .gap(crate::ds::SPACE_3)
                    .items_center()
                    .child(
                        text(target.name.clone())
                            .font(k.ty.small_medium)
                            .color(k.c.text_base),
                    )
                    .child(
                        text(outcome)
                            .font(k.ty.text_xsmall)
                            .color(if target.passed() {
                                k.c.success.fg
                            } else {
                                k.c.danger.fg
                            }),
                    )
                    .child(note(k, format!("{} ms", target.duration_ms))),
            );
        if !target.passed() {
            for output in [&target.stdout, &target.stderr] {
                if !output.trim().is_empty() {
                    item = item.child(
                        text(model::tail(output, MAX_SANDBOX_TAIL).to_owned())
                            .font(k.ty.mono)
                            .color(k.c.text_muted)
                            .wrap(),
                    );
                }
            }
        }
        body = body.child(item);
    }
    Some(body)
}

/// The confirmation a fix loop needs before any writable agent starts.
pub fn fix_loop_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(review_id) = s.code_review.fix_loop_confirm else {
        return;
    };
    let standing = s
        .code_review
        .reports
        .values()
        .find(|report| report.run == review_id)
        .map_or(0, |report| {
            report
                .findings
                .iter()
                .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
                .count()
        });
    crate::modal::Modal::new("review-fix-loop-dialog", "Start a fix loop?")
        .width(crate::modal::WIDTH_PROMPT)
        .on_close(|s: &mut AppState| s.code_review.fix_loop_confirm = None)
        .child(note(
            k,
            format!(
                "Forge sends {standing} finding(s) to a new writable agent, reviews the change \
                 again, and repeats until nothing is found or {FIX_LOOP_ITERATIONS} reviews ran. \
                 The agent edits this checkout; Forge never commits or pushes."
            ),
        ))
        .footer([
            Button::new("Cancel")
                .variant(ButtonVariant::GhostMuted)
                .on_press(|s: &mut AppState| s.code_review.fix_loop_confirm = None)
                .build(k)
                .key("review-fix-loop-cancel"),
            Button::new("Start fix loop")
                .variant(ButtonVariant::Contrast)
                .on_press(|s: &mut AppState| s.start_fix_loop())
                .build(k)
                .key("review-fix-loop-start"),
        ])
        .build(s, k, layer);
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
    let pinned = s.code_review.pinned_to(workspace);
    if lanes.len() > 1 && pinned.is_none() {
        let labels: Vec<String> = lanes.iter().map(|lane| lane_label(*lane)).collect();
        let shown = s.code_review.shown_lane(workspace);
        let at = lanes.iter().position(|lane| *lane == shown);
        top = top.child(
            SegmentedControl::new(&labels, at)
                .label("Review")
                .fit()
                .on_pick(move |s: &mut AppState, index: usize, _| {
                    if let Some(lane) = s.code_review.lanes(workspace).get(index).copied() {
                        s.code_review.shown.insert(workspace, lane);
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
            if let Some(badge) = score_badge(k, run) {
                body = body.child(badge);
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
                ReviewView::Diagrams if completed => body.child(diagrams(s, k, run)),
                ReviewView::Graph if completed => body.child(graph(s, k)),
                _ => body
                    .child(findings(s, k, run))
                    .children(kb_section(s, k, run))
                    .children(sandbox_panel(s, k, run)),
            };
            body = body.child(coverage(k, run));
        }
        None => {
            let empty = match pinned {
                Some(pin) => format!("No review of PR #{} yet.", pin.number),
                None => "No review of this workspace yet.".into(),
            };
            body = body.child(
                text(empty)
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

    pub(super) fn finding(id: u32, path: &str, line: Option<u32>, severity: Severity) -> Finding {
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
            hypothesis: None,
            kb_ref: None,
        }
    }

    pub(super) fn run(id: ReviewId, stage: Option<ReviewStage>, finished: bool) -> ReviewRun {
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
            diagrams: Vec::new(),
            auto_approvable: false,
            swarm_size: 1,
            schema_version: domain::SchemaVersion::V1,
            sandbox_run_id: None,
        }
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

    #[test]
    fn a_failed_report_read_is_not_asked_again() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        state.apply_review_run(&run);
        state.code_review.report_asked.insert(run.id);
        assert!(state.apply_review_report(workspace, Err("the review is gone".into())));
        assert!(
            state.code_review.report_asked.contains(&run.id),
            "the next sync skips it until Refresh or a reconnect"
        );
    }

    #[test]
    fn a_report_read_that_cannot_be_sent_is_not_asked_again() {
        let (mut state, workspace) = fixture_state();
        assert!(state.connection.is_none(), "every send fails");
        let run = completed(workspace, workspace_target(workspace));
        state.apply_review_run(&run);
        state.sync_review();
        state.code_review.errors.clear();
        state.sync_review();
        assert!(state.code_review.report_asked.contains(&run.id));
        assert!(
            !state.code_review.errors.contains_key(&workspace),
            "the second sync did not ask again"
        );
    }

    fn connected(state: &mut AppState) -> flume::Receiver<crate::connection::Command> {
        let (handle, rx) = crate::connection::ConnectionHandle::test_handle(std::sync::Arc::new(
            crate::connection::Metrics::default(),
        ));
        state.connection = Some(handle);
        state.code_review.provider = Some("claude".into());
        rx
    }

    fn list(state: &mut AppState, workspace: WorkspaceId, repository: &str, number: u32) {
        let store = state.store.as_mut().unwrap();
        let project = store
            .workspaces
            .iter()
            .find(|row| row.id == workspace)
            .map(|row| row.project_id);
        store
            .pull_requests
            .pull_requests
            .push(crate::review_subject::listed_pr(
                project, repository, number,
            ));
    }

    fn started(rx: &flume::Receiver<crate::connection::Command>) -> ReviewTarget {
        match rx.try_recv() {
            Ok(crate::connection::Command::StartReview { target, .. }) => target,
            other => panic!("expected StartReview, got {other:?}"),
        }
    }

    #[test]
    fn a_link_in_the_instructions_reviews_that_pull_request_not_the_changes() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        list(&mut state, workspace, "acme/forge", 96);
        state.code_review.instructions =
            TextEdit::new("Review this pr https://github.com/acme/forge/pull/96");
        state.review_instructions_edited();
        assert_eq!(state.code_review.subject, Subject::PullRequest);

        state.start_code_review();
        assert_eq!(
            started(&rx),
            ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: 96,
                base: Some("main".into()),
            }
        );
        assert_eq!(
            state.code_review.shown_lane(workspace),
            Lane::PullRequest(96)
        );

        state.code_review.subject = Subject::Changes;
        state.code_review.instructions.text.push_str(" carefully");
        state.review_instructions_edited();
        assert_eq!(
            state.code_review.subject,
            Subject::Changes,
            "the same link does not undo choosing the changes again"
        );
    }

    #[test]
    fn an_unlisted_link_is_read_before_its_review_starts() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        state.code_review.subject = Subject::PullRequest;
        state.code_review.pull_request = TextEdit::new("https://github.com/acme/forge/pull/5");
        state.start_code_review();
        let Ok(crate::connection::Command::FetchPullRequest(link)) = rx.try_recv() else {
            panic!("expected FetchPullRequest");
        };
        assert_eq!(link.number, 5);
        assert!(state.code_review.awaiting.is_some());

        list(&mut state, workspace, "acme/forge", 5);
        state.review_link_fetched("github.com", "Acme/Forge", 5, None);
        assert!(state.code_review.awaiting.is_none());
        assert!(matches!(
            started(&rx),
            ReviewTarget::PullRequest { number: 5, .. }
        ));
    }

    #[test]
    fn a_bare_number_is_read_from_this_checkout_and_a_bad_one_never_starts() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        state.code_review.subject = Subject::PullRequest;
        state.code_review.pull_request = TextEdit::new("main");
        state.start_code_review();
        assert!(rx.try_recv().is_err());
        assert!(state.code_review.errors.contains_key(&workspace));

        state.code_review.pull_request = TextEdit::new("#12");
        state.start_code_review();
        assert_eq!(
            started(&rx),
            ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: 12,
                base: None,
            }
        );
    }

    #[test]
    fn a_pull_request_opened_for_review_is_pinned_and_starts_only_when_pressed() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        list(&mut state, workspace, "acme/forge", 96);
        state
            .open_pull_request_review("acme/forge", 96)
            .expect("a checkout of acme/forge is open");
        assert!(rx.try_recv().is_err(), "opening it starts nothing");
        assert_eq!(
            state.code_review.shown_lane(workspace),
            Lane::PullRequest(96)
        );

        state.code_review.instructions = TextEdit::new("see https://github.com/acme/forge/pull/7");
        state.review_instructions_edited();
        state.start_code_review();
        assert_eq!(
            started(&rx),
            ReviewTarget::PullRequest {
                workspace_id: workspace,
                number: 96,
                base: Some("main".into()),
            },
            "a link in the instructions never moves a pinned review"
        );

        state.unpin_review();
        assert_eq!(state.code_review.subject, Subject::PullRequest);
        assert_eq!(state.code_review.pull_request.text, "#96");
    }

    #[test]
    fn a_pinned_review_drops_the_choice_of_what_to_review() {
        let (mut state, workspace) = fixture_state();
        list(&mut state, workspace, "acme/forge", 96);
        state
            .open_pull_request_review("acme/forge", 96)
            .expect("a checkout of acme/forge is open");
        let keys = keys(state);
        assert!(has(&keys, "review-pinned"), "{keys:?}");
        assert!(!has(&keys, "review-subject-kind"));
        assert!(has(&keys, "review-presets"));
    }

    #[test]
    fn the_start_stays_busy_until_the_reviewer_session_is_shown() {
        let (mut state, workspace) = fixture_state();
        let _rx = connected(&mut state);
        let reviewer = state
            .rows
            .iter()
            .find(|row| row.workspace_id == workspace)
            .map(|row| row.id)
            .expect("the fixture checkout has a session");
        state.start_code_review();
        assert!(state.code_review.busy(workspace));

        let id = ReviewId::new();
        assert!(state.review_started(workspace, Ok(id)));
        let mut live = run(id, Some(ReviewStage::Pack), false);
        live.workspace_id = workspace;
        live.target = workspace_target(workspace);
        state.apply_review_run(&live);
        assert!(state.code_review.busy(workspace), "no reviewer to show yet");
        assert_eq!(state.workbench.mode, CenterMode::Review);

        live.stage = Some(ReviewStage::Model);
        live.session_id = Some(reviewer);
        state.apply_review_run(&live);
        assert_eq!(state.workbench.mode, CenterMode::Session);
        assert_eq!(state.active, Some(reviewer));
        assert!(
            state.code_review.busy(workspace),
            "a running review of the same target is not started twice"
        );

        live.status = ReviewStatus::Completed;
        live.finished_at = Some(Timestamp::now());
        state.apply_review_run(&live);
        assert!(!state.code_review.busy(workspace));
    }

    #[test]
    fn a_reviewer_that_lands_after_the_person_left_review_does_not_take_focus() {
        let (mut state, workspace) = fixture_state();
        let _rx = connected(&mut state);
        let reviewer = state
            .rows
            .iter()
            .find(|row| row.workspace_id == workspace)
            .map(|row| row.id)
            .expect("the fixture checkout has a session");
        state.start_code_review();
        let id = ReviewId::new();
        assert!(state.review_started(workspace, Ok(id)));
        state.show_mode(CenterMode::Changes);

        let mut live = run(id, Some(ReviewStage::Model), false);
        live.workspace_id = workspace;
        live.target = workspace_target(workspace);
        live.session_id = Some(reviewer);
        state.apply_review_run(&live);
        assert_eq!(state.workbench.mode, CenterMode::Changes);
        assert_eq!(state.code_review.follow, None);
    }

    #[test]
    fn a_completed_run_shows_its_score_badge_and_hypothesis_chips() {
        let (mut state, workspace) = fixture_state();
        let mut run = completed(workspace, workspace_target(workspace));
        run.score = Some(3.5);
        run.verdict = Some(Verdict::Review);
        let mut hunted = finding(1, "a.rs", Some(2), Severity::P1);
        hunted.hypothesis = Some(domain::Hypothesis::RaceCondition);
        with_report(
            &mut state,
            &run,
            vec![hunted, finding(2, "a.rs", Some(4), Severity::P2)],
            vec![],
            None,
        );
        let shown = keys(state);
        assert!(has(&shown, "review-score-badge"), "{shown:?}");
        assert!(has(&shown, "review-hypothesis-1"));
        assert!(
            !has(&shown, "review-hypothesis-2"),
            "a single reviewer's finding carries none"
        );
        assert!(has(&shown, "review-fix-loop"));
    }

    #[test]
    fn the_diagrams_tab_draws_each_diagram_of_the_run() {
        let (mut state, workspace) = fixture_state();
        let mut run = completed(workspace, workspace_target(workspace));
        run.diagrams = vec![domain::Diagram {
            kind: domain::DiagramKind::Flow,
            title: "Branches in pick".into(),
            nodes: vec![domain::DiagramNode {
                id: "start".into(),
                label: "pick".into(),
            }],
            edges: Vec::new(),
        }];
        with_report(&mut state, &run, vec![], vec![], None);
        state.images = Some(rune::ImageCache::new(std::sync::Arc::new(|| {})));
        state.code_review.view = ReviewView::Diagrams;
        let shown = keys(state);
        assert!(has(&shown, "review-diagrams"), "{shown:?}");
        assert!(has(&shown, "review-diagram-0"));
    }

    #[test]
    fn graph_answers_land_on_their_file_and_a_stale_run_is_ignored() {
        let (mut state, workspace) = fixture_state();
        let mut run = completed(workspace, workspace_target(workspace));
        run.coverage.included = vec!["a.rs".into()];
        with_report(&mut state, &run, vec![], vec![], None);
        state.code_review.graph = Some(ReviewGraph {
            run: run.id,
            files: vec![GraphFile {
                path: "a.rs".into(),
                ..GraphFile::default()
            }],
        });
        let node = domain::GraphNode {
            kind: domain::NodeKind::File,
            path: "b.rs".into(),
            name: "b.rs".into(),
            line: 0,
            signature: String::new(),
        };
        assert!(state.apply_graph_nodes(
            run.id,
            "a.rs",
            &domain::GraphQuery::Imports("a.rs".into()),
            Ok(vec![node])
        ));
        assert!(!state.apply_graph_nodes(
            ReviewId::new(),
            "a.rs",
            &domain::GraphQuery::Imports("a.rs".into()),
            Ok(vec![])
        ));
        let graph = state.code_review.graph.as_ref().expect("graph");
        assert!(matches!(&graph.files[0].imports, Some(Ok(nodes)) if nodes.len() == 1));
        assert!(graph.files[0].external.is_none());
        state.code_review.view = ReviewView::Graph;
        let shown = keys(state);
        assert!(has(&shown, "review-graph"), "{shown:?}");
    }

    #[test]
    fn a_cited_knowledge_base_and_a_sandbox_run_have_their_sections() {
        let (mut state, workspace) = fixture_state();
        let mut run = completed(workspace, workspace_target(workspace));
        run.sandbox_run_id = Some("s1".into());
        let mut cited = finding(1, "a.rs", Some(2), Severity::P1);
        cited.kb_ref = Some("architecture.md#crates".into());
        with_report(&mut state, &run, vec![cited], vec![], None);
        state.code_review.kb_open = true;
        state.code_review.kb = Some((
            workspace,
            Ok(vec![domain::KbFile {
                file: "architecture.md".into(),
                content: "# Architecture\n\n## crates\n\n- 3 file(s)\n".into(),
                source: "daemon".into(),
                updated_at: None,
            }]),
        ));
        state.code_review.sandbox = Some((
            run.id,
            Ok(domain::SandboxRun {
                id: "s1".into(),
                workspace_id: workspace,
                review_id: Some(run.id),
                targets: vec![domain::SandboxTarget {
                    name: "test".into(),
                    exit_code: Some(1),
                    duration_ms: 3,
                    timed_out: false,
                    stdout: "1 failed".into(),
                    stderr: String::new(),
                }],
                error: None,
                started_at: domain::Timestamp::now(),
                finished_at: None,
            }),
        ));
        let shown = keys(state);
        assert!(has(&shown, "review-kb"), "{shown:?}");
        assert!(has(&shown, "review-kb-0"));
        assert!(has(&shown, "review-sandbox"));
        assert!(has(&shown, "review-sandbox-test"));
    }

    #[test]
    fn fix_loop_asks_first_and_a_cancel_starts_nothing() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            vec![],
            None,
        );
        state.code_review.fix_loop_confirm = Some(run.id);
        let shown = keys(state);
        assert!(
            has(&shown, "review-fix-loop-dialog")
                || shown
                    .iter()
                    .any(|key| key.starts_with("review-fix-loop-dialog")),
            "{shown:?}"
        );
        assert!(has(&shown, "review-fix-loop-start"));

        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &run, vec![], vec![], None);
        state.code_review.fix_loop_confirm = Some(run.id);
        state.apply_fix_loop(
            workspace,
            FixLoopProgress {
                iteration: 1,
                findings: 0,
                done: true,
                error: None,
            },
        );
        assert!(
            state
                .code_review
                .fix_loops
                .get(&workspace)
                .is_some_and(|p| p.done)
        );
    }
}
