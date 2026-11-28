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
use rune::{El, ScrollSpec, TextEdit, TextStyle, col, el, row, text};
use rune_kit::diff::{Annotation, DiffState, DiffView, LineRef, Side};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Kit, Markdown, Progress, RadioGroup, RadioItem,
    SegmentedControl, Select, TextArea, TextInput,
};

use crate::app::AppState;
use crate::pr_url::PullRequestLink;
use crate::review_subject::{Resolution, Subject};
use crate::workbench::CenterMode;

mod chat;
mod model;

pub use client::CUSTOM_PRESET;
pub use client::MAX_PROMPT_BYTES as PROMPT_BUDGET;
pub use model::FindingFilter;
use model::{
    STAGES, annotations, findings_prompt, group_findings, group_title, is_stale, listed,
    shown_finding, standing_prompt, supersedes,
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
    Summary,
    Diff,
    Diagrams,
    Graph,
}

/// Files of a run whose graph neighbours the Graph tab lists.
const MAX_GRAPH_FILES: usize = 20;
/// Bytes of a sandbox target's output the Sandbox checks section shows.
const MAX_SANDBOX_TAIL: usize = 2048;
const SANDBOX_READING: &str = "Reading the sandbox run…";
const FIX_LOOP_ITERATIONS: u32 = 3;
pub const SANDBOX_CHECKS_PATH: &str = ".forge/review/checks.toml";

/// `hash` is the one the run refused; the daemon refuses its approval once the file stops hashing to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxConfirm {
    pub workspace_id: WorkspaceId,
    pub hash: String,
    /// `None` while the daemon reads it.
    pub file: Option<Result<String, String>>,
}

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
    /// The daemon is building the project's first graph; asked again on `GraphIndexed`.
    pub indexing: bool,
}

impl ReviewGraph {
    fn waiting(&self) -> bool {
        self.indexing
            || self
                .files
                .iter()
                .any(|file| file.imports.is_none() || file.external.is_none())
    }
}

/// The knowledge base read for the run whose findings cite it: a newer run of
/// the same workspace may cite sections the daemon has rewritten since.
#[derive(Clone, Debug)]
pub struct KbRead {
    pub run: ReviewId,
    pub workspace: WorkspaceId,
    /// `None` while the daemon reads it.
    pub files: Option<Result<Vec<domain::KbFile>, String>>,
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

/// The sentence's word whose listbox is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    Subject,
    Focus,
    Depth,
    Agent,
}

/// A lane's chat: an ordinary agent session started with the run's report.
/// Runtime-only, like `Session::action`: the link dies with this window, the
/// session does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewChat {
    pub session: SessionId,
    /// The run the chat was last told about.
    pub run: ReviewId,
}

/// One lane's latest run as the project tree lists it under its checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeReview {
    pub lane: Lane,
    pub title: String,
    pub status: String,
    pub tone: crate::ds::Tone,
    pub chat: Option<SessionId>,
}

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
    pub kb: Option<KbRead>,
    pub kb_open: bool,
    pub sandbox: Option<(ReviewId, Result<domain::SandboxRun, String>)>,
    /// The `checks.toml` hash the daemon acknowledged approving.
    pub sandbox_approved: Option<String>,
    /// Sent for approval, until the daemon answers.
    pub sandbox_approving: Option<String>,
    /// The refused file a person pressed Approve on, read through the daemon to be shown first.
    pub sandbox_confirm: Option<SandboxConfirm>,
    /// The run a person pressed Fix Loop on, until they confirm or cancel.
    pub fix_loop_confirm: Option<ReviewId>,
    pub fix_loops: HashMap<WorkspaceId, FixLoopProgress>,
    /// The four-question form instead of the one-sentence setup.
    pub customizing: bool,
    pub picker: Option<Picker>,
    pub filter: FindingFilter,
    /// The finding the detail pane shows, while the list holds it.
    pub chosen: Option<u32>,
    pub chats: HashMap<Slot, ReviewChat>,
    /// The run whose chat is being started, until its session id is back.
    pub chat_starting: Option<ReviewId>,
    /// The chat session the daemon opened, until its row is listed.
    pub chat_opening: Option<SessionId>,
    pub question: TextEdit,
    /// The finding the next question is about, in the run it was chosen in.
    pub question_about: Option<(ReviewId, u32)>,
    /// Prompts handed to the worker, oldest first; each `SendSessionPrompt`
    /// result settles the front one, since the worker answers in send order.
    pub prompt_sends: std::collections::VecDeque<PromptSend>,
    /// Each chat's conversation as last read; runtime-only, re-read when the
    /// session's activity moves.
    pub logs: HashMap<SessionId, ChatLog>,
    /// The transcript is scrolled far enough from its end to offer a jump back.
    pub chat_far: bool,
}

/// A chat's conversation, read through `GetSessionConversation`.
#[derive(Clone, Debug, Default)]
pub struct ChatLog {
    pub items: Vec<chat::ChatItem>,
    /// False while the daemon has no transcript it can read for the session.
    pub available: bool,
    pub truncated: bool,
    pub error: Option<String>,
    /// The session's activity the last read answered for.
    pub read_for: Option<ChatStamp>,
    /// The activity a read in flight was asked for.
    pub asking: Option<ChatStamp>,
}

/// What moves when a chat's conversation may have grown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChatStamp {
    pub activity: domain::Timestamp,
    pub status: Option<crate::app::SessionStatus>,
    pub running: bool,
}

/// What a sent prompt does once the daemon takes it, or undoes when it refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptSend {
    Findings,
    Question {
        typed: String,
        about: Option<(ReviewId, u32)>,
    },
    Tell {
        slot: Slot,
        session: SessionId,
        run: ReviewId,
    },
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
            sandbox_approved: None,
            sandbox_approving: None,
            sandbox_confirm: None,
            fix_loop_confirm: None,
            fix_loops: HashMap::new(),
            customizing: false,
            picker: None,
            filter: FindingFilter::Open,
            chosen: None,
            chats: HashMap::new(),
            chat_starting: None,
            chat_opening: None,
            question: TextEdit::new(""),
            question_about: None,
            prompt_sends: std::collections::VecDeque::new(),
            logs: HashMap::new(),
            chat_far: false,
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

    /// Each lane's latest run of `workspace`, in lane order, with its chat
    /// when that session is still listed in `rows`.
    #[must_use]
    pub fn tree(&self, workspace: WorkspaceId, rows: &[crate::app::SessionRow]) -> Vec<TreeReview> {
        self.lanes(workspace)
            .iter()
            .filter_map(|lane| {
                let run = self.latest.get(&(workspace, *lane))?;
                let (status, tone) = tree_status(run);
                let chat = self
                    .chats
                    .get(&(workspace, *lane))
                    .map(|chat| chat.session)
                    .filter(|session| rows.iter().any(|row| row.id == *session));
                Some(TreeReview {
                    lane: *lane,
                    title: match lane {
                        Lane::Workspace => "Review · changes".to_owned(),
                        Lane::PullRequest(number) => format!("Review · PR #{number}"),
                    },
                    status,
                    tone,
                    chat,
                })
            })
            .collect()
    }

    /// Whether `session` is some lane's chat, which the tree hangs under its review.
    #[must_use]
    pub fn is_chat(&self, session: SessionId) -> bool {
        self.chats.values().any(|chat| chat.session == session)
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
        Verdict::Block => "Blocks merge",
        Verdict::Review => "Needs review",
        Verdict::Ok => "OK to merge",
    }
}

/// What a tree row says about its lane's latest run, and in which tone.
#[must_use]
pub fn tree_status(run: &ReviewRun) -> (String, crate::ds::Tone) {
    use crate::ds::Tone;
    if running(run) {
        return ("Reviewing".to_owned(), Tone::Info);
    }
    match (run.status, run.verdict) {
        (ReviewStatus::Completed, Some(verdict)) => {
            (verdict_label(verdict).to_owned(), verdict_tone(verdict))
        }
        (ReviewStatus::Completed, None) => ("Done".to_owned(), Tone::Neutral),
        (ReviewStatus::Failed, _) => ("Failed".to_owned(), Tone::Danger),
        (status, _) => (status_label(status).to_owned(), Tone::Neutral),
    }
}

/// Lanes as the rail's row targets spell them.
#[must_use]
pub fn lane_key(lane: Lane) -> String {
    match lane {
        Lane::Workspace => "changes".to_owned(),
        Lane::PullRequest(number) => format!("pr{number}"),
    }
}

#[must_use]
pub fn parse_lane_key(key: &str) -> Option<Lane> {
    if key == "changes" {
        return Some(Lane::Workspace);
    }
    key.strip_prefix("pr")?.parse().ok().map(Lane::PullRequest)
}

/// Shows one lane's review from the project tree.
pub fn open_lane(s: &mut AppState, workspace: WorkspaceId, lane: Lane) {
    s.code_review.shown.insert(workspace, lane);
    open(s, workspace);
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
    s.code_review.kb = None;
    sync(s);
}

/// Replies asked on the old connection are dropped as stale, so what was
/// waiting on one is asked again on the next sync.
pub fn reconnected(s: &mut AppState) {
    s.code_review.starting = false;
    s.code_review.chat_starting = None;
    s.code_review.chat_opening = None;
    s.code_review.prompt_sends.clear();
    for log in s.code_review.logs.values_mut() {
        log.asking = None;
        log.read_for = None;
    }
    s.code_review.asked_config.clear();
    s.code_review.asked_list.clear();
    s.code_review.report_asked.clear();
    if s.code_review.awaiting.take().is_some()
        && let Some(workspace) = s.code_review.workspace
    {
        s.code_review
            .errors
            .insert(workspace, crate::pr_link::LINK_DROPPED.to_owned());
    }
    if s.code_review
        .kb
        .as_ref()
        .is_some_and(|kb| kb.files.is_none())
    {
        s.code_review.kb = None;
    }
    if let Some(confirm) = s
        .code_review
        .sandbox_confirm
        .as_ref()
        .filter(|confirm| confirm.file.is_none())
    {
        let (workspace, hash) = (confirm.workspace_id, confirm.hash.clone());
        s.confirm_sandbox(workspace, hash);
    }
    if s.code_review.diff.as_ref().is_some_and(|diff| !diff.loaded) {
        s.code_review.diff = None;
    }
    if s.code_review
        .graph
        .as_ref()
        .is_some_and(ReviewGraph::waiting)
    {
        s.code_review.graph = None;
    }
    if s.code_review
        .sandbox
        .as_ref()
        .is_some_and(|(_, read)| matches!(read, Err(note) if note == SANDBOX_READING))
    {
        s.code_review.sandbox = None;
    }
}

/// The project's first graph finished building: an indexing read is asked
/// again, or shows why the build failed.
pub fn graph_indexed(s: &mut AppState, error: Option<&str>) {
    let Some(graph) = s.code_review.graph.as_mut().filter(|graph| graph.indexing) else {
        return;
    };
    let Some(error) = error else {
        s.code_review.graph = None;
        sync(s);
        return;
    };
    graph.indexing = false;
    for file in &mut graph.files {
        for read in [&mut file.imports, &mut file.external] {
            read.get_or_insert_with(|| Err(error.to_owned()));
        }
    }
}

/// The finding a pending question is about, only while its run is shown:
/// ids are positions within a run, so another run's same id is another finding.
fn about_finding<'a>(
    about: Option<(ReviewId, u32)>,
    run: &ReviewRun,
    report: &'a Report,
) -> Option<&'a Finding> {
    let (_, id) = about.filter(|(review, _)| *review == run.id)?;
    report.findings.iter().find(|finding| finding.id == id)
}

impl CodeReview {
    /// The full form opened from a run starts on what that run reviewed, not
    /// on whatever the setup last held.
    pub fn customize_for(&mut self, target: &ReviewTarget) {
        match target {
            ReviewTarget::Workspace { .. } => self.subject = Subject::Changes,
            ReviewTarget::PullRequest { number, .. } => {
                self.subject = Subject::PullRequest;
                self.pull_request = TextEdit::new(format!("#{number}"));
            }
            _ => {}
        }
    }

    /// Whether an update to `slot`'s chat is on its way.
    #[must_use]
    pub fn telling(&self, slot: Slot) -> bool {
        self.prompt_sends
            .iter()
            .any(|sent| matches!(sent, PromptSend::Tell { slot: at, .. } if *at == slot))
    }
}

/// Ends a chat start once its session's row is listed.
pub fn settle_chat_start(s: &mut AppState) {
    if let Some(session) = s.code_review.chat_opening
        && s.rows.iter().any(|row| row.id == session)
    {
        s.code_review.chat_opening = None;
    }
}

/// The project tree shows every checkout's reviews, so each checkout's runs
/// are read once per connection, when the store arrives or changes.
pub fn list_every_checkout(s: &mut AppState) {
    settle_chat_start(s);
    let (Some(store), Some(connection)) = (&s.store, &s.connection) else {
        return;
    };
    let generation = s.generation;
    for workspace in store.workspaces.iter().map(|workspace| workspace.id) {
        if s.code_review.asked_list.get(&workspace) == Some(&generation) {
            continue;
        }
        if connection.list_reviews(generation, workspace).is_ok() {
            s.code_review.asked_list.insert(workspace, generation);
        }
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
    // The settings and the runs are read once per connection, not once ever:
    // a restarted daemon interrupts the runs it had without broadcasting it.
    if s.code_review.asked_config.get(&workspace) != Some(&generation) {
        s.code_review.asked_config.insert(workspace, generation);
        let result = match &s.connection {
            Some(connection) => connection.review_config(generation, workspace),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            s.code_review.errors.insert(workspace, error);
        }
    }
    if s.code_review.asked_list.get(&workspace) != Some(&generation) {
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

/// The Graph tab's, the Sandbox checks section's and an open knowledge base's
/// reads, once per shown run. Runs every poll, so the run is borrowed, never cloned.
fn sync_reads(s: &mut AppState, workspace: WorkspaceId, generation: u64) {
    let review = &mut s.code_review;
    let lane = review.shown_lane(workspace);
    let Some(run) = review.latest.get(&(workspace, lane)) else {
        return;
    };
    if run.status != ReviewStatus::Completed {
        return;
    }
    if review.view == ReviewView::Graph
        && review
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
        review.graph = Some(ReviewGraph {
            run: run.id,
            files,
            indexing: false,
        });
    }
    if let Some(id) = &run.sandbox_run_id
        && review
            .sandbox
            .as_ref()
            .is_none_or(|(read, _)| *read != run.id)
        && let Some(connection) = &s.connection
    {
        review.sandbox = Some((run.id, Err(SANDBOX_READING.into())));
        let _ = connection.get_sandbox_checks(generation, run.id, id.clone());
    }
    let cites_kb = review
        .reports
        .get(&(workspace, lane))
        .filter(|report| report.run == run.id)
        .is_some_and(|report| report.findings.iter().any(|f| f.kb_ref.is_some()));
    if review.kb_open && cites_kb && review.kb.as_ref().is_none_or(|kb| kb.run != run.id) {
        let files = match &s.connection {
            Some(connection) => connection.get_kb(generation, workspace).err().map(Err),
            None => Some(Err("The client worker is still starting.".to_string())),
        };
        review.kb = Some(KbRead {
            run: run.id,
            workspace,
            files,
        });
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
        let Some((provider, preset, instructions)) = self.review_choices(workspace) else {
            return;
        };
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

    /// The shown run's target again, with the sentence's current choices.
    pub fn rerun_code_review(&mut self) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let Some(target) = self
            .code_review
            .shown_run(workspace)
            .map(|run| run.target.clone())
        else {
            return;
        };
        let Some((provider, preset, instructions)) = self.review_choices(workspace) else {
            return;
        };
        self.send_review(
            target,
            Some(preset),
            self.code_review.tier,
            Some(provider),
            instructions,
        );
    }

    pub fn cancel_review_run(&mut self, workspace: WorkspaceId, run: ReviewId) {
        let result = match &self.connection {
            Some(connection) => connection.cancel_review(run),
            None => Err("The client worker is still starting.".to_string()),
        };
        if let Err(error) = result {
            self.code_review.errors.insert(workspace, error);
        }
    }

    /// The provider, preset and instructions a start sends, or `None` with
    /// the reason on screen.
    fn review_choices(
        &mut self,
        workspace: WorkspaceId,
    ) -> Option<(String, String, Option<String>)> {
        let Some(config) = self.code_review.configs.get(&workspace) else {
            self.code_review
                .errors
                .insert(workspace, "Review settings have not been read yet.".into());
            return None;
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
            return None;
        }
        let instructions = (!instructions.is_empty()).then_some(instructions);
        Some((provider?, preset, instructions))
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
                let slot = (workspace, lane(&report.run.target));
                let superseded = self
                    .code_review
                    .latest
                    .get(&slot)
                    .is_some_and(|latest| latest.id != report.run.id);
                // A spec run's G3 may read an older review than the one the Review tab shows.
                if self.spec_review_read(&report.run, &report.findings) && superseded {
                    return true;
                }
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
        // The app's generic "… failed" toast already says why a start failed.
        if label == "CreateReviewChat" && !ok {
            self.code_review.chat_starting = None;
        }
        if matches!(label, "SetFindingFeedback" | "ClearReviewSuppression") {
            self.review_feedback_settled(ok);
        }
        if label == "SendSessionPrompt" {
            self.prompt_settled(ok);
            if ok {
                self.toasts.success(message.to_string(), None);
            }
        }
        if label == "ApproveSandboxChecks" {
            let sent = self.code_review.sandbox_approving.take();
            if ok {
                self.code_review.sandbox_approved = sent;
                self.toasts.success(message.to_string(), None);
            }
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
                self.code_review.diff_state.set_wrap(self.diff_wrap);
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
        result: Result<Option<Vec<domain::GraphNode>>, String>,
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
        let result = match result {
            Ok(Some(nodes)) => Ok(nodes),
            Ok(None) => {
                graph.indexing = true;
                return true;
            }
            Err(error) => Err(error),
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
        match &mut self.code_review.kb {
            Some(kb) if kb.workspace == workspace && kb.files.is_none() => {
                kb.files = Some(result);
                true
            }
            _ => false,
        }
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

    pub fn toggle_review_kb(&mut self) {
        self.code_review.kb_open = !self.code_review.kb_open;
        sync(self);
    }

    /// Opens the confirmation and reads the file it shows through the daemon (ADR-012).
    pub fn confirm_sandbox(&mut self, workspace_id: WorkspaceId, hash: String) {
        let sent = match &self.connection {
            Some(connection) => connection.read_sandbox_checks(self.generation, workspace_id),
            None => Err("not connected to the daemon".to_owned()),
        };
        self.code_review.sandbox_confirm = Some(SandboxConfirm {
            workspace_id,
            hash,
            file: sent.err().map(Err),
        });
    }

    pub fn apply_sandbox_file(
        &mut self,
        workspace_id: WorkspaceId,
        result: Result<String, String>,
    ) -> bool {
        match &mut self.code_review.sandbox_confirm {
            Some(confirm) if confirm.workspace_id == workspace_id && confirm.file.is_none() => {
                confirm.file = Some(result);
                true
            }
            _ => false,
        }
    }

    /// Approves exactly the `checks.toml` the run refused, once the person saw it;
    /// the daemon refuses in turn if the file changed since.
    pub fn approve_sandbox(&mut self) {
        let Some(confirm) = self.code_review.sandbox_confirm.take() else {
            return;
        };
        if !matches!(confirm.file, Some(Ok(_))) {
            self.code_review.sandbox_confirm = Some(confirm);
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        match connection.approve_sandbox_checks(confirm.workspace_id, confirm.hash.clone()) {
            Ok(()) => self.code_review.sandbox_approving = Some(confirm.hash),
            Err(error) => self.toasts.danger("checks.toml was not approved", error),
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
        match result {
            Ok(()) => self
                .code_review
                .prompt_sends
                .push_back(PromptSend::Findings),
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
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

    /// The lane's chat while its session still runs.
    #[must_use]
    pub fn live_chat(&self, slot: Slot) -> Option<ReviewChat> {
        let chat = self.code_review.chats.get(&slot)?;
        self.rows
            .iter()
            .any(|row| row.id == chat.session && row.running)
            .then_some(*chat)
    }

    /// Asks the shown lane's chat the typed question, starting the chat with
    /// the run's report first when it has none running.
    pub fn ask_review_chat(&mut self) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let (Some(run), Some(report)) = (
            self.code_review.shown_run(workspace),
            self.code_review.shown_report(workspace),
        ) else {
            return;
        };
        let slot = (workspace, lane(&run.target));
        let typed = self.code_review.question.text.trim().to_owned();
        let about = self.code_review.question_about;
        let question = match about_finding(about, run, report) {
            Some(finding) if !typed.is_empty() => model::about(finding, &typed),
            _ => model::head(&typed, model::QUESTION_BYTES).to_owned(),
        };
        let Some(chat) = self.live_chat(slot) else {
            self.start_review_chat((!question.is_empty()).then_some(question));
            return;
        };
        if question.is_empty() {
            return;
        }
        let result = match &self.connection {
            Some(connection) => connection.send_session_prompt(chat.session, question, 0),
            None => Err("The client worker is still starting.".to_string()),
        };
        match result {
            Ok(()) => {
                self.code_review.question = TextEdit::new("");
                self.code_review.question_about = None;
                self.code_review
                    .prompt_sends
                    .push_back(PromptSend::Question { typed, about });
            }
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
    }

    /// Settles the oldest prompt sent: a refused question comes back into an
    /// empty box, and a refused update leaves the chat marked as behind.
    fn prompt_settled(&mut self, ok: bool) {
        let Some(sent) = self.code_review.prompt_sends.pop_front() else {
            return;
        };
        match sent {
            PromptSend::Findings => {}
            PromptSend::Question { typed, about } => {
                if !ok && self.code_review.question.text.trim().is_empty() {
                    self.code_review.question = TextEdit::new(&typed);
                    self.code_review.question_about = about;
                }
            }
            PromptSend::Tell { slot, session, run } => {
                if let Some(chat) = self.code_review.chats.get_mut(&slot)
                    && chat.session == session
                    && ok
                {
                    chat.run = run;
                }
            }
        }
    }

    /// A new read-only chat for the shown run: an ordinary agent session whose
    /// first prompt is the report, closed by the read-only rules.
    pub fn start_review_chat(&mut self, question: Option<String>) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        if self.code_review.chat_starting.is_some() {
            return;
        }
        let (Some(run), Some(report)) = (
            self.code_review.shown_run(workspace),
            self.code_review.shown_report(workspace),
        ) else {
            return;
        };
        let subject = match lane(&run.target) {
            Lane::Workspace => "the changes in this checkout".to_owned(),
            Lane::PullRequest(number) => format!(
                "pull request #{number}, which is not checked out here (read it with \
                 `gh pr view {number}` and `gh pr diff {number}`)"
            ),
        };
        let prompt = model::chat_prompt(&subject, run, &report.findings, question.as_deref());
        let review = run.id;
        let (provider, profile) = match self.prompt_agent() {
            Ok(agent) => agent,
            Err(reason) => {
                self.code_review.errors.insert(workspace, reason);
                return;
            }
        };
        let result = match &self.connection {
            Some(connection) => {
                connection.create_review_chat(workspace, provider, profile, prompt, review)
            }
            None => Err("The client worker is still starting.".to_string()),
        };
        match result {
            Ok(()) => {
                self.code_review.chat_starting = Some(review);
                self.code_review.question = TextEdit::new("");
                self.code_review.question_about = None;
            }
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
    }

    /// The daemon's id for the chat `review` asked for. The start stays busy
    /// until the session's row is listed: in between the chat would read as
    /// ended and a second press would open another one.
    pub fn review_chat_opened(&mut self, review: ReviewId, session: SessionId) -> bool {
        if self.code_review.chat_starting == Some(review) {
            self.code_review.chat_opening = Some(session);
            self.code_review.chat_starting = None;
            settle_chat_start(self);
        }
        let slot = self
            .code_review
            .latest
            .iter()
            .find(|(_, run)| run.id == review)
            .map(|(slot, _)| *slot)
            .or_else(|| {
                self.code_review
                    .reports
                    .iter()
                    .find(|(_, report)| report.run == review)
                    .map(|(slot, _)| *slot)
            });
        if let Some(slot) = slot {
            self.code_review.chats.insert(
                slot,
                ReviewChat {
                    session,
                    run: review,
                },
            );
        }
        true
    }

    /// The chat the Review's findings page shows, while its session is listed.
    fn shown_chat_session(&self) -> Option<SessionId> {
        if self.workbench.mode != crate::workbench::CenterMode::Review
            || self.code_review.view != ReviewView::Findings
        {
            return None;
        }
        let workspace = self.code_review.workspace?;
        let run = self.code_review.shown_run(workspace)?;
        let chat = self
            .code_review
            .chats
            .get(&(workspace, lane(&run.target)))?;
        self.rows
            .iter()
            .any(|row| row.id == chat.session)
            .then_some(chat.session)
    }

    fn chat_stamp(&self, session: SessionId) -> Option<ChatStamp> {
        let row = self.rows.iter().find(|row| row.id == session)?;
        let activity = self
            .store
            .as_ref()?
            .sessions
            .iter()
            .find(|known| known.id == session)?
            .last_activity_at;
        Some(ChatStamp {
            activity,
            status: row.status,
            running: row.running,
        })
    }

    /// Re-reads the shown chat's conversation once per activity bump, which
    /// the daemon throttles; a read in flight is never doubled.
    pub fn poll_review_chat(&mut self) {
        let Some(session) = self.shown_chat_session() else {
            return;
        };
        let Some(stamp) = self.chat_stamp(session) else {
            return;
        };
        let log = self.code_review.logs.entry(session).or_default();
        if log.asking.is_some() || log.read_for == Some(stamp) {
            return;
        }
        let sent = self.connection.as_ref().is_some_and(|connection| {
            connection
                .session_conversation(self.generation, session)
                .is_ok()
        });
        if sent {
            log.asking = Some(stamp);
        }
    }

    pub fn apply_session_conversation(
        &mut self,
        session: SessionId,
        result: Result<domain::SessionConversation, String>,
    ) -> bool {
        let Some(log) = self.code_review.logs.get_mut(&session) else {
            return false;
        };
        let Some(asked) = log.asking.take() else {
            return false;
        };
        log.read_for = Some(asked);
        match result {
            Ok(conversation) => {
                log.items = chat::items(&conversation.turns);
                log.available = conversation.available;
                log.truncated = conversation.truncated;
                log.error = None;
            }
            Err(error) => log.error = Some(error),
        }
        true
    }

    /// Queues the shown run's delta into a chat that still talks about an earlier run.
    pub fn tell_review_chat(&mut self) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let (Some(run), Some(report)) = (
            self.code_review.shown_run(workspace),
            self.code_review.shown_report(workspace),
        ) else {
            return;
        };
        let slot = (workspace, lane(&run.target));
        let Some(chat) = self.live_chat(slot).filter(|chat| chat.run != run.id) else {
            return;
        };
        if self.code_review.telling(slot) {
            return;
        }
        let update = model::chat_update(run, &report.findings, &report.resolved);
        let review = run.id;
        let result = match &self.connection {
            Some(connection) => connection.send_session_prompt(chat.session, update, 0),
            None => Err("The client worker is still starting.".to_string()),
        };
        match result {
            Ok(()) => self.code_review.prompt_sends.push_back(PromptSend::Tell {
                slot,
                session: chat.session,
                run: review,
            }),
            Err(error) => {
                self.code_review.errors.insert(workspace, error);
            }
        }
    }

    /// Points the next question at one finding of the shown run and moves the
    /// keyboard there.
    pub fn ask_about_finding(&mut self, finding: u32) {
        let Some(run) = self
            .code_review
            .workspace
            .and_then(|workspace| self.code_review.shown_run(workspace))
        else {
            return;
        };
        self.code_review.question_about = Some((run.id, finding));
        self.code_review.chosen = Some(finding);
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

fn full_form(s: &AppState, k: &Kit) -> El {
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
                &["Default tier", "Quick", "Standard", "Deep"],
                tier_index(s.code_review.tier),
            )
            .label("Tier")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                s.code_review.tier = match index {
                    1 => Some(Tier::Quick),
                    2 => Some(Tier::Standard),
                    3 => Some(Tier::Deep),
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
    body = body.children(start_notices(s, k, workspace));
    let ready = ready_to_start(s, workspace, false);
    let actions = row()
        .w_full()
        .gap(crate::ds::SPACE_4)
        .items_center()
        .child(
            Button::new("Fewer options")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| s.code_review.customizing = false)
                .build(k)
                .key("review-fewer"),
        )
        .child(el().grow(1.0))
        .children(busy_mark(s, k, workspace));
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

/// Whether the start button can send: `rerun` starts the shown run's target
/// again, so the what-to-review choice does not apply.
fn ready_to_start(s: &AppState, workspace: WorkspaceId, rerun: bool) -> bool {
    let review = &s.code_review;
    let custom = review.preset.as_deref() == Some(CUSTOM_PRESET);
    let busy = if rerun {
        review.starting
            || review.follow.is_some()
            || review.shown_run(workspace).is_some_and(running)
    } else {
        review.busy(workspace)
    };
    review.configs.contains_key(&workspace)
        && !review.offered.is_empty()
        && !busy
        && (!custom || !review.instructions.text.trim().is_empty())
        && (rerun
            || review.pinned_to(workspace).is_some()
            || review.subject == Subject::Changes
            || !review.pull_request.text.trim().is_empty())
}

/// A review runs for minutes: the vsync `Loader` would repaint the window all along.
fn busy_mark(s: &AppState, k: &Kit, workspace: WorkspaceId) -> Option<El> {
    if !s.code_review.busy(workspace) {
        return None;
    }
    Some(if s.reduce_motion() {
        crate::ui::busy(s, k)
    } else {
        crate::ds::working(14.0, k.c.icon_muted).key("review-busy")
    })
}

/// The preset the start sends: the one chosen, else the repository's default.
fn chosen_preset<'a>(
    review: &CodeReview,
    config: &'a ReviewSettings,
) -> Option<&'a domain::ReviewPreset> {
    let id = review
        .preset
        .as_deref()
        .unwrap_or(config.default_preset.as_str());
    config.presets.iter().find(|preset| preset.id == id)
}

fn depth_word(tier: Option<Tier>) -> &'static str {
    match tier {
        None => "default",
        Some(Tier::Quick) => "quick",
        Some(Tier::Standard) => "standard",
        Some(Tier::Deep) => "deep",
    }
}

const DEPTHS: [&str; 4] = ["default", "quick", "standard", "deep"];

/// The setup as one sentence, each choice a listbox. `rerun` is the shown run:
/// its target is fixed and the button starts it again.
fn sentence(
    s: &AppState,
    k: &Kit,
    layer: &mut Vec<El>,
    workspace: WorkspaceId,
    rerun: Option<&ReviewRun>,
) -> El {
    let review = &s.code_review;
    let Some(config) = review.configs.get(&workspace) else {
        return note(k, "Reading review settings…");
    };
    let word = |value: &str| {
        text(value.to_owned())
            .font(k.ty.small)
            .color(k.c.text_muted)
    };
    let mut line = row()
        .key("review-sentence")
        .role("group")
        .label("New review")
        .w_full()
        .flex_wrap()
        .gap(crate::ds::SPACE_3)
        .items_center();
    match (rerun, review.pinned_to(workspace)) {
        (Some(_), _) => {
            line = line.child(word("Reviewed"));
        }
        (None, Some(pin)) => {
            line = line.child(word("Review")).child(
                text(format!("PR #{} · {}", pin.number, pin.title))
                    .key("review-pinned")
                    .font(k.ty.small_medium)
                    .color(k.c.text_base)
                    .truncate()
                    .max_w(360.0),
            );
        }
        (None, None) => {
            let subjects = ["my changes", "a pull request"];
            let current = match review.subject {
                Subject::Changes => subjects[0],
                Subject::PullRequest => subjects[1],
            };
            line = line.child(word("Review")).child(
                Select::new("review-word-subject", &subjects)
                    .label("What to review")
                    .current(Some(current))
                    .details(&[
                        Some("The working tree against its base branch, uncommitted changes included."),
                        Some("Its head against its base, fetched without moving this checkout."),
                    ])
                    .width(150.0)
                    .open(review.picker == Some(Picker::Subject))
                    .on_toggle(|s: &mut AppState| toggle_picker(s, Picker::Subject))
                    .on_close(|s: &mut AppState| s.code_review.picker = None)
                    .on_choose(|s: &mut AppState, choice, _| {
                        s.code_review.picker = None;
                        s.code_review.subject = if choice == "a pull request" {
                            Subject::PullRequest
                        } else {
                            Subject::Changes
                        };
                    })
                    .build(k, layer),
            );
            if review.subject == Subject::PullRequest {
                line = line.child(
                    TextInput::new(&review.pull_request)
                        .label("Pull request")
                        .placeholder("96, #96 or a link")
                        .width(Some(200.0))
                        .on_edit(|s: &mut AppState| &mut s.code_review.pull_request)
                        .on_submit(|s: &mut AppState, _| s.start_code_review())
                        .build(k)
                        .key("review-pull-request"),
                );
            }
        }
    }
    let preset = chosen_preset(review, config);
    let names: Vec<String> = config
        .presets
        .iter()
        .map(|preset| preset.name.clone())
        .collect();
    let details: Vec<Option<String>> = config
        .presets
        .iter()
        .map(|preset| {
            let detail = if preset.id == CUSTOM_PRESET && preset.detail.is_empty() {
                "Your own words, written below."
            } else {
                preset.detail.as_str()
            };
            (!detail.is_empty()).then(|| detail.to_owned())
        })
        .collect();
    let ids: Vec<(String, String)> = config
        .presets
        .iter()
        .map(|preset| (preset.name.clone(), preset.id.clone()))
        .collect();
    line = line.child(word("for")).child(
        Select::new("review-word-focus", &names)
            .label("Focus")
            .current(preset.map(|preset| preset.name.as_str()))
            .details(&details)
            .width(180.0)
            .open(review.picker == Some(Picker::Focus))
            .on_toggle(|s: &mut AppState| toggle_picker(s, Picker::Focus))
            .on_close(|s: &mut AppState| s.code_review.picker = None)
            .on_choose(move |s: &mut AppState, name, _| {
                s.code_review.picker = None;
                if let Some((_, id)) = ids.iter().find(|(known, _)| known == name) {
                    s.code_review.preset = Some(id.clone());
                }
            })
            .build(k, layer),
    );
    let forced = preset.and_then(|preset| preset.tier);
    let depth = Select::new("review-word-depth", &DEPTHS)
        .label("Depth")
        .current(Some(depth_word(forced.or(review.tier))))
        .details(&[
            Some("The repository's default tier, else Forge's."),
            None,
            None,
            None,
        ])
        .width(110.0)
        .disabled(forced.is_some())
        .open(review.picker == Some(Picker::Depth) && forced.is_none())
        .on_toggle(|s: &mut AppState| toggle_picker(s, Picker::Depth))
        .on_close(|s: &mut AppState| s.code_review.picker = None)
        .on_choose(|s: &mut AppState, choice, _| {
            s.code_review.picker = None;
            s.code_review.tier = match choice {
                "quick" => Some(Tier::Quick),
                "standard" => Some(Tier::Standard),
                "deep" => Some(Tier::Deep),
                _ => None,
            };
        })
        .build(k, layer);
    line = line.child(word("at")).child(depth).child(word("depth"));
    let agents: Vec<String> = review
        .offered
        .iter()
        .map(|(_, name)| name.clone())
        .collect();
    let current_agent = review
        .offered
        .iter()
        .find(|(id, _)| review.provider.as_deref() == Some(id.as_str()))
        .or_else(|| review.offered.first())
        .map(|(_, name)| name.as_str());
    if !agents.is_empty() {
        let offered = review.offered.clone();
        line = line.child(word("with")).child(
            Select::new("review-word-agent", &agents)
                .label("Agent")
                .current(current_agent)
                .width(150.0)
                .open(review.picker == Some(Picker::Agent))
                .on_toggle(|s: &mut AppState| toggle_picker(s, Picker::Agent))
                .on_close(|s: &mut AppState| s.code_review.picker = None)
                .on_choose(move |s: &mut AppState, name, _| {
                    s.code_review.picker = None;
                    if let Some((id, _)) = offered.iter().find(|(_, known)| known == name) {
                        s.code_review.provider = Some(id.to_string());
                    }
                })
                .build(k, layer),
        );
    }
    let customize_target = rerun.map(|run| run.target.clone());
    let customize = Button::new("Customize…")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| {
            if let Some(target) = &customize_target {
                s.code_review.customize_for(target);
            }
            s.code_review.customizing = true;
            s.code_review.picker = None;
        })
        .build(k)
        .key("review-customize");
    let start = match rerun {
        Some(_) => Button::new(if review.starting {
            "Starting…"
        } else {
            "Run again"
        })
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Small)
        .disabled(!ready_to_start(s, workspace, true))
        .on_press(|s: &mut AppState| s.rerun_code_review())
        .build(k)
        .key("review-run-again"),
        None => Button::new(start_label(review, workspace))
            .variant(ButtonVariant::Contrast)
            .disabled(!ready_to_start(s, workspace, false))
            .on_press(|s: &mut AppState| s.start_code_review())
            .build(k)
            .key("review-start"),
    };
    // After a run the sentence is one line with its two buttons at the end;
    // the focus is explained only once a different one is picked.
    let changed_focus =
        rerun.is_some_and(|run| preset.is_some_and(|preset| preset.id != run.preset));
    let mut buttons = Some((customize, start));
    if let Some(run) = rerun
        && let Some((customize, start)) = buttons.take()
    {
        line = line
            .child(el().grow(1.0))
            .children(busy_mark(s, k, run.workspace_id))
            .child(customize)
            .child(start);
    }
    let mut body = col()
        .key(if rerun.is_some() {
            "review-rerun"
        } else {
            "review-setup"
        })
        .w_full()
        .gap(crate::ds::SPACE_3)
        .child(line);
    if let Some(preset) = preset.filter(|preset| !preset.detail.is_empty())
        && (rerun.is_none() || changed_focus)
    {
        body = body.child(
            note(k, format!("{}: {}", preset.name, preset.detail)).key("review-focus-detail"),
        );
    }
    if review.offered.is_empty() {
        body = body.child(note(k, "No installed agent can run a Forge Review."));
    }
    if let (Some(preset), Some(tier)) = (preset, forced) {
        body = body.child(note(
            k,
            format!(
                "{} always runs at {} depth.",
                preset.name,
                depth_word(Some(tier))
            ),
        ));
    }
    let custom = preset.is_some_and(|preset| preset.id == CUSTOM_PRESET);
    if rerun.is_none() || custom {
        body = body.child(
            TextArea::new(&review.instructions)
                .label(if custom {
                    "What should the reviewer look for?"
                } else {
                    "Anything to look at in particular? (optional)"
                })
                .placeholder("e.g. the liveness probe and Redis failure modes")
                .on_edit(|s: &mut AppState| &mut s.code_review.instructions)
                .on_change(|s: &mut AppState, _| s.review_instructions_edited())
                .build(k)
                .key("review-instructions")
                .h(56.0),
        );
    }
    body = body.children(start_notices(s, k, workspace));
    let mut extras = row().w_full().gap(crate::ds::SPACE_4).items_center();
    let mut any_extra = false;
    if rerun.is_none() && !config.rules.is_empty() {
        let enabled = config.rules.iter().filter(|rule| rule.enabled).count();
        extras = extras.child(note(
            k,
            format!(
                "{} repository rules apply · {enabled} enabled",
                config.rules.len()
            ),
        ));
        any_extra = true;
    }
    for error in &config.errors {
        extras = extras.child(
            text(error.clone())
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg)
                .truncate()
                .min_w(0.0),
        );
        any_extra = true;
    }
    if let Some((customize, start)) = buttons {
        return body.child(
            extras
                .child(customize)
                .child(el().grow(1.0))
                .children(busy_mark(s, k, workspace))
                .child(start),
        );
    }
    if any_extra {
        body = body.child(extras);
    }
    body
}

fn toggle_picker(s: &mut AppState, picker: Picker) {
    s.code_review.picker = (s.code_review.picker != Some(picker)).then_some(picker);
}

/// What the daemon would decide about a start, said before it is pressed.
fn start_notices(s: &AppState, k: &Kit, workspace: WorkspaceId) -> Vec<El> {
    let review = &s.code_review;
    let mut notices = Vec::new();
    let custom = review.preset.as_deref() == Some(CUSTOM_PRESET);
    if custom && review.instructions.text.trim().is_empty() {
        notices.push(
            text("Custom needs your words: write what the reviewer should look for, or pick another focus.")
                .key("review-custom-needs-words")
                .role("alert")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg)
                .wrap(),
        );
    }
    let now = Timestamp::now().as_offset().unix_timestamp();
    let target = review.target_lane(workspace);
    let shown = review.shown_lane(workspace);
    if let Some(lane) = target.filter(|lane| *lane != shown)
        && let Some(run) = review
            .latest
            .get(&(workspace, lane))
            .filter(|run| running(run))
    {
        let (id, session) = (run.id, run.session_id);
        let mut line = row()
            .key("review-running-notice")
            .role("status")
            .w_full()
            .gap(crate::ds::SPACE_3)
            .items_center()
            .child(
                text(format!(
                    "A review of {} is running · {} · {}. Start is off until it ends; \
                     cancel it to start one with other settings.",
                    lane_label(lane),
                    stage_label(run.stage),
                    duration(run.created_at, None, now)
                ))
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg)
                .wrap()
                .min_w(0.0)
                .grow(1.0),
            );
        if let Some(session) = session {
            line = line.child(
                Button::new("Show the reviewer")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut AppState| s.focus_session(session))
                    .build(k)
                    .key("review-running-show"),
            );
        }
        notices.push(
            line.child(
                Button::new("Cancel it")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut AppState| s.cancel_review_run(workspace, id))
                    .build(k)
                    .key("review-running-cancel"),
            ),
        );
    }
    let mut others: Vec<(&Slot, &ReviewRun)> = review
        .latest
        .iter()
        .filter(|(slot, run)| {
            running(run)
                && **slot != (workspace, shown)
                && Some(**slot) != target.map(|lane| (workspace, lane))
        })
        .collect();
    if !others.is_empty() {
        others.sort_by_key(|(_, run)| run.created_at);
        let mut list = col()
            .key("review-others-running")
            .w_full()
            .gap(crate::ds::SPACE_1)
            .child(note(
                k,
                "Also running; Forge runs a bounded number of reviews at once:",
            ));
        for ((other, lane), run) in others {
            let place = s
                .store
                .as_ref()
                .and_then(|store| store.workspaces.iter().find(|w| w.id == *other))
                .map_or_else(String::new, |w| w.label());
            let (other, id) = (*other, run.id);
            list = list.child(
                row()
                    .w_full()
                    .gap(crate::ds::SPACE_3)
                    .items_center()
                    .child(
                        text(format!(
                            "{place} · {} · {}",
                            lane_label(*lane),
                            duration(run.created_at, None, now)
                        ))
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_muted)
                        .truncate()
                        .min_w(0.0)
                        .grow(1.0),
                    )
                    .child(
                        Button::new("Cancel")
                            .variant(ButtonVariant::GhostMuted)
                            .control_size(ControlSize::Small)
                            .on_press(move |s: &mut AppState| s.cancel_review_run(other, id))
                            .build(k)
                            .key(format!("review-other-cancel-{id}")),
                    ),
            );
        }
        notices.push(list);
    }
    notices
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
        Some(Tier::Quick) => Some(1),
        Some(Tier::Standard) => Some(2),
        Some(Tier::Deep) => Some(3),
    }
}

/// The run's facts, each under its own name, so none has to be guessed.
fn run_header(s: &AppState, k: &Kit, run: &ReviewRun, head: Option<&str>) -> El {
    let fact = |label: &str, value: El| {
        row()
            .gap(crate::ds::SPACE_2)
            .items_center()
            .child(
                text(label.to_owned())
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted),
            )
            .child(value)
    };
    let value = |value: String| text(value).font(k.ty.text_xsmall).color(k.c.text_base);
    let focus = s
        .code_review
        .configs
        .get(&run.workspace_id)
        .and_then(|config| config.presets.iter().find(|preset| preset.id == run.preset))
        .map_or_else(|| run.preset.clone(), |preset| preset.name.clone());
    let took = duration(
        run.created_at,
        run.finished_at,
        Timestamp::now().as_offset().unix_timestamp(),
    );
    let mut facts = row()
        .key("review-run-head")
        .w_full()
        .flex_wrap()
        .gap(crate::ds::SPACE_5)
        .items_center()
        .child(fact(
            "Reviewed",
            value(target_label(&run.target)).key("review-target"),
        ))
        .child(fact("Focus", value(focus)))
        .child(fact("Depth", value(run.tier.name().to_owned())))
        .child(fact("Agent", value(run.provider.clone())))
        .child(fact(
            "Head",
            text(run.head_oid.chars().take(7).collect::<String>())
                .font(k.ty.mono)
                .color(k.c.text_base),
        ))
        .child(fact(
            if running(run) { "Running for" } else { "Took" },
            value(took),
        ));
    if run.verdict.is_none() {
        facts = facts.child(fact(
            "Status",
            text(status_label(run.status))
                .font(k.ty.text_xsmall)
                .color(if running(run) {
                    k.c.warning.fg
                } else {
                    k.c.text_base
                }),
        ));
    }
    if is_stale(run, head) {
        facts = facts.child(
            text("stale: the checkout moved since")
                .key("review-stale")
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg),
        );
    }
    facts
}

/// The verdict once, in words and colour, with what blocks the merge beside it
/// and the run's own facts at the end: one line, so the findings start high.
fn verdict_banner(
    k: &Kit,
    run: &ReviewRun,
    report: Option<&Report>,
    head: Option<&str>,
) -> Option<El> {
    if run.verdict.is_none() && report.is_none() {
        return None;
    }
    let approvable = if run.auto_approvable {
        "auto-approvable"
    } else {
        "needs a person"
    };
    let tone = run.verdict.map_or(crate::ds::Tone::Neutral, verdict_tone);
    let ink = tone.ink(k, &crate::theme::roles(k));
    let mut banner = row()
        .key("review-banner")
        .role("status")
        .w_full()
        .flex_wrap()
        .gap(crate::ds::SPACE_4)
        .items_center()
        .px(12.0)
        .py(8.0)
        .radius(k.radius.md)
        .fill(ink.soft);
    if let Some(verdict) = run.verdict {
        banner = banner
            .child(
                text(verdict_label(verdict))
                    .key("review-verdict")
                    .font(k.ty.ui_medium)
                    .color(verdict_ink(k, verdict)),
            )
            .child(
                crate::ds::chip(
                    k,
                    &crate::theme::roles(k),
                    format!("Score {}", score_label(run.score)),
                    tone,
                )
                .key("review-score-badge")
                .label(format!(
                    "Score {} · {} · {approvable}",
                    score_label(run.score),
                    verdict_label(verdict)
                )),
            )
            .child(note(k, approvable));
    }
    if let Some(report) = report {
        banner = banner.child(headline(k, report).min_w(0.0).grow(1.0));
    } else {
        banner = banner.child(el().grow(1.0));
    }
    let took = duration(
        run.created_at,
        run.finished_at,
        Timestamp::now().as_offset().unix_timestamp(),
    );
    let short: String = run.head_oid.chars().take(7).collect();
    banner = banner.child(
        text(format!("{short} · {} · {took}", run.provider))
            .key("review-banner-facts")
            .font(k.ty.small)
            .color(k.c.text_muted),
    );
    if is_stale(run, head) {
        banner = banner.child(
            text("stale: the checkout moved since")
                .key("review-stale")
                .font(k.ty.small)
                .color(k.c.warning.fg),
        );
    }
    Some(banner)
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
        .key(format!("review-finding-detail-{}", finding.id))
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
    place: Option<(usize, usize)>,
) -> El {
    if finding.reaction == Some(FindingReaction::Dismiss) {
        return dismissed_row(k, finding);
    }
    let id = finding.id;
    let pull_request = is_pull_request(run);
    let open = Button::new(format!("{} ↗", place_label(finding)))
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .help(if pull_request {
            "Show in the reviewed diff (o)"
        } else {
            "Open in the editor (o)"
        })
        .on_press(move |s: &mut AppState| s.open_finding(id))
        .build(k)
        .key(format!("review-finding-{}", finding.id));
    let roles = crate::theme::roles(k);
    let blocks = matches!(finding.severity, Severity::P0 | Severity::P1);
    let mut head = row()
        .w_full()
        .flex_wrap()
        .gap(crate::ds::SPACE_4)
        .items_center()
        .child(crate::ds::chip(
            k,
            &roles,
            format!(
                "{:?} · {}",
                finding.severity,
                if blocks {
                    "blocks merge"
                } else {
                    "worth fixing"
                }
            ),
            if blocks {
                crate::ds::Tone::Danger
            } else {
                crate::ds::Tone::Warning
            },
        ));
    if lineage {
        head = head.child(lineage_badge(k, finding));
    }
    if let Some(hypothesis) = finding.hypothesis {
        head = head.child(
            crate::ds::chip(k, &roles, hypothesis.name(), crate::ds::Tone::Info)
                .key(format!("review-hypothesis-{}", finding.id)),
        );
    }
    head = head
        .child(note(k, finding.category.clone()))
        .children(
            finding
                .rule_id
                .as_ref()
                .map(|rule| note(k, format!("rule: {rule}"))),
        )
        .child(note(k, format!("{:.0}% sure", finding.confidence * 100.0)))
        .child(el().grow(1.0))
        .children(place.map(|(at, of)| {
            text(format!("{at} of {of}"))
                .key("review-finding-place")
                .font(k.ty.small)
                .color(k.c.text_faint)
        }));
    let label = |name: &str| {
        text(name.to_owned())
            .font(k.ty.ui)
            .color(k.c.text_muted)
            .w(72.0)
    };
    let fact = |name: &str, value: El| {
        row()
            .w_full()
            .gap(crate::ds::SPACE_5)
            .items_start()
            .child(label(name))
            .child(value.min_w(0.0).grow(1.0))
    };
    let mut item = col()
        .key(format!("review-finding-detail-{}", finding.id))
        .role("region")
        .label(format!("Finding {}", finding.id))
        .w_full()
        .gap(crate::ds::SPACE_5)
        .child(head)
        .child(
            text(finding.title.trim().to_owned())
                .role("heading")
                .font(TextStyle {
                    size: 17.0,
                    line_height: 24.0,
                    ..k.ty.ui_medium
                })
                .color(k.c.text_base)
                .wrap(),
        )
        .child(
            text(finding.body.trim().to_owned())
                .font(k.ty.ui)
                .color(k.c.text_muted)
                .wrap(),
        );
    if let Some(reach) = &finding.reachability {
        item = item.child(
            fact(
                "Trigger",
                row()
                    .gap(crate::ds::SPACE_3)
                    .items_start()
                    .child(crate::ds::chip(
                        k,
                        &roles,
                        reach.kind.name(),
                        crate::ds::Tone::Neutral,
                    ))
                    .child(
                        text(reach.trigger.clone())
                            .font(k.ty.ui)
                            .color(k.c.text_base)
                            .wrap()
                            .min_w(0.0)
                            .grow(1.0),
                    ),
            )
            .key(format!("review-reachability-{}", finding.id)),
        );
    }
    if let Some(suggestion) = &finding.suggestion {
        item = item.child(fact(
            "Fix",
            text(suggestion.trim().to_owned())
                .font(k.ty.ui)
                .color(k.c.text_base)
                .wrap(),
        ));
    }
    item = item.child(fact("Where", row().child(open)));
    if !finding.evidence.trim().is_empty() {
        item = item.child(
            col()
                .key(format!("review-evidence-{}", finding.id))
                .w_full()
                .padding(10.0)
                .radius(k.radius.sm)
                .fill(k.c.bg_deep)
                .border(1.0, k.c.border_weak)
                .child(
                    text(finding.evidence.trim().to_owned())
                        .font(k.ty.mono)
                        .color(k.c.text_base)
                        .wrap(),
                ),
        );
    }
    let mut actions = row()
        .w_full()
        .flex_wrap()
        .gap(crate::ds::SPACE_3)
        .items_center()
        .pt(crate::ds::SPACE_4)
        .child(
            Button::new("Ask in review chat")
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Small)
                .help("Point the review chat's next question at this finding (a)")
                .on_press(move |s: &mut AppState| s.ask_about_finding(id))
                .build(k)
                .key(format!("review-ask-{id}")),
        )
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
        .child(el().grow(1.0));
    if !pull_request {
        actions = actions.child(
            Button::new("Send to agent")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .help(send.unwrap_or("Queue this finding for the checkout's agent"))
                .disabled(send.is_some())
                .on_press(move |s: &mut AppState| s.send_findings(Some(id)))
                .build(k)
                .key(format!("review-send-{id}")),
        );
    }
    actions = actions
        .child(reaction_button(
            k,
            finding,
            FindingReaction::Dismiss,
            "Dismiss",
            "review-dismiss",
        ))
        .child(
            Button::new("Next ›")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .help("The next finding in the list (j)")
                .on_press(|s: &mut AppState| s.step_finding(true))
                .build(k)
                .key("review-next"),
        );
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

fn run_report<'a>(s: &'a AppState, run: &ReviewRun) -> Option<&'a Report> {
    s.code_review
        .reports
        .get(&(run.workspace_id, lane(&run.target)))
        .filter(|report| report.run == run.id)
}

fn headline(k: &Kit, report: &Report) -> El {
    let ink = if domain::review::blocking(&report.findings).is_empty() {
        k.c.success.fg
    } else {
        k.c.danger.fg
    };
    text(domain::review::headline(&report.findings))
        .key("review-headline")
        .role("status")
        .font(k.ty.ui)
        .color(ink)
        .truncate()
}

fn explored(k: &Kit, run: &ReviewRun, report: &Report) -> Option<El> {
    let explored = domain::review::explored(&run.coverage, &report.findings);
    if explored.is_empty() {
        return None;
    }
    let mut body = col()
        .key("review-hypotheses")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .child(section(k, "Hypotheses explored"));
    for hunted in explored {
        let outcome = match hunted.findings {
            0 => "explored, no findings".to_owned(),
            1 => "1 finding".to_owned(),
            n => format!("{n} findings"),
        };
        body = body.child(
            note(
                k,
                format!(
                    "{} · {outcome} · {}",
                    hunted.hypothesis.name(),
                    hunted.reason
                ),
            )
            .key(format!("review-explored-{}", hunted.hypothesis.name())),
        );
    }
    Some(body)
}

fn acknowledged_scope(k: &Kit, run: &ReviewRun) -> Option<El> {
    let scope = run.coverage.acknowledged_scope.as_ref()?;
    Some(
        col()
            .key("review-scope")
            .w_full()
            .gap(crate::ds::SPACE_2)
            .child(section(k, "Acknowledged scope"))
            .child(note(k, "The reviewers were told not to report these."))
            .child(
                text(scope.clone())
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .wrap(),
            ),
    )
}

fn place_label(finding: &Finding) -> String {
    match finding.line {
        Some(line) => format!("{}:{line}", finding.path),
        None => finding.path.clone(),
    }
}

/// The shown report's ids in the list's order, under the chosen filter.
fn finding_order(review: &CodeReview, workspace: WorkspaceId) -> Vec<u32> {
    review
        .shown_report(workspace)
        .map_or_else(Vec::new, |report| {
            listed(&report.findings, &report.groups, review.filter)
        })
}

impl AppState {
    /// A workspace finding opens in the editor; a pull request's, whose head
    /// is not this checkout, in the reviewed diff.
    pub fn open_finding(&mut self, id: u32) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let (Some(run), Some(report)) = (
            self.code_review.shown_run(workspace),
            self.code_review.shown_report(workspace),
        ) else {
            return;
        };
        let Some(finding) = report.findings.iter().find(|finding| finding.id == id) else {
            return;
        };
        let (path, line) = (finding.path.clone(), finding.line.unwrap_or(1));
        if is_pull_request(run) {
            self.code_review.view = ReviewView::Diff;
            self.code_review.reveal = Some((path, line));
            return;
        }
        if let Some(connection) = &self.connection {
            let _ = connection.open_editor_at(workspace, path, line);
        }
        self.workbench.show(CenterMode::Session);
    }

    pub fn step_finding(&mut self, forward: bool) {
        let Some(workspace) = self.code_review.workspace else {
            return;
        };
        let order = finding_order(&self.code_review, workspace);
        let current = shown_finding(&order, self.code_review.chosen);
        self.code_review.chosen = model::step(&order, current, forward);
    }
}

/// The list's keys while it holds focus: `j`/`k` move, `o` opens, `a` asks
/// the chat about the finding, `d` dismisses or brings it back.
fn findings_key(s: &mut AppState, ev: &rune::KeyEvent, _tx: &mut rune::Tx) -> bool {
    let m = ev.mods;
    if m.cmd || m.ctrl || m.alt || m.logo || m.shift {
        return false;
    }
    let Some(workspace) = s.code_review.workspace else {
        return false;
    };
    let letter = match &ev.key {
        rune::Key::Text(typed) => typed.as_str(),
        _ => "",
    };
    let current = shown_finding(
        &finding_order(&s.code_review, workspace),
        s.code_review.chosen,
    );
    match (&ev.key, letter) {
        (rune::Key::Down, _) | (_, "j") => s.step_finding(true),
        (rune::Key::Up, _) | (_, "k") => s.step_finding(false),
        (rune::Key::Enter, _) | (_, "o") => {
            if let Some(id) = current {
                s.open_finding(id);
            }
        }
        (_, "a") => {
            if let Some(id) = current {
                s.ask_about_finding(id);
            }
        }
        (_, "d") => {
            if let Some(id) = current {
                s.react_to_finding(id, FindingReaction::Dismiss);
            }
        }
        _ => return false,
    }
    true
}

/// One line of the findings list: severity, title, where.
fn finding_item(k: &Kit, finding: &Finding, chosen: bool, lineage: bool) -> El {
    let id = finding.id;
    let mut item = row()
        .key(format!("review-finding-row-{id}"))
        .role("option")
        .label(format!(
            "{:?} {}, {}",
            finding.severity,
            finding.title.trim(),
            place_label(finding)
        ))
        .w_full()
        .gap(crate::ds::SPACE_4)
        .items_center()
        .px(8.0)
        .py(6.0)
        .radius(k.radius.sm)
        .hover(k.c.hover)
        .on_press(move |s: &mut AppState| s.code_review.chosen = Some(id))
        .child(
            text(format!("{:?}", finding.severity))
                .font(k.ty.text_xsmall_medium)
                .color(severity_ink(k, finding.severity)),
        )
        .child(
            col()
                .min_w(0.0)
                .grow(1.0)
                .child(
                    text(finding.title.trim().to_owned())
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .truncate(),
                )
                .child(
                    text(place_label(finding))
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_muted)
                        .truncate(),
                ),
        );
    if lineage {
        item = item.child(lineage_badge(k, finding));
    }
    if chosen {
        item = item
            .fill(k.c.surface_hover)
            .border_sides([0.0, 0.0, 0.0, 2.0], severity_ink(k, finding.severity));
    }
    item
}

fn filter_label(filter: FindingFilter, count: usize) -> String {
    let name = match filter {
        FindingFilter::Open => "Open",
        FindingFilter::New => "New",
        FindingFilter::Dismissed => "Dismissed",
    };
    format!("{name} {count}")
}

/// The findings as a list beside one finding at a time, with the chat under it.
fn findings(s: &AppState, k: &Kit, run: &ReviewRun) -> El {
    let Some(report) = run_report(s, run) else {
        return col();
    };
    let review = &s.code_review;
    let lineage = report.previous.is_some();
    let send = s.send_destination(run).err();
    let filters = [
        FindingFilter::Open,
        FindingFilter::New,
        FindingFilter::Dismissed,
    ];
    let labels: Vec<String> = filters
        .iter()
        .map(|filter| {
            let count = report.findings.iter().filter(|f| filter.keeps(f)).count();
            filter_label(*filter, count)
        })
        .collect();
    let filter = SegmentedControl::new(
        &labels,
        filters.iter().position(|filter| *filter == review.filter),
    )
    .label("Show findings")
    .fit()
    .on_pick(move |s: &mut AppState, index: usize, _| {
        if let Some(filter) = filters.get(index) {
            s.code_review.filter = *filter;
        }
    })
    .build(k)
    .key("review-filter");
    let grouping = Button::new(match review.grouping {
        Grouping::Severity => "By severity",
        Grouping::File => "By file",
    })
    .variant(ButtonVariant::GhostMuted)
    .control_size(ControlSize::Small)
    .help("Group the list by severity or by file")
    .on_press(|s: &mut AppState| {
        s.code_review.set_grouping(match s.code_review.grouping {
            Grouping::Severity => Grouping::File,
            Grouping::File => Grouping::Severity,
        });
    })
    .build(k)
    .key("review-grouping");
    let order = listed(&report.findings, &report.groups, review.filter);
    let shown = shown_finding(&order, review.chosen);
    let mut list = col()
        .key("review-finding-list")
        .role("listbox")
        .label("Findings: j and k move, o opens, a asks the chat, d dismisses")
        .w_full()
        .gap(crate::ds::SPACE_1)
        .focusable(true)
        .on_key(findings_key);
    for (key, group) in &report.groups {
        let kept: Vec<&Finding> = group
            .iter()
            .map(|at| &report.findings[*at])
            .filter(|finding| review.filter.keeps(finding))
            .collect();
        if kept.is_empty() {
            continue;
        }
        list = list.child(group_header(k, review.grouping, key, kept.len()));
        for finding in kept {
            list = list.child(finding_item(k, finding, shown == Some(finding.id), lineage));
        }
    }
    if order.is_empty() {
        list = list.child(note(
            k,
            match review.filter {
                FindingFilter::Open if report.findings.is_empty() => "The review found nothing.",
                FindingFilter::Open => "Every finding is dismissed.",
                FindingFilter::New => "Nothing new since the last run.",
                FindingFilter::Dismissed => "Nothing dismissed.",
            },
        ));
    }
    if !report.resolved.is_empty() {
        list = list.child(resolved_group(s, k, report));
    }
    let hints = text("j / k move · o open · a ask · d dismiss")
        .key("review-keys")
        .font(k.ty.small)
        .color(k.c.text_faint)
        .a11y_hidden();
    let column = col()
        .key("review-list-column")
        .w(FINDINGS_LIST_W)
        .gap(crate::ds::SPACE_4)
        .child(filter)
        .child(row().w_full().child(el().grow(1.0)).child(grouping))
        .child(list)
        .child(hints);
    let position = shown.and_then(|id| order.iter().position(|at| *at == id));
    let mut detail = col()
        .key("review-detail")
        .min_w(0.0)
        .grow(1.0)
        .gap(crate::ds::SPACE_6);
    if let Some(finding) = shown.and_then(|id| report.findings.iter().find(|f| f.id == id)) {
        let place = position.map(|at| (at + 1, order.len()));
        detail = detail.child(finding_row(k, run, finding, lineage, send, place));
    }
    let chat = chat_panel(s, k, run, report);
    let mut columns = row()
        .w_full()
        .gap(crate::ds::SPACE_7)
        .items_start()
        .child(column);
    if room_for_chat_column(s) {
        columns = columns
            .child(detail)
            .child(col().w(CHAT_COLUMN_W).child(chat));
    } else {
        columns = columns.child(detail.child(chat));
    }
    col().key("review-findings").w_full().child(columns)
}

const FINDINGS_LIST_W: f32 = 300.0;
const CHAT_COLUMN_W: f32 = 340.0;
/// Below this the detail would be squeezed under ~420 points, so the chat
/// drops under it instead of sitting beside it.
const THREE_COLUMNS_MIN_W: f32 = 1140.0;

fn room_for_chat_column(s: &AppState) -> bool {
    let sidebar = if s.sidebar.is_open() {
        s.sidebar.width()
    } else {
        0.0
    };
    s.window.get()[0] - sidebar >= THREE_COLUMNS_MIN_W
}

/// A group's title in the colour of what it means for the merge.
fn group_header(k: &Kit, grouping: Grouping, key: &str, count: usize) -> El {
    let ink = match (grouping, key) {
        (Grouping::Severity, "P0" | "P1") => k.c.danger.fg,
        (Grouping::Severity, "P2") => k.c.warning.fg,
        _ => k.c.text_faint,
    };
    text(format!("{} ({count})", group_title(grouping, key)))
        .role("heading")
        .font(k.ty.small_medium)
        .color(ink)
        .pt(crate::ds::SPACE_4)
        .pb(crate::ds::SPACE_2)
        .px(crate::ds::SPACE_2)
}

/// The lane's read-only chat: start it with the report, ask it, and keep it
/// told when the review runs again.
fn chat_panel(s: &AppState, k: &Kit, run: &ReviewRun, report: &Report) -> El {
    let review = &s.code_review;
    let slot = (run.workspace_id, lane(&run.target));
    let chat = review.chats.get(&slot).copied();
    let live = s.live_chat(slot);
    let starting = review.chat_starting.is_some() || review.chat_opening.is_some();
    let agent = s.prompt_agent().ok().map_or_else(
        || "an agent".to_owned(),
        |(provider, _)| provider.to_string(),
    );
    let mut panel = col()
        .key("review-chat")
        .role("region")
        .label("Review chat")
        .w_full()
        .gap(crate::ds::SPACE_4)
        .padding(12.0)
        .radius(k.radius.md)
        .border(1.0, k.c.border_weak)
        .child(
            row()
                .w_full()
                .gap(crate::ds::SPACE_4)
                .items_center()
                .child(
                    text("Review chat")
                        .role("heading")
                        .font(k.ty.ui_medium)
                        .color(k.c.text_base),
                )
                .child(note(k, agent.clone()))
                .child(el().grow(1.0))
                .child(crate::ds::chip(
                    k,
                    &crate::theme::roles(k),
                    "read-only",
                    crate::ds::Tone::Neutral,
                )),
        );
    let status = |message: String| {
        text(message)
            .key("review-chat-status")
            .role("status")
            .font(k.ty.small)
            .color(k.c.text_base)
            .wrap()
    };
    if starting {
        panel = panel.child(status(
            "Starting the chat with this run's report…".to_owned(),
        ));
    } else if let Some(live) = live {
        let row_status = s
            .rows
            .iter()
            .find(|row| row.id == live.session)
            .and_then(|row| row.status);
        let busy = row_status == Some(crate::app::SessionStatus::Busy);
        match transcript(s, k, live.session, busy) {
            Some(log) => panel = panel.child(log),
            None => {
                panel = panel.child(status(
                    match row_status {
                        Some(crate::app::SessionStatus::Busy) => "Working on your question…",
                        Some(crate::app::SessionStatus::Attention) => "Waiting on you in the chat",
                        None => "Ready for a question",
                    }
                    .to_owned(),
                ));
            }
        }
        if live.run != run.id && run.status == ReviewStatus::Completed {
            panel = panel.child(
                col()
                    .key("review-chat-stale")
                    .w_full()
                    .gap(crate::ds::SPACE_2)
                    .child(note(
                        k,
                        format!(
                            "This run finished after the chat started ({}). The chat still \
                             talks about the earlier one.",
                            run.verdict.map_or("no verdict", verdict_label)
                        ),
                    ))
                    .child(
                        row()
                            .gap(crate::ds::SPACE_3)
                            .child(
                                Button::new("Tell the chat what changed")
                                    .variant(ButtonVariant::Outline)
                                    .control_size(ControlSize::Small)
                                    .disabled(review.telling(slot))
                                    .on_press(|s: &mut AppState| s.tell_review_chat())
                                    .build(k)
                                    .key("review-chat-tell"),
                            )
                            .child(
                                Button::new("New chat")
                                    .variant(ButtonVariant::GhostMuted)
                                    .control_size(ControlSize::Small)
                                    .on_press(|s: &mut AppState| s.start_review_chat(None))
                                    .build(k)
                                    .key("review-chat-new"),
                            ),
                    ),
            );
        }
    } else if let Some(chat) = chat {
        let listed = s.rows.iter().any(|row| row.id == chat.session);
        let mut actions = row().gap(crate::ds::SPACE_3);
        if listed {
            let session = chat.session;
            actions = actions.child(
                Button::new("Open the old chat")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut AppState| s.focus_session(session))
                    .build(k)
                    .key("review-chat-old"),
            );
        }
        panel = panel
            .children(transcript(s, k, chat.session, false))
            .child(status("This chat ended.".to_owned()))
            .child(note(
                k,
                "Ask again to start a new one from this run: restarting the old session \
                 would not send it the report again.",
            ))
            .child(actions);
    } else {
        let standing = report
            .findings
            .iter()
            .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
            .count();
        panel = panel.child(
            col()
                .key("review-chat-empty")
                .w_full()
                .gap(crate::ds::SPACE_3)
                .py(crate::ds::SPACE_6)
                .items_center()
                .child(
                    text("Ask about this review")
                        .font(k.ty.ui_medium)
                        .color(k.c.text_base),
                )
                .child(
                    text(format!(
                        "Starts {agent} already knowing the summary and the {standing} \
                         standing finding(s). It can read the code and the pull request; it \
                         cannot edit, push or post.",
                    ))
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .align(rune::Align::Center)
                    .wrap(),
                )
                .child(suggestions(k, standing > 0)),
        );
    }
    if let Some(finding) = about_finding(review.question_about, run, report) {
        panel = panel.child(
            row()
                .key("review-question-about")
                .gap(crate::ds::SPACE_3)
                .items_center()
                .child(crate::ds::chip(
                    k,
                    &crate::theme::roles(k),
                    format!("About {:?} · {}", finding.severity, finding.title.trim()),
                    crate::ds::Tone::Info,
                ))
                .child(
                    Button::new("Clear")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut AppState| s.code_review.question_about = None)
                        .build(k)
                        .key("review-question-clear"),
                ),
        );
    }
    let ask_label = if live.is_some() { "Ask" } else { "Start chat" };
    let mut ask = row()
        .w_full()
        .gap(crate::ds::SPACE_3)
        .items_end()
        .child(
            el().min_w(0.0).grow(1.0).child(
                TextArea::new(&review.question)
                    .label("Question for the review chat")
                    .placeholder(if review.question_about.is_some() {
                        "What do you want to know about this finding?"
                    } else {
                        "Ask about this review. Shift+Enter for a new line."
                    })
                    .width(None)
                    .auto_grow(1, COMPOSER_MAX_ROWS)
                    .disabled(starting)
                    .on_edit(|s: &mut AppState| &mut s.code_review.question)
                    .on_key(|s: &mut AppState, ev, _| {
                        if ev.key == rune::Key::Enter && !ev.mods.shift && !ev.mods.alt {
                            s.ask_review_chat();
                            true
                        } else {
                            false
                        }
                    })
                    .build(k)
                    .key("review-question"),
            ),
        )
        .child(
            Button::new(ask_label)
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Small)
                .disabled(
                    starting
                        || (live.is_some() && review.question.text.trim().is_empty())
                        || run.status != ReviewStatus::Completed,
                )
                .on_press(|s: &mut AppState| s.ask_review_chat())
                .build(k)
                .key("review-chat-ask"),
        );
    if let Some(live) = live {
        let session = live.session;
        ask = ask.child(
            Button::new("Open chat")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| s.focus_session(session))
                .build(k)
                .key("review-chat-open"),
        );
    }
    panel = panel.child(ask);
    if live.is_some() {
        panel = panel.child(note(
            k,
            "A question waits while the agent works; a second one is refused until it is sent.",
        ));
    }
    panel
}

const COMPOSER_MAX_ROWS: usize = 6;
const BUBBLE_MAX_W: f32 = 300.0;
const CHAT_LOG_KEY: &str = "review-chat-log";
/// How far above its end the transcript offers a jump back.
const CHAT_FAR: f32 = 240.0;

fn chat_log_height(s: &AppState) -> f32 {
    (s.window.get()[1] - 380.0).clamp(220.0, 560.0)
}

const SUGGESTIONS: [&str; 3] = [
    "Which finding should I fix first, and why?",
    "Is anything here a real blocker for merging?",
    "Explain the riskiest finding with the code it points at.",
];

fn suggestions(k: &Kit, any_standing: bool) -> El {
    let mut list = col()
        .key("review-chat-suggestions")
        .w_full()
        .gap(crate::ds::SPACE_2)
        .pt(crate::ds::SPACE_3)
        .items_center();
    let offered = if any_standing {
        &SUGGESTIONS[..]
    } else {
        &SUGGESTIONS[2..]
    };
    for (at, words) in offered.iter().enumerate() {
        list = list.child(
            Button::new(*words)
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    s.code_review.question = TextEdit::new(*words);
                })
                .build(k)
                .key(format!("review-chat-suggestion-{at}")),
        );
    }
    list
}

/// The chat's conversation, newest at the bottom, or `None` while the daemon
/// has nothing it can read for the session (the status line stands in).
fn transcript(s: &AppState, k: &Kit, session: SessionId, busy: bool) -> Option<El> {
    let log = s.code_review.logs.get(&session)?;
    if !log.available || log.items.is_empty() {
        return None;
    }
    let mut list = col()
        .key("review-chat-turns")
        .w_full()
        .gap(crate::ds::SPACE_6)
        .py(crate::ds::SPACE_4);
    if log.truncated {
        list = list.child(chat_note(k, "Earlier turns are not shown."));
    }
    for (at, item) in log.items.iter().enumerate() {
        list = list.child(chat_item(k, session, at, item));
    }
    if busy {
        list = list.child(
            row()
                .key("review-chat-working")
                .role("status")
                .label("The chat is working")
                .gap(crate::ds::SPACE_3)
                .items_center()
                .child(crate::ds::working(14.0, k.c.text_muted))
                .child(text("Working…").font(k.ty.small).color(k.c.text_muted)),
        );
    }
    if let Some(error) = &log.error {
        list = list.child(chat_note(k, format!("Could not read the chat: {error}")));
    }
    let scroller = col()
        .key(CHAT_LOG_KEY)
        .role("log")
        .label("Review chat conversation")
        .w_full()
        .h(chat_log_height(s))
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(16.0)
        .on_far_from_bottom_if(CHAT_FAR, None, |s: &mut AppState, far| {
            let changed = s.code_review.chat_far != far;
            s.code_review.chat_far = far;
            changed
        })
        .child(list);
    let mut frame = col().key("review-chat-frame").w_full().child(scroller);
    if s.code_review.chat_far {
        frame = frame.child(
            row()
                .w_full()
                .justify_center()
                .pt(crate::ds::SPACE_2)
                .child(
                    Button::new("Jump to latest")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .on_press_tx(|_s: &mut AppState, tx| tx.scroll_to_bottom(CHAT_LOG_KEY))
                        .build(k)
                        .key("review-chat-jump"),
                ),
        );
    }
    Some(frame)
}

fn chat_note(k: &Kit, words: impl Into<String>) -> El {
    text(words.into())
        .font(k.ty.text_xsmall)
        .color(k.c.text_faint)
        .align(rune::Align::Center)
        .w_full()
        .wrap()
}

fn chat_item(k: &Kit, session: SessionId, at: usize, item: &chat::ChatItem) -> El {
    match item {
        chat::ChatItem::Briefing => chat_note(k, "Started with this run's report"),
        chat::ChatItem::Update => chat_note(k, "Told the chat the review ran again"),
        chat::ChatItem::Question(words) => row()
            .key(format!("review-chat-question-{at}"))
            .w_full()
            .justify_end()
            .child(
                col()
                    .max_w(BUBBLE_MAX_W)
                    .px(crate::ds::SPACE_5)
                    .py(crate::ds::SPACE_4)
                    .radius(k.radius.xl)
                    .fill(k.c.layer_02)
                    .child(
                        text(words.clone())
                            .font(k.ty.ui)
                            .color(k.c.text_base)
                            .wrap(),
                    ),
            ),
        chat::ChatItem::Answer { text: words, tools } => rune::memo(
            format!("review-chat-answer-{session}-{at}"),
            (words, tools, crate::workbench::view::kit_fingerprint(k)),
            || {
                let mut answer = col().w_full().gap(crate::ds::SPACE_3);
                if !words.trim().is_empty() {
                    answer = answer.child(Markdown::new(words.clone()).build(k));
                }
                let (shown, more) = chat::shown_tools(tools);
                if !shown.is_empty() {
                    let mut ran = col().w_full().gap(crate::ds::SPACE_1);
                    for tool in shown {
                        ran = ran.child(
                            text(tool.clone())
                                .font(k.ty.text_xsmall)
                                .color(k.c.text_faint)
                                .truncate(),
                        );
                    }
                    if more > 0 {
                        ran = ran.child(
                            text(format!("and {more} more"))
                                .font(k.ty.text_xsmall)
                                .color(k.c.text_faint),
                        );
                    }
                    answer = answer.child(ran);
                }
                answer
            },
        ),
    }
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
    let diff = DiffView::new(
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
    .grow(1.0)
    .min_h(0.0);
    let layout = row()
        .key("review-diff-layout")
        .w_full()
        .gap(crate::ds::SPACE_5)
        .items_center()
        .justify_end()
        .children((!s.diff_split).then(|| crate::workbench::view::diff_wrap_switch(s, k)))
        .child(
            rune_kit::SegmentedControl::new(&["Unified", "Split"], Some(usize::from(s.diff_split)))
                .label("Patch layout")
                .width(Some(150.0))
                .on_pick(|s: &mut AppState, index, _| s.set_diff_split(index == 1))
                .build(k),
        );
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(crate::ds::SPACE_4)
        .child(layout)
        .child(diff)
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

const REVIEW_VIEWS: [ReviewView; 5] = [
    ReviewView::Findings,
    ReviewView::Summary,
    ReviewView::Diff,
    ReviewView::Diagrams,
    ReviewView::Graph,
];

/// The report's tabs, and the workspace run's hand-offs beside them. A pull
/// request's head is not this checkout, so it gets no fix buttons at all
/// rather than three disabled ones.
fn review_tabs(s: &AppState, k: &Kit, run: &ReviewRun, report: Option<&Report>) -> El {
    let standing_count = report.map_or(0, |report| {
        report
            .findings
            .iter()
            .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
            .count()
    });
    let labels: Vec<String> = REVIEW_VIEWS
        .iter()
        .map(|view| match view {
            ReviewView::Findings => format!("Findings {standing_count}"),
            ReviewView::Summary => "Summary".into(),
            ReviewView::Diff => "Diff".into(),
            ReviewView::Diagrams => "Diagrams".into(),
            ReviewView::Graph => "Graph".into(),
        })
        .collect();
    let actions = row()
        .key("review-actions")
        .w_full()
        .flex_wrap()
        .gap(crate::ds::SPACE_3)
        .items_center()
        .child(
            SegmentedControl::new(
                &labels,
                REVIEW_VIEWS
                    .iter()
                    .position(|view| *view == s.code_review.view),
            )
            .label("Show")
            .fit()
            .on_pick(|s: &mut AppState, index: usize, _| {
                if let Some(view) = REVIEW_VIEWS.get(index) {
                    s.code_review.view = *view;
                }
            })
            .build(k)
            .key("review-view"),
        )
        .child(el().grow(1.0));
    if is_pull_request(run) {
        return actions;
    }
    fix_actions(s, k, run, actions, standing_count > 0)
}

fn fix_actions(s: &AppState, k: &Kit, run: &ReviewRun, actions: El, standing: bool) -> El {
    let send = s.send_destination(run).err();
    let mut actions = actions
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
                .disabled(!standing)
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
            .disabled(!standing || looping)
            .on_press(move |s: &mut AppState| s.code_review.fix_loop_confirm = Some(review_id))
            .build(k)
            .key("review-fix-loop")
        });
    if let Some(reason) = send {
        actions = actions.child(note(k, reason));
    }
    actions
}

/// The run's prose and its bookkeeping, off the findings' page: the summary
/// repeats per hypothesis and would otherwise push the list's detail away.
fn summary_tab(s: &AppState, k: &Kit, run: &ReviewRun, report: Option<&Report>) -> El {
    let summary = (!run.summary.trim().is_empty()).then(|| {
        col()
            .w_full()
            .gap(crate::ds::SPACE_2)
            .child(section(k, "Summary"))
            .child(
                Markdown::new(run.summary.clone())
                    .build(k)
                    .key("review-summary"),
            )
    });
    col()
        .key("review-summary-tab")
        .w_full()
        .max_w(920.0)
        .gap(crate::ds::SPACE_6)
        .children(summary)
        .children(report.and_then(|report| explored(k, run, report)))
        .children(acknowledged_scope(k, run))
        .children(kb_section(s, k, run))
        .children(sandbox_panel(s, k, run))
        .child(coverage(k, run))
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
    if graph.indexing {
        return body.child(note(k, "Indexing the codebase graph…").key("review-graph-indexing"));
    }
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
            .on_press(|s: &mut AppState| s.toggle_review_kb())
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
        .filter(|kb| kb.run == run.id)
        .and_then(|kb| kb.files.as_ref());
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
        .child(section(k, "Sandbox checks"));
    let read = s
        .code_review
        .sandbox
        .as_ref()
        .filter(|(read, _)| *read == run.id)
        .map(|(_, result)| result);
    let sandbox = match read {
        None => return Some(body.child(note(k, SANDBOX_READING))),
        Some(Err(error)) => return Some(body.child(note(k, error.clone()))),
        Some(Ok(sandbox)) => sandbox,
    };
    if let Some(error) = &sandbox.error {
        body = body.child(note(k, error.clone()));
    }
    if let Some(hash) = &sandbox.unapproved_hash {
        let approved = s.code_review.sandbox_approved.as_ref() == Some(hash);
        let approving = s.code_review.sandbox_approving.as_ref() == Some(hash);
        let (workspace_id, hash) = (sandbox.workspace_id, hash.clone());
        body = body.child(
            row()
                .gap(crate::ds::SPACE_3)
                .items_center()
                .child(
                    Button::new(if approved {
                        "Approved"
                    } else if approving {
                        "Approving…"
                    } else {
                        "Approve checks.toml"
                    })
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .disabled(approved || approving)
                    .on_press(move |s: &mut AppState| s.confirm_sandbox(workspace_id, hash.clone()))
                    .build(k)
                    .key("review-sandbox-approve"),
                )
                .child(note(
                    k,
                    if approved {
                        "The next deep review runs it."
                    } else {
                        "Its commands run in the sandbox on every deep review until the file changes."
                    },
                )),
        );
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

pub fn sandbox_approve_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(confirm) = &s.code_review.sandbox_confirm else {
        return;
    };
    let mut modal = crate::modal::Modal::new("review-sandbox-confirm", "Approve checks.toml?")
        .width(crate::modal::WIDTH_WIDE)
        .on_close(|s: &mut AppState| s.code_review.sandbox_confirm = None)
        .child(note(
            k,
            "Every deep review of this project runs these targets' commands in the sandbox \
             until the file changes.",
        ));
    modal = match &confirm.file {
        None => modal.child(note(k, format!("Reading {SANDBOX_CHECKS_PATH}…"))),
        Some(Err(error)) => modal.child(note(k, error.clone())),
        Some(Ok(file)) => modal.child(
            text(file.clone())
                .font(k.ty.mono)
                .color(k.c.text_base)
                .wrap()
                .key("review-sandbox-confirm-file"),
        ),
    };
    modal
        .child(note(k, format!("SHA-256 {}", confirm.hash)))
        .footer([
            Button::new("Cancel")
                .variant(ButtonVariant::GhostMuted)
                .on_press(|s: &mut AppState| s.code_review.sandbox_confirm = None)
                .build(k)
                .key("review-sandbox-confirm-cancel"),
            Button::new("Approve")
                .variant(ButtonVariant::Contrast)
                .disabled(!matches!(confirm.file, Some(Ok(_))))
                .on_press(|s: &mut AppState| s.approve_sandbox())
                .build(k)
                .key("review-sandbox-confirm-approve"),
        ])
        .build(s, k, layer);
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

pub fn view(s: &AppState, k: &Kit, layer: &mut Vec<El>) -> El {
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
    // The view rises into the title row when the sidebar is closed, so its
    // header starts past the traffic lights like a terminal pane's.
    let mut top = row()
        .key("review-head")
        .w_full()
        .h(crate::ui::TITLE_H + crate::ds::SPACE_4)
        .pl(if s.sidebar.is_open() {
            12.0
        } else {
            s.title_clear.get()
        })
        .pr(12.0)
        .gap(10.0)
        .items_center()
        .window_drag()
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
    // The Diff tab scrolls itself, so its sticky headers and virtual rows see
    // the real viewport rather than a fixed box inside the page's scroll.
    let diff_fills = !s.code_review.customizing
        && s.code_review.view == ReviewView::Diff
        && run.is_some_and(|run| run.status == ReviewStatus::Completed);
    let mut body = col().key("review-body").w_full().gap(10.0);
    if diff_fills {
        body = body.grow(1.0).min_h(0.0);
    }
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
    if s.code_review.customizing {
        body = body.child(full_form(s, k));
    } else if run.is_none() {
        body = body.child(sentence(s, k, layer, workspace, None));
    }
    match run {
        Some(run) => {
            let completed = run.status == ReviewStatus::Completed;
            let report = run_report(s, run);
            let banner = verdict_banner(k, run, report.filter(|_| completed), head);
            let led = banner.is_some();
            body = body.children(banner);
            if !led {
                body = body.child(run_header(s, k, run, head));
            }
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
            if !s.code_review.customizing {
                body = body.child(sentence(s, k, layer, workspace, Some(run)));
            }
            if !completed {
                body = body
                    .child(findings(s, k, run))
                    .child(summary_tab(s, k, run, report));
            } else {
                body = body.child(review_tabs(s, k, run, report));
                body = match s.code_review.view {
                    ReviewView::Summary => body.child(summary_tab(s, k, run, report)),
                    ReviewView::Diff => body.child(reviewed_diff(s, k)),
                    ReviewView::Diagrams => body.child(diagrams(s, k, run)),
                    ReviewView::Graph => body.child(graph(s, k)),
                    ReviewView::Findings => body.child(findings(s, k, run)),
                };
            }
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
        .child({
            let page = col()
                .key("review-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .px(12.0)
                .py(6.0);
            if diff_fills {
                page.child(body)
            } else {
                page.scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body)
            }
        })
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
            reachability: None,
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
            tier: Tier::Quick,
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
            reviewers: 1,
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
        assert_eq!(verdict_label(Verdict::Block), "Blocks merge");
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
        for key in [
            "review-sentence",
            "review-word-subject-trigger",
            "review-word-focus-trigger",
            "review-word-depth-trigger",
            "review-customize",
            "review-start",
            "review-empty",
        ] {
            assert!(keys.iter().any(|known| known == key), "{key}: {keys:?}");
        }
        assert!(
            !keys.iter().any(|key| key == "review-presets"),
            "the four-question form waits behind Customize"
        );
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

    #[test]
    fn a_spec_runs_older_review_offers_its_findings_without_replacing_the_review_tab() {
        let (mut state, workspace) = fixture_state();
        let older = completed(workspace, workspace_target(workspace));
        let newer = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &newer, Vec::new(), Vec::new(), None);
        let mut spec = crate::test_support::spec_run(
            domain::spec_run::model::SpecStage::ResultGate,
            Timestamp::now(),
        );
        spec.review_id = Some(older.id);
        spec.coder_workspace_id = Some(workspace);
        if let Some(store) = state.store.as_mut() {
            store.spec_runs = vec![spec.clone()];
        }
        state.spec_panel = Some(crate::spec_run_view::SpecPanel::new(spec.id));

        let mut cited = finding(7, "src/old.rs", Some(9), Severity::P2);
        cited.suggestion = Some("check the bound".into());
        assert!(state.apply_review_report(
            workspace,
            Ok(Box::new(client::ReviewReport {
                run: older,
                findings: vec![cited],
                resolved: Vec::new(),
                previous: None,
            })),
        ));
        let (text, left_out) = state
            .spec_panel
            .as_ref()
            .and_then(|panel| panel.review_round.clone())
            .expect("G3 offers the findings");
        assert!(text.contains("src/old.rs:9 — finding 7"), "{text}");
        assert!(text.contains("check the bound"), "{text}");
        assert_eq!(left_out, 0);
        assert_eq!(
            state.code_review.reports[&(workspace, lane(&newer.target))].run,
            newer.id,
            "the Review tab keeps the newer report"
        );
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
        with_report(
            &mut state,
            &run,
            vec![liked.clone(), dismissed.clone()],
            Vec::new(),
            None,
        );
        if let Some(config) = state.code_review.configs.get_mut(&workspace) {
            config.suppressions = vec![ReviewSuppression {
                kind: domain::SuppressionKind::Rule,
                value: "money".into(),
                severity: None,
                reason: "3 dismissals in 30 days".into(),
            }];
        }
        let shown = keys(state);
        assert!(
            !has(&shown, "review-finding-row-2"),
            "the open list leaves a dismissed finding out"
        );
        for key in [
            "review-up-1",
            "review-down-1",
            "review-dismiss-1",
            "review-send-1",
        ] {
            assert!(has(&shown, key), "{key}: {shown:?}");
        }
        assert!(has(&shown, "review-suppressed"));
        assert!(has(&shown, "review-suppression-undo-0"));
        assert!(has(&shown, "review-view"));
        assert!(has(&shown, "review-send-all"));
        assert!(has(&shown, "review-fix"));

        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &run, vec![liked, dismissed], Vec::new(), None);
        state.code_review.filter = FindingFilter::Dismissed;
        let shown = keys(state);
        assert!(has(&shown, "review-finding-row-2"), "{shown:?}");
        assert!(has(&shown, "review-undo-2"));
        assert!(
            !has(&shown, "review-up-2"),
            "a dismissed finding is collapsed"
        );
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
    fn a_pull_request_run_offers_no_send_or_fix() {
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
        assert!(has(&shown, "review-banner"));
        for gone in [
            "review-send-all",
            "review-fix",
            "review-fix-loop",
            "review-send-1",
        ] {
            assert!(!has(&shown, gone), "{gone} is offered for a pull request");
        }
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

    fn reconnect(state: &mut AppState, generation: u64) {
        state.apply_events(
            vec![
                crate::connection::UiEvent::Connecting {
                    generation,
                    detail: String::new(),
                },
                crate::connection::UiEvent::Connected {
                    generation,
                    detail: String::new(),
                    editor_surface: client::EditorSurface::Cells,
                },
            ],
            &mut rune::Tx::default(),
        );
    }

    fn count(
        rx: &flume::Receiver<crate::connection::Command>,
        wanted: impl Fn(&crate::connection::Command) -> bool,
    ) -> usize {
        rx.try_iter().filter(|command| wanted(command)).count()
    }

    fn lists(command: &crate::connection::Command) -> bool {
        matches!(command, crate::connection::Command::ListReviews { .. })
    }

    #[test]
    fn a_known_workspace_lists_its_runs_again_after_a_reconnect() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        let mut running = run(ReviewId::new(), Some(ReviewStage::Model), false);
        running.workspace_id = workspace;
        running.target = workspace_target(workspace);
        state.apply_review_run(&running);
        assert!(state.code_review.busy(workspace));

        state.sync_review();
        assert_eq!(count(&rx, lists), 1);
        state.sync_review();
        assert_eq!(count(&rx, lists), 0, "once per connection");

        reconnect(&mut state, 2);
        state.sync_review();
        assert_eq!(
            count(&rx, lists),
            1,
            "a run already on screen is listed again"
        );
        let mut interrupted = running.clone();
        interrupted.status = ReviewStatus::Interrupted;
        interrupted.finished_at = Some(Timestamp::now());
        assert!(state.apply_reviews(workspace, Ok(vec![interrupted])));
        assert!(!state.code_review.busy(workspace), "Start is free again");
    }

    #[test]
    fn refresh_reads_the_settings_again_while_keeping_the_old_ones() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        let reads_config = |command: &crate::connection::Command| {
            matches!(command, crate::connection::Command::ReviewConfig { .. })
        };
        state.sync_review();
        assert_eq!(count(&rx, reads_config), 1);
        state.sync_review();
        assert_eq!(count(&rx, reads_config), 0);
        refresh(&mut state);
        assert_eq!(count(&rx, reads_config), 1);
        assert!(state.code_review.configs.contains_key(&workspace));
    }

    #[test]
    fn a_github_read_lost_to_a_reconnect_frees_start_and_says_why() {
        let (mut state, workspace) = fixture_state();
        let _rx = connected(&mut state);
        state.code_review.subject = Subject::PullRequest;
        state.code_review.pull_request = TextEdit::new("https://github.com/acme/forge/pull/5");
        state.start_code_review();
        assert!(state.code_review.busy(workspace));
        state.open_pr_link();
        if let Some(dialog) = state.pr_link.as_mut() {
            dialog.url = TextEdit::new("https://github.com/acme/forge/pull/6");
        }
        state.submit_pr_link();
        assert!(state.pr_link.as_ref().is_some_and(|d| d.pending.is_some()));

        reconnect(&mut state, 2);
        assert!(state.code_review.awaiting.is_none());
        assert!(!state.code_review.busy(workspace));
        assert_eq!(
            state.code_review.errors.get(&workspace).map(String::as_str),
            Some(crate::pr_link::LINK_DROPPED)
        );
        let dialog = state.pr_link.as_ref().expect("the dialog stays");
        assert!(dialog.pending.is_none(), "Open is pressable again");
        assert_eq!(dialog.error.as_deref(), Some(crate::pr_link::LINK_DROPPED));
    }

    #[test]
    fn the_knowledge_base_is_read_for_each_run_on_refresh_and_after_a_reconnect() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        let reads_kb = |command: &crate::connection::Command| {
            matches!(command, crate::connection::Command::GetKb { .. })
        };
        let mut cited = finding(1, "a.rs", Some(2), Severity::P1);
        cited.kb_ref = Some("architecture.md#crates".into());
        let first = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &first, vec![cited.clone()], vec![], None);
        state.toggle_review_kb();
        assert_eq!(count(&rx, reads_kb), 1);
        assert!(state.apply_kb(workspace, Ok(Vec::new())));
        state.sync_review();
        assert_eq!(count(&rx, reads_kb), 0, "read once per run");

        let second = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &second, vec![cited], vec![], None);
        state.sync_review();
        assert_eq!(
            count(&rx, reads_kb),
            1,
            "a newer run is not shown the older read"
        );
        assert!(
            state
                .code_review
                .kb
                .as_ref()
                .is_some_and(|kb| kb.run == second.id && kb.files.is_none())
        );
        assert!(state.apply_kb(workspace, Ok(Vec::new())));

        refresh(&mut state);
        assert_eq!(count(&rx, reads_kb), 1);
        reconnect(&mut state, 2);
        state.sync_review();
        assert_eq!(count(&rx, reads_kb), 1, "the read in flight is asked again");
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

    fn live_row(state: &mut AppState, workspace: WorkspaceId) -> SessionId {
        let row = state
            .rows
            .iter_mut()
            .find(|row| row.workspace_id == workspace)
            .expect("the fixture checkout has a session");
        row.running = true;
        row.id
    }

    #[test]
    fn a_question_starts_a_read_only_chat_that_the_tree_hangs_under_its_review() {
        use crate::connection::Command;
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
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
        state.ask_about_finding(1);
        state.code_review.question = TextEdit::new("is it real?");
        state.ask_review_chat();
        let (prompt, review) = match rx.try_recv() {
            Ok(Command::CreateReviewChat {
                initial_prompt,
                review_id,
                ..
            }) => (initial_prompt, review_id),
            other => panic!("expected CreateReviewChat, got {other:?}"),
        };
        assert_eq!(review, run.id);
        assert!(prompt.contains("About finding 1 (P1, a.rs:2"), "{prompt}");
        assert!(
            prompt.contains("gh pr diff 7"),
            "a PR head is not checked out"
        );
        assert!(prompt.ends_with(domain::pr_review::READ_ONLY_RULES));
        assert!(state.code_review.question.text.is_empty());
        state.ask_review_chat();
        assert!(rx.try_recv().is_err(), "one chat starts at a time");

        let session = live_row(&mut state, workspace);
        assert!(state.review_chat_opened(run.id, session));
        assert_eq!(state.code_review.chat_starting, None);
        let tree = state.code_review.tree(workspace, &state.rows);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].lane, Lane::PullRequest(7));
        assert_eq!(tree[0].chat, Some(session));
        assert!(state.code_review.is_chat(session));

        state.code_review.question = TextEdit::new("and the 503?");
        state.ask_review_chat();
        match rx.try_recv() {
            Ok(Command::SendSessionPrompt {
                session_id, text, ..
            }) => {
                assert_eq!(session_id, session);
                assert_eq!(text, "and the 503?");
            }
            other => panic!("expected SendSessionPrompt, got {other:?}"),
        }

        let shown = keys(state);
        assert!(
            has(&shown, &format!("review-row-{workspace}-pr7")),
            "{shown:?}"
        );
        assert!(has(&shown, "review-chat-open"));
    }

    #[test]
    fn the_chat_reads_its_conversation_once_per_activity_and_draws_it() {
        use crate::connection::Command;
        use domain::{ConversationTurn, SessionConversation, TurnRole};
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        let run = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &run, Vec::new(), Vec::new(), None);
        let session = live_row(&mut state, workspace);
        assert!(state.review_chat_opened(run.id, session));
        while rx.try_recv().is_ok() {}
        state.workbench.mode = CenterMode::Review;

        state.poll_review_chat();
        match rx.try_recv() {
            Ok(Command::SessionConversation { session_id, .. }) => assert_eq!(session_id, session),
            other => panic!("expected SessionConversation, got {other:?}"),
        }
        state.poll_review_chat();
        assert!(rx.try_recv().is_err(), "one read in flight");

        let opening = format!(
            "{} of x.\n\n{}Is it safe?\n\n## Rules for this session\n\nRules.",
            model::CHAT_OPENING,
            model::FIRST_QUESTION
        );
        let turns = vec![
            ConversationTurn {
                role: TurnRole::User,
                text: opening,
                tools: Vec::new(),
            },
            ConversationTurn {
                role: TurnRole::Agent,
                text: "It is.".into(),
                tools: vec!["Read a.rs".into()],
            },
        ];
        assert!(state.apply_session_conversation(
            session,
            Ok(SessionConversation {
                session_id: session,
                available: true,
                turns,
                truncated: false,
            })
        ));
        state.poll_review_chat();
        assert!(
            rx.try_recv().is_err(),
            "the same activity is not read twice"
        );
        let shown = keys(state);
        assert!(has(&shown, "review-chat-question-1"), "{shown:?}");
        assert!(
            has(&shown, &format!("review-chat-answer-{session}-2")),
            "{shown:?}"
        );
    }

    #[test]
    fn a_chat_told_about_a_newer_run_answers_from_it() {
        use crate::connection::Command;
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        let first = completed(workspace, workspace_target(workspace));
        with_report(&mut state, &first, Vec::new(), Vec::new(), None);
        let session = live_row(&mut state, workspace);
        state.review_chat_opened(first.id, session);

        let mut second = completed(workspace, workspace_target(workspace));
        second.verdict = Some(Verdict::Ok);
        with_report(
            &mut state,
            &second,
            vec![finding(2, "b.rs", None, Severity::P2)],
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Some(first.id),
        );
        state.tell_review_chat();
        match rx.try_recv() {
            Ok(Command::SendSessionPrompt { text, .. }) => {
                assert!(text.contains("Resolved: [P1] a.rs:2"), "{text}");
                assert!(text.contains("Finding 2 [P2] b.rs"));
            }
            other => panic!("expected SendSessionPrompt, got {other:?}"),
        }
        state.tell_review_chat();
        assert!(rx.try_recv().is_err(), "one update in flight at a time");
        // One prompt waits per session, so a refused update must stay undone.
        state.review_command_result("SendSessionPrompt", false, "Conflict");
        assert_eq!(
            state.code_review.chats[&(workspace, Lane::Workspace)].run,
            first.id
        );
        state.tell_review_chat();
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::SendSessionPrompt { .. })
        ));
        state.review_command_result("SendSessionPrompt", true, "queued");
        assert_eq!(
            state.code_review.chats[&(workspace, Lane::Workspace)].run,
            second.id
        );
        state.tell_review_chat();
        assert!(
            rx.try_recv().is_err(),
            "an up-to-date chat is not told twice"
        );
    }

    #[test]
    fn a_refused_question_comes_back_to_its_box() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![finding(1, "a.rs", Some(2), Severity::P1)],
            Vec::new(),
            None,
        );
        let session = live_row(&mut state, workspace);
        state.review_chat_opened(run.id, session);
        state.ask_about_finding(1);
        state.code_review.question = TextEdit::new("still real?");
        state.ask_review_chat();
        assert!(rx.try_recv().is_ok());
        assert!(state.code_review.question.text.is_empty());
        state.review_command_result("SendSessionPrompt", false, "Conflict");
        assert_eq!(state.code_review.question.text, "still real?");
        assert_eq!(state.code_review.question_about, Some((run.id, 1)));
    }

    #[test]
    fn the_findings_list_moves_by_key_and_a_dismissed_choice_falls_back() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        with_report(
            &mut state,
            &run,
            vec![
                finding(1, "a.rs", Some(2), Severity::P1),
                finding(2, "a.rs", Some(9), Severity::P2),
            ],
            Vec::new(),
            None,
        );
        state.step_finding(true);
        assert_eq!(state.code_review.chosen, Some(2));
        state.react_to_finding(2, FindingReaction::Dismiss);
        let order = finding_order(&state.code_review, workspace);
        assert_eq!(shown_finding(&order, state.code_review.chosen), Some(1));
        let shown = keys(state);
        assert!(has(&shown, "review-finding-detail-1"), "{shown:?}");
        assert!(has(&shown, "review-finding-list"));
    }

    #[test]
    fn tree_rows_say_how_each_lane_ended() {
        let workspace = WorkspaceId::new();
        let mut done = completed(workspace, workspace_target(workspace));
        done.verdict = Some(Verdict::Block);
        assert_eq!(
            tree_status(&done),
            ("Blocks merge".to_owned(), crate::ds::Tone::Danger)
        );
        let live = run(ReviewId::new(), Some(ReviewStage::Model), false);
        assert_eq!(tree_status(&live).0, "Reviewing");
        assert_eq!(
            parse_lane_key(&lane_key(Lane::PullRequest(12))),
            Some(Lane::PullRequest(12))
        );
        assert_eq!(
            parse_lane_key(&lane_key(Lane::Workspace)),
            Some(Lane::Workspace)
        );
        assert_eq!(parse_lane_key("prx"), None);
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
        assert!(!has(&keys, "review-word-subject-trigger"));
        assert!(has(&keys, "review-sentence"));
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
    fn a_completed_run_leads_with_its_verdict_and_shows_reachability_hypotheses_and_scope() {
        use rune::{Env, Runtime};
        let (mut state, workspace) = fixture_state();
        let mut run = completed(workspace, workspace_target(workspace));
        let choice = |hypothesis, reason: &str| domain::HypothesisChoice {
            hypothesis,
            reason: reason.into(),
        };
        run.coverage.hypotheses = vec![
            choice(domain::Hypothesis::RaceCondition, "diff: lock ×2"),
            choice(domain::Hypothesis::AuthBypass, "default order"),
        ];
        run.coverage.acknowledged_scope = Some("- No Windows support.".into());
        let mut hunted = finding(1, "a.rs", Some(2), Severity::P1);
        hunted.hypothesis = Some(domain::Hypothesis::RaceCondition);
        hunted.title = "The lock is taken twice".into();
        hunted.reachability = Some(domain::Reachability {
            kind: domain::ReachabilityKind::Sequence,
            trigger: "two saves in a row".into(),
        });
        with_report(&mut state, &run, vec![hunted], vec![], None);
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
            let Some(at) = elements.iter().find(|e| e.key.as_deref() == Some(key)) else {
                return Vec::new();
            };
            elements
                .iter()
                .filter(|e| e.path.starts_with(&at.path))
                .filter_map(|e| e.text.clone())
                .collect()
        };
        assert_eq!(
            text_under("review-headline"),
            ["Do not merge until: The lock is taken twice"]
        );
        assert_eq!(
            text_under("review-reachability-1"),
            ["Trigger", "sequence", "two saves in a row"]
        );
        rt.state.code_review.view = ReviewView::Summary;
        rt.invalidate();
        rt.frame(2.0 / 60.0);
        let elements = rt.elements();
        let text_under = |key: &str| -> Vec<String> {
            let Some(at) = elements.iter().find(|e| e.key.as_deref() == Some(key)) else {
                return Vec::new();
            };
            elements
                .iter()
                .filter(|e| e.path.starts_with(&at.path))
                .filter_map(|e| e.text.clone())
                .collect()
        };
        assert_eq!(
            text_under("review-explored-RaceCondition"),
            ["RaceCondition · 1 finding · diff: lock ×2"]
        );
        assert_eq!(
            text_under("review-explored-AuthBypass"),
            ["AuthBypass · explored, no findings · default order"]
        );
        assert!(
            text_under("review-scope").contains(&"- No Windows support.".to_owned()),
            "{:?}",
            text_under("review-scope")
        );
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
            indexing: false,
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
            Ok(Some(vec![node]))
        ));
        assert!(!state.apply_graph_nodes(
            ReviewId::new(),
            "a.rs",
            &domain::GraphQuery::Imports("a.rs".into()),
            Ok(Some(vec![]))
        ));
        let graph = state.code_review.graph.as_ref().expect("graph");
        assert!(matches!(&graph.files[0].imports, Some(Ok(nodes)) if nodes.len() == 1));
        assert!(graph.files[0].external.is_none());
        state.code_review.view = ReviewView::Graph;
        let shown = keys(state);
        assert!(has(&shown, "review-graph"), "{shown:?}");
    }

    #[test]
    fn reads_left_waiting_by_a_dropped_connection_are_asked_again() {
        let (mut state, workspace) = fixture_state();
        let run = completed(workspace, workspace_target(workspace));
        let pending = |indexing| ReviewGraph {
            run: run.id,
            files: vec![GraphFile {
                path: "a.rs".into(),
                imports: Some(Ok(vec![])),
                external: None,
            }],
            indexing,
        };
        state.code_review.graph = Some(pending(false));
        state.code_review.sandbox = Some((run.id, Err(SANDBOX_READING.into())));
        reconnected(&mut state);
        assert!(state.code_review.graph.is_none());
        assert!(state.code_review.sandbox.is_none());

        let mut answered = pending(false);
        answered.files[0].external = Some(Ok(vec![]));
        state.code_review.graph = Some(answered);
        state.code_review.sandbox = Some((run.id, Err("no such run".into())));
        reconnected(&mut state);
        assert!(state.code_review.graph.is_some(), "an answered read stays");
        assert!(state.code_review.sandbox.is_some());
    }

    #[test]
    fn an_indexing_graph_waits_for_its_build_and_shows_a_failed_one() {
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
            indexing: false,
        });
        assert!(state.apply_graph_nodes(
            run.id,
            "a.rs",
            &domain::GraphQuery::Imports("a.rs".into()),
            Ok(None)
        ));
        assert!(state.code_review.graph.as_ref().expect("graph").indexing);

        graph_indexed(&mut state, Some("the checkout has no commit to index"));
        let graph = state.code_review.graph.as_ref().expect("graph");
        assert!(!graph.indexing);
        assert!(matches!(&graph.files[0].imports, Some(Err(error)) if error.contains("no commit")));

        state.code_review.graph.as_mut().expect("graph").indexing = true;
        graph_indexed(&mut state, None);
        assert!(
            state
                .code_review
                .graph
                .as_ref()
                .is_none_or(|graph| !graph.indexing),
            "a finished build asks again"
        );

        state.code_review.graph = Some(ReviewGraph {
            run: run.id,
            files: Vec::new(),
            indexing: true,
        });
        state.code_review.view = ReviewView::Graph;
        assert!(has(&keys(state), "review-graph-indexing"));
    }

    #[test]
    fn a_cited_knowledge_base_and_a_sandbox_run_have_their_sections() {
        let (mut state, workspace) = fixture_state();
        state.code_review.view = ReviewView::Summary;
        let mut run = completed(workspace, workspace_target(workspace));
        run.sandbox_run_id = Some("s1".into());
        let mut cited = finding(1, "a.rs", Some(2), Severity::P1);
        cited.kb_ref = Some("architecture.md#crates".into());
        with_report(&mut state, &run, vec![cited], vec![], None);
        state.code_review.kb_open = true;
        state.code_review.kb = Some(KbRead {
            run: run.id,
            workspace,
            files: Some(Ok(vec![domain::KbFile {
                file: "architecture.md".into(),
                content: "# Architecture\n\n## crates\n\n- 3 file(s)\n".into(),
                source: "daemon".into(),
                updated_at: None,
            }])),
        });
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
                unapproved_hash: None,
                started_at: domain::Timestamp::now(),
                finished_at: None,
            }),
        ));
        let shown = keys(state);
        assert!(has(&shown, "review-kb"), "{shown:?}");
        assert!(has(&shown, "review-kb-0"));
        assert!(has(&shown, "review-sandbox"));
        assert!(has(&shown, "review-sandbox-test"));
        assert!(!has(&shown, "review-sandbox-approve"));
    }

    #[test]
    fn an_unapproved_sandbox_file_offers_its_approval() {
        let (mut state, workspace) = fixture_state();
        state.code_review.view = ReviewView::Summary;
        let mut run = completed(workspace, workspace_target(workspace));
        run.sandbox_run_id = Some("s1".into());
        with_report(&mut state, &run, vec![], vec![], None);
        state.code_review.sandbox = Some((
            run.id,
            Ok(domain::SandboxRun {
                id: "s1".into(),
                workspace_id: workspace,
                review_id: Some(run.id),
                targets: Vec::new(),
                error: Some("checks.toml is not approved for this project; nothing ran".into()),
                unapproved_hash: Some("abc".into()),
                started_at: domain::Timestamp::now(),
                finished_at: None,
            }),
        ));
        let shown = keys(state);
        assert!(has(&shown, "review-sandbox-approve"), "{shown:?}");
    }

    #[test]
    fn approving_checks_toml_shows_the_file_first_and_says_approved_only_on_the_ack() {
        let (mut state, workspace) = fixture_state();
        let rx = connected(&mut state);
        state.confirm_sandbox(workspace, "abc".into());
        assert!(matches!(
            rx.try_recv(),
            Ok(crate::connection::Command::ReadSandboxChecks { workspace_id, .. }) if workspace_id == workspace
        ));
        state.approve_sandbox();
        assert!(
            rx.try_recv().is_err(),
            "nothing is approved before it is shown"
        );
        let file = "[[targets]]\nname = \"test\"\ncommand = \"cargo test\"\n";
        assert!(state.apply_sandbox_file(workspace, Ok(file.into())));

        state.approve_sandbox();
        assert!(matches!(
            rx.try_recv(),
            Ok(crate::connection::Command::ApproveSandboxChecks { file_hash, .. }) if file_hash == "abc"
        ));
        assert!(state.code_review.sandbox_confirm.is_none());
        assert_eq!(
            state.code_review.sandbox_approved, None,
            "not before the ack"
        );
        state.review_command_result("ApproveSandboxChecks", false, "checks.toml changed");
        assert_eq!(state.code_review.sandbox_approved, None);
        assert_eq!(state.code_review.sandbox_approving, None);

        state.code_review.sandbox_approving = Some("abc".into());
        state.review_command_result("ApproveSandboxChecks", true, "approved");
        assert_eq!(state.code_review.sandbox_approved.as_deref(), Some("abc"));
    }

    #[test]
    fn the_checks_toml_confirmation_shows_its_commands_and_its_hash() {
        let (mut state, workspace) = fixture_state();
        state.code_review.sandbox_confirm = Some(SandboxConfirm {
            workspace_id: workspace,
            hash: "abc".into(),
            file: Some(Ok(
                "[[targets]]\nname = \"test\"\ncommand = \"cargo test\"\n".into(),
            )),
        });
        let shown = keys(state);
        assert!(has(&shown, "review-sandbox-confirm-file"), "{shown:?}");
        assert!(has(&shown, "review-sandbox-confirm-approve"));
        assert!(has(&shown, "review-sandbox-confirm-cancel"));
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
