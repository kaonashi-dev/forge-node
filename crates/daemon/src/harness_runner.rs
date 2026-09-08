//! The harness cycle, run as jobs instead of as terminals somebody watches.
//!
//! [`crate::harness_io`] is the state on disk; [`crate::jobs`] is how an agent
//! runs without a PTY. This is the piece between them: it starts the job a
//! step needs, and when that job exits it writes the step's outcome to
//! `harness/` and starts the next one.
//!
//! The whole design follows from one property jobs have and sessions do not —
//! **a job ends, observably**. That is what lets the cycle advance on its own:
//!
//! ```text
//!   Spec ──exit 0──▶ spec_ready ──▶ [human gate]
//!                                        │ approve
//!                                        ▼
//!                    Implement ──exit 0──▶ Review ──APPROVED──▶ done
//!                        ▲                        │
//!                        └── CHANGES_REQUESTED ───┘  (up to max_review_rounds)
//! ```
//!
//! Two rules keep it honest. A step that exits non-zero **blocks** the feature
//! with the reason rather than advancing past it; and the human gate is never
//! automated — the machine stops at `spec_ready` and only a
//! `HarnessAdvance::ApproveSpec` from a person starts the implementation.
//!
//! Which provider runs which step is the user's choice, read from the same
//! `app_state` keys the GUI's Harness settings write. A step can therefore be
//! reviewed by Claude and implemented by Codex without either knowing the
//! other exists: they meet only in `harness/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use domain::{AgentProviderId, HarnessStep, Job, JobRequest, JobState, ProjectId, SessionRole};
use harness_service::HarnessTrigger;
use protocol::{error::ProtocolError, response::Response, ErrorCode};

use crate::core::Daemon;

/// The `app_state` key naming the provider for each step.
///
/// The same keys the GUI's Harness settings write (`ui.harness.*`): the choice
/// belongs to the user, and there is no second place to set it.
fn provider_key(step: HarnessStep) -> &'static str {
    match step {
        HarnessStep::Spec => "ui.harness.orchestrator",
        HarnessStep::Implement => "ui.harness.executor",
        HarnessStep::Review => "ui.harness.reviewer",
        // A step added to the protocol after this build: reviewed by the one
        // agent whose job is to say no.
        _ => "ui.harness.reviewer",
    }
}

/// The graph role a step's job takes, so the tree reads the same whether the
/// step ran headless or in a terminal.
fn role(step: HarnessStep) -> SessionRole {
    match step {
        HarnessStep::Spec => SessionRole::Orchestrator,
        HarnessStep::Implement => SessionRole::Executor,
        HarnessStep::Review => SessionRole::Reviewer,
        _ => SessionRole::Generic,
    }
}

/// The verdict shape the reviewer is asked to answer in.
///
/// Small on purpose: everything else the reviewer has to say belongs in
/// `progress/review_<id>.md`, which is prose for a human. This is the part the
/// machine acts on, and it either parses or it does not.
///
/// The vocabulary is `.claude/agents/reviewer.md`'s, not a second one: this
/// enum said `REJECTED` while every prompt said `CHANGES_REQUESTED`, and a
/// settlement that only understood one of them sent genuine rejections to
/// `blocked` instead of round two.
const VERDICT_SCHEMA: &str = r#"{"type":"object","properties":{"verdict":{"type":"string","enum":["APPROVED","CHANGES_REQUESTED"]},"summary":{"type":"string"}},"required":["verdict"],"additionalProperties":false}"#;

/// What a review file said, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Approved,
    ChangesRequested,
    /// No verdict line at all — including a template line left unfilled.
    Missing,
    /// Two verdict lines that disagree.
    Ambiguous,
}

/// Read the verdict out of a review file.
///
/// A *line*, never a substring of the document. `review.contains("APPROVED")`
/// was the original rule and it approved on the word wherever it appeared —
/// including in `.claude/agents/reviewer.md`'s own format example, which
/// prints `**Verdict:** APPROVED | CHANGES_REQUESTED`, and in the ordinary
/// prose of a rejection ("cannot be approved"). A reviewer that copied the
/// template, or hedged in English, silently marked the feature `done`.
///
/// `REJECTED` is accepted beside `CHANGES_REQUESTED` because review files
/// written by earlier builds say it, and a settlement that could not read its
/// own history would block them.
fn parse_verdict(review: &str) -> Verdict {
    let mut seen: Option<Verdict> = None;
    for line in review.lines() {
        let Some(verdict) = verdict_on_line(line) else {
            continue;
        };
        match seen {
            None => seen = Some(verdict),
            Some(previous) if previous == verdict => {}
            // A reviewer that wrote both answered neither.
            Some(_) => return Verdict::Ambiguous,
        }
    }
    seen.unwrap_or(Verdict::Missing)
}

/// The verdict a single line states, or `None` when it states none.
///
/// Tolerates the markdown a reviewer actually writes — a bullet, a heading, an
/// emphasised `**Verdict:**` label — and then demands that what is left be
/// *exactly* one token. That last part is the whole point: it is what makes
/// the unfilled `APPROVED | CHANGES_REQUESTED` template line match nothing.
fn verdict_on_line(line: &str) -> Option<Verdict> {
    let text = line.trim().trim_start_matches(['-', '*', '#', '>', ' ']);
    let text = text.trim_start_matches(['*', '_']).trim();
    // An optional `Verdict:` label, in any case.
    let text = match text.split_once(':') {
        Some((label, rest))
            if label
                .trim_matches(['*', '_', ' '])
                .eq_ignore_ascii_case("verdict") =>
        {
            rest
        }
        _ => text,
    };
    let text = text.trim().trim_matches(['*', '_', '`', ' ']).trim();
    match text.to_ascii_uppercase().as_str() {
        "APPROVED" => Some(Verdict::Approved),
        "CHANGES_REQUESTED" | "REJECTED" => Some(Verdict::ChangesRequested),
        _ => None,
    }
}

/// A harness refusal as a protocol error.
///
/// The transition table refusing a step is `InvalidRequest` and not a crash:
/// "you cannot implement a spec nobody approved" is an answer, and the client
/// shows it.
fn harness_refusal(error: harness_service::HarnessError) -> ProtocolError {
    crate::harness_io::harness_err(error)
}

/// The reviewer's structured answer, from the job's own transcript.
///
/// The schema is asked for on every review (`JobRequest::schema`) and, until
/// now, thrown away: the verdict was read from the markdown with a prose
/// parser, so a provider that had *answered the question* was second-guessed by
/// a regex over its prose.
///
/// Keyed on the field **name**, never on the words: the prompt that asks for
/// `APPROVED` is echoed in the same transcript, and matching the value alone
/// would read the question as the answer. Scanned backwards because a run's
/// last word is its answer.
fn verdict_in_log(path: &Path) -> Option<Verdict> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|value| verdict_in_json(&value, 0))
}

/// Depth-limited search for a `verdict` field, including one nested inside a
/// string that is itself JSON — which is how both dialects wrap a schema
/// answer (`{"type":"result","result":"{\"verdict\":…}"}`).
fn verdict_in_json(value: &serde_json::Value, depth: u8) -> Option<Verdict> {
    if depth > 6 {
        return None;
    }
    match value {
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("verdict").and_then(serde_json::Value::as_str) {
                if let Some(verdict) = verdict_on_line(text) {
                    return Some(verdict);
                }
            }
            map.values()
                .find_map(|child| verdict_in_json(child, depth + 1))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|child| verdict_in_json(child, depth + 1)),
        serde_json::Value::String(text) => serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|nested| verdict_in_json(&nested, depth + 1)),
        _ => None,
    }
}

impl Daemon {
    /// Start one step of the harness cycle for a feature.
    ///
    /// Answers with the job, which may still be queued. The step's *outcome*
    /// arrives later, as a `JobUpdated` in a final state plus whatever this
    /// module then wrote to `harness/`.
    pub(crate) fn run_harness_step(
        self: &Arc<Self>,
        project_id: ProjectId,
        feature_id: u32,
        step: HarnessStep,
    ) -> Result<Response, ProtocolError> {
        self.run_harness_step_forced(project_id, feature_id, step, false)
    }

    /// The same, with the transition table optionally overridden.
    ///
    /// `force` exists so an operator with a shell can unstick a state this
    /// build's table has no row for; it is logged as `gate_bypassed` and the
    /// GUI never sets it.
    pub(crate) fn run_harness_step_forced(
        self: &Arc<Self>,
        project_id: ProjectId,
        feature_id: u32,
        step: HarnessStep,
        force: bool,
    ) -> Result<Response, ProtocolError> {
        let root = self.project_root_for(project_id)?;
        let feature = harness_service::get_feature(&root, feature_id)
            .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, e.to_string()))?;
        let Some(workspace_id) = feature.workspace_id else {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "this feature is not bound to a checkout, so nothing can be run for it; \
                 register it from the Feature tab",
            ));
        };
        // Single flight, per feature. `DEFAULT_MAX_CONCURRENT_JOBS` is a global
        // ceiling and says nothing about *which* jobs: a click on *Run
        // implement* racing the automatic Implement → Review chain used to put
        // two agents in one worktree, editing the same files.
        if let Some(live) = self.live_job_for_feature(feature_id) {
            return Err(ProtocolError::conflict(format!(
                "feature {feature_id} already has a step running (job {live}); \
                 cancel it before starting another"
            )));
        }
        let provider_id = self.harness_provider(step)?;
        // The attempt this run will be. A retry is not a fresh start: the
        // previous agent may have left the working tree half-edited, and a
        // prompt that did not say so invited the next one to start again from
        // a state it did not expect.
        let attempt = harness_service::attempts_of(&feature, step) + 1;
        let prompt = step_prompt(step, feature_id, &feature, &root, attempt);
        let schema = matches!(step, HarnessStep::Review).then(|| VERDICT_SCHEMA.to_owned());

        // The id is minted here rather than inside `start_job` so the event
        // can name it. Jobs live in the daemon's memory only: restart it and
        // the Feature tab says "no step is on record", while the transcript is
        // still on disk with nothing pointing at it. This line is that pointer.
        let job_id = domain::JobId::new();

        // The transition is what decides whether this step may run at all, and
        // it writes the attempt before the process exists: a run that dies in
        // its first second still leaves a row saying it was attempted.
        let started = harness_service::apply(
            &root,
            feature_id,
            None,
            &HarnessTrigger::StartStep {
                step,
                job: Some(job_id.to_string()),
                provider: Some(provider_id.to_string()),
                transport: Some("cli".to_owned()),
                force,
            },
        )
        .map_err(harness_refusal)?;
        debug_assert!(started.applied, "an unconditional apply always applies");
        self.broadcast_harness_feature(project_id, &root, feature_id);

        let launched = self.start_job_with_id(
            job_id,
            JobRequest {
                provider_id,
                workspace_id,
                role: role(step),
                feature_id: Some(feature_id),
                parent_session_id: None,
                prompt,
                resume_from: None,
                schema,
            },
        );
        if let Err(error) = &launched {
            // The attempt is on record and no process will ever settle it, so
            // it is settled here — through the same budget a crashed run gets.
            self.settle_step_failure(
                project_id,
                &root,
                feature_id,
                step,
                format!("the {step:?} step could not start: {}", error.message),
            );
        }
        launched
    }

    /// The job still in flight for a feature, queued or running.
    pub(crate) fn live_job_for_feature(&self, feature_id: u32) -> Option<domain::JobId> {
        let inner = self.lock();
        inner
            .jobs
            .values()
            .find(|job| job.feature_id == Some(feature_id) && !job.state.is_final())
            .map(|job| job.id)
    }

    /// Answer a question about the harness, as one headless run.
    ///
    /// The operator's counterpart to the cycle: the steps run the work, this
    /// says what the work is doing. It is deliberately the same machinery —
    /// a job, a prompt, a stream — because the alternative is a second kind of
    /// agent with a second set of failure modes for something that is, at
    /// bottom, one question and one answer.
    ///
    /// The state goes in the prompt rather than being left for the agent to
    /// discover: `features.json` is one file and reading it is three tool
    /// calls the answer does not need. What is *not* in the briefing — a
    /// spec's text, a review's reasoning — the agent can still go and read,
    /// because it runs in the checkout with the harness root in its
    /// environment like every other job.
    pub(crate) fn ask_harness(
        self: &Arc<Self>,
        project_id: ProjectId,
        question: String,
        resume_from: Option<String>,
    ) -> Result<Response, ProtocolError> {
        let question = question.trim();
        if question.is_empty() {
            return Err(ProtocolError::new(ErrorCode::InvalidRequest, "ask what?"));
        }
        let root = self.project_root_for(project_id)?;
        let workspace_id = self.main_workspace_of(project_id).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                "this project has no checkout to run an agent in",
            )
        })?;
        // Whoever runs the spec: the question is about the same cycle, and a
        // separate setting would be one more thing to configure for no gain.
        let provider_id = self.harness_provider(HarnessStep::Spec)?;
        let briefing = harness_briefing(&root);
        let prompt = format!(
            "You are the harness lieutenant for this repository. Answer the operator's \
             question about the state of the feature harness.\n\n\
             Harness state lives in {}/harness — read files there if the briefing below \
             is not enough. Do not change any harness state and do not write code; you \
             are being asked, not told.\n\n\
             Answer in a few sentences, plainly, and say plainly when something is not \
             knowable from what you can see.\n\n\
             === Harness state ===\n{briefing}\n\
             === Operator's question ===\n{question}\n",
            root.display()
        );

        self.start_job(JobRequest {
            provider_id,
            workspace_id,
            role: SessionRole::Generic,
            feature_id: None,
            parent_session_id: None,
            prompt,
            resume_from,
            schema: None,
        })
    }

    /// The project's own checkout, which is where a question is answered.
    ///
    /// A worktree would do as well for reading, but the main checkout is the
    /// one that owns `harness/`, so the answer is read from the state itself
    /// rather than from a snapshot of it.
    fn main_workspace_of(&self, project_id: ProjectId) -> Option<domain::WorkspaceId> {
        let inner = self.lock();
        let root = inner.projects.get(&project_id)?.root_path.clone();
        inner
            .workspaces
            .values()
            .find(|workspace| workspace.project_id == project_id && workspace.path == root)
            .or_else(|| {
                inner
                    .workspaces
                    .values()
                    .find(|workspace| workspace.project_id == project_id)
            })
            .map(|workspace| workspace.id)
    }

    /// The provider one step runs on.
    ///
    /// `provider:<id>` is the only spelling resolved here. A profile or the
    /// "default" choice is a GUI notion — it depends on what that client has
    /// visible — so an unset or unresolvable choice falls back to the first
    /// installed provider that can run headless at all, which is the answer a
    /// user would give if asked "just use whatever works".
    fn harness_provider(&self, step: HarnessStep) -> Result<AgentProviderId, ProtocolError> {
        let choice = self.app_state_value(provider_key(step));
        if let Some(id) = choice
            .as_deref()
            .and_then(|value| value.trim().strip_prefix("provider:"))
            .filter(|id| !id.is_empty())
        {
            return Ok(AgentProviderId::new(id));
        }
        self.first_headless_provider().ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                "no agent is configured for this harness step, and none that can run \
                 headless is installed; pick one in Settings → Harness",
            )
        })
    }

    /// React to a job that has just reached a final state.
    ///
    /// Called from the job reaper, which is why it takes the finished row
    /// rather than looking one up: by the time this runs the process is gone
    /// and its exit code is the only thing that decides what happens next.
    pub(crate) fn advance_harness_after_job(self: &Arc<Self>, job: &Job) {
        let (Some(feature_id), Some(step)) = (job.feature_id, step_of(job)) else {
            return;
        };
        let Some((project_id, root)) = self.project_of_workspace(job.workspace_id) else {
            return;
        };
        if let Ok(feature) = harness_service::get_feature(&root, feature_id) {
            if job_is_stale(&feature, job) {
                tracing::info!(
                    %job.id,
                    feature_id,
                    "ignoring a job the feature is no longer waiting on"
                );
                return;
            }
        }
        match job.state {
            JobState::Succeeded => {
                self.harness_step_succeeded(project_id, &root, feature_id, step, job);
            }
            JobState::Failed => {
                let reason = format!(
                    "the {step:?} agent exited {}; see {}",
                    job.exit_code.unwrap_or(-1),
                    job.log_path.display()
                );
                self.settle_step_failure(project_id, &root, feature_id, step, reason);
            }
            // A cancel is a decision, not a failure: the feature keeps the
            // status it had and waits for whatever the user does next.
            _ => {}
        }
        self.broadcast_harness_feature(project_id, &root, feature_id);
    }

    /// A step that ended badly, through the retry budget.
    ///
    /// The distinction the old code could not make: a rate limit, a network
    /// blip and a genuine "I could not do this" all reached
    /// `block_harness_feature` and stopped the feature dead. The table spends
    /// `max_step_attempts` first and only then blocks — and it says which of
    /// the two happened in the event log.
    fn settle_step_failure(
        self: &Arc<Self>,
        project_id: ProjectId,
        root: &Path,
        feature_id: u32,
        step: HarnessStep,
        reason: String,
    ) {
        match harness_service::apply(
            root,
            feature_id,
            None,
            &HarnessTrigger::StepFailed { step, reason },
        ) {
            Ok(applied) => self.start_next(project_id, root, feature_id, applied.next_step),
            Err(error) => {
                tracing::warn!(feature_id, %error, "could not settle a failed harness step");
            }
        }
    }

    /// Start whatever the transition table said comes next.
    ///
    /// A launch that fails blocks the feature with the reason rather than
    /// leaving a row that says `in_progress` with nothing running in it.
    fn start_next(
        self: &Arc<Self>,
        project_id: ProjectId,
        root: &Path,
        feature_id: u32,
        next: Option<HarnessStep>,
    ) {
        let Some(step) = next else {
            return;
        };
        if let Err(error) = self.run_harness_step(project_id, feature_id, step) {
            self.block_harness_feature(
                root,
                feature_id,
                format!("could not start the {step:?} step: {}", error.message),
            );
        }
    }

    /// What a finished step means for the feature.
    fn harness_step_succeeded(
        self: &Arc<Self>,
        project_id: ProjectId,
        root: &Path,
        feature_id: u32,
        step: HarnessStep,
        job: &Job,
    ) {
        let trigger = match step {
            // The spec is written; the machine stops here unless the
            // repository has written `require_human_spec_approval: false`.
            // Exit 0 alone does not say that: an agent that could not write
            // its documents still exits 0, and a gate with nothing to read
            // is not a gate.
            HarnessStep::Spec => match missing_gate_documents(root, feature_id) {
                Ok(missing) if missing.is_empty() => HarnessTrigger::SpecWritten,
                Ok(missing) => {
                    self.block_harness_feature(
                        root,
                        feature_id,
                        format!(
                            "the spec step exited 0 but left no usable gate documents: {}. \
                             Re-run the spec step once the agent can write them.",
                            missing.join(", ")
                        ),
                    );
                    return;
                }
                Err(error) => {
                    self.block_harness_feature(
                        root,
                        feature_id,
                        format!("could not read the spec step's documents: {error}"),
                    );
                    return;
                }
            },
            HarnessStep::Implement => HarnessTrigger::Implemented,
            HarnessStep::Review => {
                self.settle_review(project_id, root, feature_id, job);
                return;
            }
            // A step this build does not know: the feature keeps its status
            // and waits for a person rather than being advanced by a guess.
            _ => return,
        };
        match harness_service::apply(root, feature_id, None, &trigger) {
            Ok(applied) => self.start_next(project_id, root, feature_id, applied.next_step),
            Err(error) => {
                self.block_harness_feature(root, feature_id, error.to_string());
            }
        }
    }

    /// Close the feature, or send it back to the implementer.
    ///
    /// The reviewer is asked for its answer twice — once as structured output
    /// against [`VERDICT_SCHEMA`], once as the last line of
    /// `progress/review_<id>.md` — and the schema is believed first. A provider
    /// that supports a schema *answered a question*; the markdown is the human
    /// artefact and a prose parser over it is a good fallback and a bad
    /// primary. A provider with no schema still settles on the file, which is
    /// why both paths stay.
    ///
    /// A review that says neither — or both — leaves the feature blocked rather
    /// than guessing: "the reviewer did not answer" is a real outcome.
    fn settle_review(
        self: &Arc<Self>,
        project_id: ProjectId,
        root: &Path,
        feature_id: u32,
        job: &Job,
    ) {
        let (verdict, source) = match verdict_in_log(&job.log_path) {
            Some(verdict) => (verdict, "schema"),
            None => {
                let review = harness_service::read_artifact(
                    root,
                    feature_id,
                    domain::HarnessArtifactKind::Review,
                )
                .unwrap_or_default();
                (parse_verdict(&review), "file")
            }
        };
        let trigger = match verdict {
            Verdict::Approved => HarnessTrigger::ReviewVerdict {
                approved: true,
                source,
            },
            Verdict::ChangesRequested => HarnessTrigger::ReviewVerdict {
                approved: false,
                source,
            },
            // Both of these block, with different reasons, because they need
            // different fixes: one reviewer never answered, the other answered
            // twice.
            Verdict::Ambiguous => HarnessTrigger::ReviewUnreadable {
                reason: "the review states more than one verdict; \
                         progress/review_<id>.md must carry exactly one"
                    .to_owned(),
            },
            Verdict::Missing => {
                // Say what was actually read: "no verdict" with nothing else is
                // the same message whether the reviewer hedged or never wrote
                // the file at all, and those need different fixes.
                let review = harness_service::read_artifact(
                    root,
                    feature_id,
                    domain::HarnessArtifactKind::Review,
                )
                .unwrap_or_default();
                let seen = review.trim();
                let seen = if seen.is_empty() {
                    "progress/review_<id>.md is empty or missing".to_owned()
                } else {
                    format!("it ends: {}", seen.lines().last().unwrap_or_default())
                };
                HarnessTrigger::ReviewUnreadable {
                    reason: format!(
                        "the review has no line that is exactly APPROVED or \
                         CHANGES_REQUESTED — {seen}"
                    ),
                }
            }
        };
        match harness_service::apply(root, feature_id, None, &trigger) {
            Ok(applied) => self.start_next(project_id, root, feature_id, applied.next_step),
            Err(error) => self.block_harness_feature(root, feature_id, error.to_string()),
        }
    }

    fn block_harness_feature(&self, root: &Path, feature_id: u32, reason: String) {
        tracing::warn!(feature_id, reason, "harness feature blocked");
        let _ = harness_service::apply(root, feature_id, None, &HarnessTrigger::Block { reason });
    }

    /// Watch what is running and say — or do — something about it.
    ///
    /// Two budgets, deliberately different in kind. `stale_after_minutes` only
    /// *warns*: a long implementation legitimately thinks for minutes and a
    /// watchdog that killed on silence would be worse than none. `max_step_minutes`
    /// cancels, and the cancellation is a failed attempt, so it reaches the
    /// same retry budget a crash does rather than leaving the feature running
    /// forever in a row nobody will ever settle.
    pub(crate) fn run_harness_watchdog(self: &Arc<Self>) {
        let mut warned: std::collections::HashSet<domain::JobId> = std::collections::HashSet::new();
        loop {
            std::thread::sleep(crate::jobs::WATCHDOG_TICK);
            if self.is_shutting_down() {
                return;
            }
            let running: Vec<Job> = {
                let inner = self.lock();
                inner
                    .jobs
                    .values()
                    .filter(|job| job.state == JobState::Running && job.feature_id.is_some())
                    .cloned()
                    .collect()
            };
            warned.retain(|id| running.iter().any(|job| job.id == *id));
            let now = time::OffsetDateTime::now_utc();
            for job in running {
                let Some((project_id, root)) = self.project_of_workspace(job.workspace_id) else {
                    continue;
                };
                let rules = harness_service::rules_of(&root).unwrap_or_default();
                let ran_for = now - job.started_at.as_offset();
                if ran_for.whole_minutes() >= i64::from(rules.max_step_minutes) {
                    self.expire_job(
                        job.id,
                        format!(
                            "the step ran for {} minutes, past max_step_minutes ({})",
                            ran_for.whole_minutes(),
                            rules.max_step_minutes
                        ),
                    );
                    continue;
                }
                let quiet_since = job.last_output_at.unwrap_or(job.started_at).as_offset();
                let quiet = now - quiet_since;
                if quiet.whole_minutes() >= i64::from(rules.stale_after_minutes)
                    && warned.insert(job.id)
                {
                    let Some(feature_id) = job.feature_id else {
                        continue;
                    };
                    let mut data = serde_json::Map::new();
                    data.insert("job".into(), job.id.to_string().into());
                    data.insert("quiet_minutes".into(), quiet.whole_minutes().into());
                    let _ = harness_service::append_event(&root, feature_id, "step_stale", data);
                    tracing::warn!(
                        %job.id,
                        feature_id,
                        minutes = quiet.whole_minutes(),
                        "a harness step has been quiet"
                    );
                    self.broadcast_harness_feature(project_id, &root, feature_id);
                }
            }
        }
    }

    /// Settle the steps that died with the previous daemon.
    ///
    /// Jobs live in memory, so a restart forgets every process it was
    /// following while the *row* still says `in_progress` and the attempt on it
    /// still says it is running. Nothing was ever going to settle those, and
    /// `read_step_trace` — the primitive written to answer exactly this — had
    /// no callers at all. Each is settled as a failure and therefore goes
    /// through `max_step_attempts` like any other: retried once, then blocked
    /// with a reason the timeline states.
    pub(crate) fn recover_harness_after_restart(self: &Arc<Self>) {
        let projects: Vec<(ProjectId, PathBuf)> = {
            let inner = self.lock();
            inner
                .projects
                .values()
                .map(|project| (project.id, project.root_path.clone()))
                .collect()
        };
        for (project_id, root) in projects {
            let Ok(list) = harness_service::list_features(&root) else {
                continue;
            };
            for feature in list.features.iter().filter(|f| f.is_open()) {
                // Two records of the same fact, because rows written before
                // `attempts[]` existed have only the second: the attempt on the
                // row, and the event log's unsettled `*_started`. Either one
                // means a step was entered and nothing ever closed it.
                let unsettled = harness_service::live_attempt(feature).is_some()
                    || harness_service::read_step_trace(&root, feature.id)
                        .is_ok_and(|trace| trace.unsettled.is_some());
                if !unsettled {
                    // A feature merely waiting at the gate is not a dead run.
                    continue;
                }
                let step = harness_service::live_attempt(feature)
                    .map(|attempt| attempt.step)
                    .or_else(|| harness_service::step_of_status(&feature.status));
                let Some(step) = step else { continue };
                tracing::info!(
                    feature_id = feature.id,
                    ?step,
                    "settling a harness step that outlived its daemon"
                );
                self.settle_step_failure(
                    project_id,
                    &root,
                    feature.id,
                    step,
                    "the daemon restarted while this step was running".to_owned(),
                );
                self.broadcast_harness_feature(project_id, &root, feature.id);
            }
        }
    }

    /// Tell every client the feature row changed under them.
    fn broadcast_harness_feature(&self, project_id: ProjectId, root: &Path, feature_id: u32) {
        if let Ok(feature) = harness_service::get_feature(root, feature_id) {
            self.registry
                .broadcast_domain(protocol::event::DaemonEvent::HarnessFeatureChanged {
                    project_id,
                    feature: Box::new(feature),
                });
        }
    }

    /// The project a checkout belongs to, and that project's root.
    fn project_of_workspace(
        &self,
        workspace_id: domain::WorkspaceId,
    ) -> Option<(ProjectId, PathBuf)> {
        let inner = self.lock();
        let workspace = inner.workspaces.get(&workspace_id)?;
        let project = inner.projects.get(&workspace.project_id)?;
        Some((project.id, project.root_path.clone()))
    }
}

/// Whether a finished job is one the feature has stopped waiting on.
///
/// A human `Block` settles the live attempt and cancels its job, but the
/// process can still reach the reaper with an exit code — it was past the
/// kill, or it exited in the same instant. Settling the feature on that exit
/// would undo the decision: a failure spends a retry and puts the row back to
/// work, a success is refused and overwrites the human's reason. A row written
/// before `attempts[]` existed, or a live attempt that names no job, has
/// nothing to compare against and settles as before.
fn job_is_stale(feature: &domain::HarnessFeature, job: &Job) -> bool {
    let has_attempts = feature
        .attempts
        .as_deref()
        .is_some_and(|attempts| !attempts.is_empty());
    if !has_attempts {
        return false;
    }
    match harness_service::live_attempt(feature) {
        Some(live) => match live.job.as_deref() {
            Some(expected) => expected != job.id.to_string(),
            None => false,
        },
        None => true,
    }
}

/// The step a job was run for, from the role it carries.
fn step_of(job: &Job) -> Option<HarnessStep> {
    match job.role {
        SessionRole::Orchestrator => Some(HarnessStep::Spec),
        SessionRole::Executor => Some(HarnessStep::Implement),
        SessionRole::Reviewer => Some(HarnessStep::Review),
        _ => None,
    }
}

/// Every feature and where it stands, small enough to put in a prompt.
///
/// One line each, in the vocabulary the harness itself uses, so an agent that
/// then goes and reads `features.json` finds the same words rather than a
/// paraphrase it has to reconcile.
fn harness_briefing(root: &Path) -> String {
    let Ok(list) = harness_service::list_features(root) else {
        return "(the harness state could not be read)".to_owned();
    };
    if list.features.is_empty() {
        return "(no features are registered)".to_owned();
    }
    list.features
        .iter()
        .map(|feature| {
            let rounds = feature.review_rounds.unwrap_or(0);
            let checkout = feature.workspace_path.as_deref().unwrap_or("(no checkout)");
            format!(
                "#{} {} — status {} · review rounds {} · {}",
                feature.id, feature.slug, feature.status, rounds, checkout
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Display paths of the gate documents a spec step should have written.
///
/// Empty counts as missing: an agent that could not write still exits 0, and
/// the daemon answers that path with an empty string rather than an error.
fn missing_gate_documents(root: &Path, feature_id: u32) -> Result<Vec<String>, String> {
    let feature = harness_service::get_feature(root, feature_id).map_err(|e| e.to_string())?;
    let specs = format!("harness/specs/{feature_id}-{}", feature.slug);
    let wanted = [
        (
            domain::HarnessArtifactKind::Requirements,
            format!("{specs}/requirements.md"),
        ),
        (
            domain::HarnessArtifactKind::Design,
            format!("{specs}/design.md"),
        ),
        (
            domain::HarnessArtifactKind::Tasks,
            format!("{specs}/tasks.md"),
        ),
        (
            domain::HarnessArtifactKind::Gate,
            format!("harness/progress/gate_{feature_id}.md"),
        ),
    ];
    let mut missing = Vec::new();
    for (kind, display) in wanted {
        let present = harness_service::read_artifact(root, feature_id, kind)
            .map(|text| !text.trim().is_empty())
            .unwrap_or(false);
        if !present {
            missing.push(display);
        }
    }
    Ok(missing)
}

/// What each step is told to do.
///
/// The prompts name the repository's own skills rather than restating the
/// protocol: the harness rules live in `.claude/skills/` and `AGENTS.md`, and
/// a prompt that repeated them would be a second copy to keep in sync.
///
/// Every harness path is spelled **absolutely**, from the checkout that owns
/// `harness/`. A step runs in a worktree, and that worktree carries its own
/// frozen copy of `harness/` from the commit it was made at — so an agent told
/// to write `harness/specs/9-…/` writes it beside itself, where nothing else
/// will ever look. The feature then reaches the human gate with its status
/// advanced and not one artefact where the gate reads: `scripts/harness
/// validate` calls that incoherent, and it is right. Naming the root leaves
/// nothing to resolve.
fn step_prompt(
    step: HarnessStep,
    feature_id: u32,
    feature: &domain::HarnessFeature,
    root: &Path,
    attempt: u32,
) -> String {
    let task = feature.spec_raw.as_deref().unwrap_or("").trim();
    let slug = &feature.slug;
    let harness = root.join("harness");
    let harness = harness.display();
    let retry = if attempt > 1 {
        format!(
            "This is attempt {attempt} of this step. The previous attempt did not \
             finish, so the working tree may already carry a partial change from \
             it: check `git status` and `git diff` before you start, and continue \
             from what is there rather than assuming a clean tree.\n\n"
        )
    } else {
        String::new()
    };
    let rule = format!(
        "{retry}Harness state lives in {harness} — the checkout that owns it, which is \
         *not* the directory you are standing in. Write every spec, gate, \
         progress and event file under that absolute path; the copy of \
         harness/ beside you is a frozen snapshot and edits to it are lost. \
         Code, tests and the gate command belong in your own working tree.\n\n"
    );
    match step {
        HarnessStep::Spec => format!(
            "Run the /feature skill for harness feature {feature_id} ({slug}).\n\n\
             {rule}\
             Research the codebase, write the spec under \
             {harness}/specs/{feature_id}-{slug}/, write \
             {harness}/progress/gate_{feature_id}.md, and stop at the human gate. \
             Do not implement anything.\n\n\
             The user's task (spec_raw — pass verbatim into the spec):\n\n{task}\n"
        ),
        HarnessStep::Implement => format!(
            "Run the /feature-go skill for harness feature {feature_id} ({slug}).\n\n\
             {rule}\
             The spec is approved. Read {harness}/specs/{feature_id}-{slug}/ and \
             {harness}/progress/context_{feature_id}.md, implement it, and run the gate \
             command until it is green. Write {harness}/progress/impl_{feature_id}.md when \
             you are done. If a previous review rejected the work, read \
             {harness}/progress/review_{feature_id}.md first and address it.\n"
        ),
        HarnessStep::Review | _ => format!(
            "Review harness feature {feature_id} ({slug}) against \
             {harness}/specs/{feature_id}-{slug}/ and {harness}/CHECKPOINTS.md.\n\n\
             {rule}\
             Write your findings to {harness}/progress/review_{feature_id}.md and end that \
             file with a line that is exactly `APPROVED` or exactly \
             `CHANGES_REQUESTED` — one of the two words alone, not both, and not \
             inside a sentence. Answer with the same verdict in your structured \
             output. Do not edit any code.\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this parser replaced: `review.contains("APPROVED")` approved on
    /// the word wherever it appeared, and `.claude/agents/reviewer.md`'s own
    /// format example prints it. A reviewer that copied the template, or left
    /// it unfilled, marked the feature `done` without anyone reviewing it.
    #[test]
    fn the_unfilled_template_line_is_not_an_approval() {
        let review = "# Review — feature 3\n\n**Verdict:** APPROVED | CHANGES_REQUESTED\n";
        assert!(
            review.to_uppercase().contains("APPROVED"),
            "the old rule fired"
        );
        assert_eq!(parse_verdict(review), Verdict::Missing);
    }

    /// The other half of the same bug: prose that argues *against* approving
    /// still contains the word.
    #[test]
    fn prose_about_approval_is_not_an_approval() {
        for review in [
            "This cannot be APPROVED: C4 is empty.\n",
            "Not approved — see crates/daemon/src/core.rs:812.\n",
            "The spec was approved; the implementation is not.\n",
        ] {
            assert_eq!(parse_verdict(review), Verdict::Missing, "{review}");
        }
    }

    #[test]
    fn a_verdict_line_settles_it() {
        assert_eq!(parse_verdict("findings\n\nAPPROVED\n"), Verdict::Approved);
        assert_eq!(
            parse_verdict("findings\n\nCHANGES_REQUESTED\n"),
            Verdict::ChangesRequested
        );
    }

    /// Every prompt says `CHANGES_REQUESTED`; the settlement used to understand
    /// only `REJECTED`, so a correctly written rejection blocked the feature
    /// instead of starting round two. Both are read now.
    #[test]
    fn the_older_vocabulary_still_reads_as_a_rejection() {
        assert_eq!(parse_verdict("REJECTED\n"), Verdict::ChangesRequested);
    }

    #[test]
    fn the_markdown_a_reviewer_actually_writes_is_tolerated() {
        for review in [
            "**Verdict:** APPROVED\n",
            "- Verdict: APPROVED\n",
            "## APPROVED\n",
            "verdict: approved\n",
            "`APPROVED`\n",
            "> **Verdict:** `APPROVED`\n",
        ] {
            assert_eq!(parse_verdict(review), Verdict::Approved, "{review}");
        }
    }

    /// A reviewer that wrote both answered neither, and that has to stop the
    /// cycle rather than pick the one that happens to come first.
    #[test]
    fn two_disagreeing_verdicts_are_ambiguous() {
        let review = "APPROVED\n\n...on reflection:\n\nCHANGES_REQUESTED\n";
        assert_eq!(parse_verdict(review), Verdict::Ambiguous);
    }

    #[test]
    fn restating_the_same_verdict_is_not_ambiguous() {
        assert_eq!(
            parse_verdict("**Verdict:** APPROVED\n\nsummary\n\nAPPROVED\n"),
            Verdict::Approved
        );
    }

    #[test]
    fn an_empty_review_has_no_verdict() {
        assert_eq!(parse_verdict(""), Verdict::Missing);
        assert_eq!(parse_verdict("   \n\n"), Verdict::Missing);
    }

    /// The schema and the prompts have to name the same two words, or the
    /// structured answer and the file disagree by construction.
    #[test]
    fn the_schema_speaks_the_reviewers_vocabulary() {
        let schema: serde_json::Value = serde_json::from_str(VERDICT_SCHEMA).expect("valid JSON");
        let values = schema["properties"]["verdict"]["enum"]
            .as_array()
            .expect("enum");
        assert_eq!(values[0], "APPROVED");
        assert_eq!(values[1], "CHANGES_REQUESTED");
        assert_eq!(schema["required"][0], "verdict");
        for value in values {
            let word = value.as_str().expect("string");
            assert!(
                verdict_on_line(word).is_some(),
                "the parser cannot read the schema's own {word} back"
            );
        }
    }

    /// H6: the schema answer was asked for on every review and thrown away.
    #[test]
    fn the_schema_answer_is_read_out_of_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("job.jsonl");
        std::fs::write(
            &log,
            "{\"type\":\"system\",\"session_id\":\"s\"}\n\
             {\"type\":\"result\",\"result\":\"{\\\"verdict\\\":\\\"CHANGES_REQUESTED\\\"}\"}\n",
        )
        .unwrap();
        assert_eq!(verdict_in_log(&log), Some(Verdict::ChangesRequested));
    }

    #[test]
    fn a_verdict_field_at_the_top_level_is_read_too() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("job.jsonl");
        std::fs::write(&log, "{\"verdict\":\"APPROVED\",\"summary\":\"fine\"}\n").unwrap();
        assert_eq!(verdict_in_log(&log), Some(Verdict::Approved));
    }

    /// The trap that makes keying on the *word* wrong: the prompt asking for a
    /// verdict is echoed in the very transcript the answer is read from, so a
    /// search for "APPROVED" would read the question as the answer.
    #[test]
    fn the_prompt_echoed_in_the_transcript_is_not_a_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("job.jsonl");
        std::fs::write(
            &log,
            "{\"type\":\"user\",\"text\":\"end that file with exactly APPROVED or CHANGES_REQUESTED\"}\n\
             plain text a provider without a stream format writes\n",
        )
        .unwrap();
        assert_eq!(
            verdict_in_log(&log),
            None,
            "no schema answer: fall back to the file"
        );
    }

    /// The last word is the answer: an agent that thought out loud and then
    /// answered is read as having answered.
    #[test]
    fn the_last_structured_answer_wins() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("job.jsonl");
        std::fs::write(
            &log,
            "{\"verdict\":\"CHANGES_REQUESTED\"}\n{\"verdict\":\"APPROVED\"}\n",
        )
        .unwrap();
        assert_eq!(verdict_in_log(&log), Some(Verdict::Approved));
    }

    #[test]
    fn a_step_maps_to_the_role_its_job_carries_and_back() {
        for step in [
            HarnessStep::Spec,
            HarnessStep::Implement,
            HarnessStep::Review,
        ] {
            let job = Job {
                id: domain::JobId::new(),
                provider_id: AgentProviderId::new("claude"),
                workspace_id: domain::WorkspaceId::new(),
                role: role(step),
                feature_id: Some(1),
                parent_session_id: None,
                state: JobState::Succeeded,
                summary: String::new(),
                prompt: String::new(),
                provider_session_id: None,
                exit_code: Some(0),
                last_line: None,
                last_output_at: None,
                started_at: domain::Timestamp::now(),
                finished_at: None,
                log_path: PathBuf::new(),
            };
            assert_eq!(step_of(&job), Some(step));
        }
    }

    /// The bug this guards: a step told to write `harness/…` writes it into the
    /// worktree it stands in, and the shared state never sees the spec.
    #[test]
    fn every_step_is_told_the_absolute_harness_root() {
        let feature = domain::HarnessFeature {
            id: 9,
            slug: "themes".into(),
            spec_raw: Some("add themes".into()),
            ..domain::HarnessFeature::default()
        };
        let root = Path::new("/checkout/forge");
        for step in [
            HarnessStep::Spec,
            HarnessStep::Implement,
            HarnessStep::Review,
        ] {
            let prompt = step_prompt(step, 9, &feature, root, 1);
            assert!(
                prompt.contains("/checkout/forge/harness"),
                "{step:?} must name the harness root: {prompt}"
            );
            assert!(
                !prompt.contains(" harness/specs"),
                "{step:?} must not spell a harness path relatively: {prompt}"
            );
        }
    }

    /// The exit of a job a human already blocked must not settle the feature.
    #[test]
    fn a_job_the_feature_is_no_longer_waiting_on_is_stale() {
        let job = Job {
            id: domain::JobId::new(),
            provider_id: AgentProviderId::new("claude"),
            workspace_id: domain::WorkspaceId::new(),
            role: SessionRole::Executor,
            feature_id: Some(1),
            parent_session_id: None,
            state: JobState::Failed,
            summary: String::new(),
            prompt: String::new(),
            provider_session_id: None,
            exit_code: Some(1),
            last_line: None,
            last_output_at: None,
            started_at: domain::Timestamp::now(),
            finished_at: None,
            log_path: PathBuf::new(),
        };
        let attempt = |job: Option<String>, settled: bool| domain::HarnessAttempt {
            step: HarnessStep::Implement,
            n: 1,
            job,
            provider: None,
            transport: None,
            started_at: "t".into(),
            settled_at: settled.then(|| "t".to_owned()),
            outcome: None,
            detail: None,
        };
        let mut feature = domain::HarnessFeature::default();

        // A row from before attempts existed has nothing to compare against.
        assert!(!job_is_stale(&feature, &job));

        feature.attempts = Some(vec![attempt(Some(job.id.to_string()), false)]);
        assert!(!job_is_stale(&feature, &job), "its own live attempt");

        feature.attempts = Some(vec![attempt(None, false)]);
        assert!(
            !job_is_stale(&feature, &job),
            "an attempt that names no job"
        );

        feature.attempts = Some(vec![attempt(Some(domain::JobId::new().to_string()), false)]);
        assert!(job_is_stale(&feature, &job), "another job's attempt");

        feature.attempts = Some(vec![attempt(Some(job.id.to_string()), true)]);
        assert!(
            job_is_stale(&feature, &job),
            "settled by a human: nothing is live"
        );
    }

    /// A job nobody ran for the harness must not move a feature.
    #[test]
    fn a_job_with_an_unrelated_role_is_not_a_step() {
        let job = Job {
            id: domain::JobId::new(),
            provider_id: AgentProviderId::new("claude"),
            workspace_id: domain::WorkspaceId::new(),
            role: SessionRole::Generic,
            feature_id: Some(1),
            parent_session_id: None,
            state: JobState::Succeeded,
            summary: String::new(),
            prompt: String::new(),
            provider_session_id: None,
            exit_code: Some(0),
            last_line: None,
            last_output_at: None,
            started_at: domain::Timestamp::now(),
            finished_at: None,
            log_path: PathBuf::new(),
        };
        assert_eq!(step_of(&job), None);
    }

    /// The prompt carries the ids the agent needs to find its own artefacts —
    /// a spec prompt that named no feature would have the agent guess.
    /// A retry is not a fresh start: the previous agent may have left the tree
    /// half-edited, and a prompt that did not say so invited the next one to
    /// begin from a state it did not expect.
    #[test]
    fn a_retry_tells_the_agent_the_tree_may_be_dirty() {
        let feature = domain::HarnessFeature {
            id: 4,
            slug: "thing".into(),
            spec_raw: Some("do it".into()),
            ..domain::HarnessFeature::default()
        };
        let root = Path::new("/repo");
        let first = step_prompt(HarnessStep::Implement, 4, &feature, root, 1);
        assert!(
            !first.contains("attempt"),
            "a first run says nothing: {first}"
        );
        let again = step_prompt(HarnessStep::Implement, 4, &feature, root, 2);
        assert!(again.contains("attempt 2"), "{again}");
        assert!(again.contains("git status"), "{again}");
    }

    #[test]
    fn each_prompt_names_the_feature_and_its_skill() {
        let feature = domain::HarnessFeature {
            id: 4,
            slug: "daemon-stats".into(),
            spec_raw: Some("wire the stats command".into()),
            ..domain::HarnessFeature::default()
        };
        let root = Path::new("/checkout/forge");
        let spec = step_prompt(HarnessStep::Spec, 4, &feature, root, 1);
        assert!(spec.contains("/feature skill"));
        assert!(spec.contains("harness/specs/4-daemon-stats/"));
        assert!(spec.contains("wire the stats command"));

        let implement = step_prompt(HarnessStep::Implement, 4, &feature, root, 1);
        assert!(implement.contains("/feature-go"));
        assert!(implement.contains("review_4.md"));

        let review = step_prompt(HarnessStep::Review, 4, &feature, root, 1);
        assert!(review.contains("review_4.md"));
        assert!(review.contains("APPROVED"));
        assert!(review.contains("Do not edit any code"));
    }
}
