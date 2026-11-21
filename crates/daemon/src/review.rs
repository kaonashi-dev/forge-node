//! Forge Review: one worker thread per run, `diff → pack → model → validate`,
//! with git, the pack, the transcript and SQLite outside the core lock. The
//! reviewer is an ordinary read-only agent session, read when its hook idles.

use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::collections::HashSet;

use domain::{
    ActivityEvidence, ActivityState, AgentProviderId, FindingReaction, Hypothesis, ProjectId,
    ReviewId, ReviewRun, ReviewStage, ReviewStatus, ReviewTarget, SessionId, SessionKind,
    SessionRole, Tier, Timestamp, WorkspaceId,
};
use protocol::{DaemonEvent, ErrorCode, ProtocolError, Response};

use codebase_graph::Graph;

use crate::core::{db_err, Daemon};

const MAX_REPO_FILE_BYTES: u64 = 64 * 1024;
const MAX_REPO_PROMPT_BYTES: u64 = 16 * 1024;
const REPO_PROMPT: &str = ".forge/review/prompt.md";
const PROJECT_DOCS: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const MAX_RULE_FILES: usize = 100;
const MAX_PRESET_FILES: usize = 50;
const MAX_FILE_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_RUNS: u32 = 20;
const KEEP_DAYS: i64 = 90;
const MAX_LISTED_REVIEWS: u32 = 100;
const REVIEWER_TITLE: &str = "Forge Review";
/// A run at or above this score teaches the knowledge base its patterns.
const GOOD_SCORE: f32 = 4.0;
/// How long an idle reviewer's unparseable answer is re-read before the run
/// fails: the hook can report idle a moment before the transcript is flushed.
const ANSWER_GRACE: Duration = Duration::from_secs(2);

pub(crate) struct RunningReview {
    pub(crate) id: ReviewId,
    /// Head, base, provider, model, effort, preset, tier and instructions: a
    /// start with the same key joins this run, any other start for the
    /// workspace replaces it.
    pub(crate) key: String,
}

pub(crate) enum End {
    Cancelled,
    Failed(String),
}

impl Daemon {
    #[allow(
        clippy::too_many_arguments,
        reason = "one per StartReview field, as the request carries them"
    )]
    pub(crate) fn start_review(
        self: &Arc<Self>,
        target: ReviewTarget,
        tier: Option<Tier>,
        provider: Option<String>,
        preset: Option<String>,
        instructions: Option<String>,
        options: domain::AgentLaunchOptions,
        swarm: domain::SwarmOptions,
    ) -> Result<Response, ProtocolError> {
        let workspace_id = target.workspace_id();
        let size = self.config.review.swarm_size(swarm.size);
        let hypotheses = if size == 1 && swarm.hypotheses.is_empty() {
            Vec::new()
        } else {
            review_swarm::select(usize::from(size), &swarm.hypotheses)
        };
        let path = self.workspace_path(workspace_id)?;
        let provider = provider.unwrap_or_else(|| self.config.review.provider.clone());
        let options = domain::AgentLaunchOptions {
            model: options.model.or_else(|| self.config.review.model.clone()),
            effort: options.effort.or_else(|| self.config.review.effort.clone()),
        };
        let descriptor = self
            .lock()
            .agents
            .descriptor(&AgentProviderId::new(provider.as_str()))
            .cloned()
            .ok_or_else(|| {
                ProtocolError::invalid_request(format!("unknown provider `{provider}`"))
            })?;
        if descriptor.structured_review.is_none() {
            return Err(ProtocolError::invalid_request(format!(
                "provider `{provider}` cannot run a structured review"
            )));
        }
        agents::launch_options::validate(&descriptor, &options)
            .map_err(|error| ProtocolError::invalid_request(error.to_string()))?;
        // A pull request's head is unknown until the worker fetches it, so its
        // slot and join key leave the head out.
        let slot = review_slot(&target);
        let (head, base_ref, branch, target_key) = match &target {
            ReviewTarget::Workspace { base, .. } => {
                let head = git_service::head_commit(&path).ok_or_else(|| {
                    ProtocolError::invalid_request("the workspace has no commit to review")
                })?;
                let base_ref = git_service::compare_base(&path, base.as_deref());
                if let (Some(base), None) = (base, &base_ref) {
                    return Err(ProtocolError::invalid_request(format!(
                        "base `{base}` does not name a commit"
                    )));
                }
                let branch = git_service::current_branch(&path).ok().flatten();
                let key = format!("{head}\0{base_ref:?}");
                (head, base_ref, branch, key)
            }
            ReviewTarget::PullRequest { base, .. } => {
                if let Some(base) = base {
                    git_service::validate_branch_name(base).map_err(|error| {
                        ProtocolError::invalid_request(format!("base `{base}`: {error}"))
                    })?;
                }
                (String::new(), None, None, format!("pr\0{base:?}"))
            }
            _ => return Err(ProtocolError::invalid_request("unsupported review target")),
        };
        let repo = load_review_dir(&path, &self.config.review.instructions);
        let resolved = review::resolve(
            &repo.config,
            &repo.rules,
            &repo.presets,
            preset.as_deref(),
            tier,
            instructions.as_deref(),
        )
        .map_err(ProtocolError::invalid_request)?;
        let mut notes = repo.problems;
        notes.extend(resolved.notes.iter().cloned());
        if options.model.is_some() || options.effort.is_some() {
            let mut note = format!("reviewer: {provider}");
            for (name, value) in [("model", &options.model), ("effort", &options.effort)] {
                if let Some(value) = value {
                    let _ = write!(note, ", {name} {value}");
                }
            }
            notes.push(note);
        }
        let context = read_context(&path, &resolved.context, &mut notes);
        let key = format!(
            "{target_key}\0{provider}\0{:?}\0{:?}\0{}\0{:?}\0{}\0{hypotheses:?}",
            options.model, options.effort, resolved.preset, resolved.tier, resolved.instructions
        );
        let max_workers = self.config.review.max_swarms.max(1);

        let id = ReviewId::new();
        {
            let mut inner = self.lock();
            match inner.reviews_running.get(&slot) {
                Some(running)
                    if running.key == key && !inner.review_cancel.contains(&running.id) =>
                {
                    return Ok(Response::ReviewStarted {
                        review_id: running.id,
                    });
                }
                // A superseded worker lives on until it sees its cancel, so it still counts.
                _ if inner.review_workers >= max_workers => {
                    return Err(ProtocolError::conflict(format!(
                        "{max_workers} reviews are already running"
                    )));
                }
                Some(running) => {
                    let superseded = running.id;
                    inner.review_cancel.insert(superseded);
                    crate::actions::run::wake_waiters(&mut inner);
                }
                None => {}
            }
            inner
                .reviews_running
                .insert(slot.clone(), RunningReview { id, key });
            inner.review_workers += 1;
        }

        let run = ReviewRun {
            id,
            workspace_id,
            target,
            head_oid: head,
            base_oid: None,
            tier: resolved.tier,
            preset: resolved.preset.clone(),
            provider: provider.clone(),
            status: ReviewStatus::Queued,
            stage: None,
            summary: String::new(),
            score: None,
            verdict: None,
            error: None,
            session_id: None,
            coverage: domain::Coverage {
                notes,
                ..domain::Coverage::default()
            },
            created_at: Timestamp::now(),
            finished_at: None,
            branch,
            tree_oid: None,
            diagrams: Vec::new(),
            auto_approvable: false,
            swarm_size: u8::try_from(hypotheses.len().max(1)).unwrap_or(1),
            schema_version: if hypotheses.is_empty() {
                domain::SchemaVersion::V1
            } else {
                domain::SchemaVersion::V2
            },
            sandbox_run_id: None,
        };
        if let Err(error) = self.review_store.insert_run(&run) {
            self.release_review(&slot, id);
            return Err(db_err(error));
        }
        self.registry
            .broadcast_domain(DaemonEvent::ReviewUpdated { run: run.clone() });

        let daemon = Arc::clone(self);
        let mut queued = run.clone();
        let launch = Launch {
            provider,
            options,
            swarm: hypotheses,
        };
        std::thread::Builder::new()
            .name("forge-review".into())
            .spawn(move || daemon.run_review(run, path, launch, base_ref, &resolved, &context))
            .map_err(|error| {
                self.release_review(&slot, id);
                queued.status = ReviewStatus::Failed;
                queued.error = Some(format!("cannot start the review: {error}"));
                self.finish_review(&mut queued);
                ProtocolError::new(ErrorCode::IoError, format!("cannot start review: {error}"))
            })?;
        Ok(Response::ReviewStarted { review_id: id })
    }

    pub(crate) fn cancel_review(&self, review_id: ReviewId) -> Result<Response, ProtocolError> {
        let running = {
            let mut inner = self.lock();
            let running = inner.reviews_running.values().any(|r| r.id == review_id);
            if running {
                inner.review_cancel.insert(review_id);
                crate::actions::run::wake_waiters(&mut inner);
            }
            running
        };
        if !running && self.review_store.get(review_id).map_err(db_err)?.is_none() {
            return Err(ProtocolError::not_found("review"));
        }
        Ok(Response::Ack)
    }

    pub(crate) fn get_review(&self, review_id: ReviewId) -> Result<Response, ProtocolError> {
        let (run, findings) = self
            .review_store
            .get(review_id)
            .map_err(db_err)?
            .ok_or_else(|| ProtocolError::not_found("review"))?;
        let (previous, resolved) = match self.previous_review(&run) {
            Some((previous, before)) => (
                Some(previous.id),
                review::resolved(&before, &findings, &run.coverage.withheld),
            ),
            None => (None, Vec::new()),
        };
        Ok(Response::Review {
            run: Box::new(run),
            findings,
            resolved,
            previous,
        })
    }

    /// The newest completed run of `run`'s lineage created before it, with its
    /// findings; `None` for a run that has not completed.
    fn previous_review(&self, run: &ReviewRun) -> Option<(ReviewRun, Vec<domain::Finding>)> {
        if run.status != ReviewStatus::Completed {
            return None;
        }
        let runs = self
            .review_store
            .list(run.workspace_id, MAX_LISTED_REVIEWS)
            .inspect_err(|error| tracing::warn!(%error, "could not list reviews for lineage"))
            .ok()?;
        let previous = runs.into_iter().find(|other| {
            other.id != run.id
                && other.status == ReviewStatus::Completed
                && other.created_at < run.created_at
                && review::same_lineage(run, other)
        })?;
        self.review_store
            .get(previous.id)
            .inspect_err(|error| tracing::warn!(%error, "could not read the previous review"))
            .ok()
            .flatten()
    }

    pub(crate) fn list_reviews(
        &self,
        workspace_id: WorkspaceId,
        limit: u32,
    ) -> Result<Response, ProtocolError> {
        let runs = self
            .review_store
            .list(workspace_id, limit.min(MAX_LISTED_REVIEWS))
            .map_err(db_err)?;
        Ok(Response::Reviews(runs))
    }

    pub(crate) fn get_review_config(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let project = self.workspace_project(workspace_id)?;
        let feedback = self.review_store.feedback(project).map_err(db_err)?;
        let repo = load_review_dir(&path, &self.config.review.instructions);
        let (default_preset, note) = review::default_preset(&repo.config, &repo.presets);
        let mut errors = repo.problems;
        errors.extend(note);
        Ok(Response::ReviewConfig(domain::ReviewSettings {
            default_preset,
            presets: repo
                .presets
                .into_iter()
                .map(|preset| domain::ReviewPreset {
                    id: preset.id,
                    name: preset.name,
                    detail: preset.detail,
                    tier: preset.tier,
                })
                .collect(),
            rules: repo
                .rules
                .into_iter()
                .map(|rule| domain::ReviewRule {
                    id: rule.id,
                    title: rule.title,
                    severity: rule.severity,
                    category: rule.category,
                    scope: rule.scope,
                    enabled: rule.enabled,
                })
                .collect(),
            errors,
            suppressions: review::suppressions(&feedback, unix_now()),
        }))
    }

    pub(crate) fn get_review_diff(&self, review_id: ReviewId) -> Result<Response, ProtocolError> {
        let (run, _) = self
            .review_store
            .get(review_id)
            .map_err(db_err)?
            .ok_or_else(|| ProtocolError::not_found("review"))?;
        let no_diff = || ProtocolError::invalid_request("the review has no diff yet");
        if run.head_oid.is_empty() {
            return Err(no_diff());
        }
        let path = self.workspace_path(run.workspace_id)?;
        let context = git_service::DIFF_CONTEXT_LINES;
        let (base, head) = match &run.target {
            ReviewTarget::PullRequest { .. } => (
                run.base_oid.as_deref().ok_or_else(no_diff)?,
                run.head_oid.as_str(),
            ),
            _ => (
                run.base_oid.as_deref().unwrap_or(&run.head_oid),
                run.tree_oid.as_deref().ok_or_else(|| {
                    ProtocolError::invalid_request("the reviewed working tree was not kept")
                })?,
            ),
        };
        if !git_service::object_exists(&path, head) {
            return Err(ProtocolError::invalid_request(
                "the reviewed tree is no longer in the repository",
            ));
        }
        let diff = git_service::commit_range_diff(&path, base, head, context)
            .map_err(crate::core::git_err)?;
        Ok(Response::WorkspaceDiff(domain::WorkspaceDiff {
            workspace_id: run.workspace_id,
            branch: diff.branch,
            files: diff.files.into_iter().map(crate::core::diff_file).collect(),
            truncated: diff.truncated,
        }))
    }

    /// One kind of diagram for the range a run reviewed, drawn again from the
    /// current graph and never stored.
    pub(crate) fn get_review_diagram(
        &self,
        review_id: ReviewId,
        kind: domain::DiagramKind,
    ) -> Result<Response, ProtocolError> {
        let (run, _) = self
            .review_store
            .get(review_id)
            .map_err(db_err)?
            .ok_or_else(|| ProtocolError::not_found("review"))?;
        let Response::WorkspaceDiff(diff) = self.get_review_diff(review_id)? else {
            return Err(ProtocolError::invalid_request("the review has no diff"));
        };
        let path = self.workspace_path(run.workspace_id)?;
        let project = self.workspace_project(run.workspace_id)?;
        let graph = self
            .review_store
            .load_graph(project)
            .map_err(db_err)?
            .unwrap_or_default();
        let changed: Vec<diagram_gen::ChangedFile> = diff
            .files
            .into_iter()
            .filter(|file| !file.binary && !file.truncated)
            .map(|file| diagram_gen::ChangedFile {
                text: reviewed_text(&path, &run, &file.path),
                path: file.path,
                patch: file.patch,
            })
            .collect();
        Ok(Response::ReviewDiagram(diagram_gen::generate_kind(
            &changed, &graph, kind,
        )))
    }

    pub(crate) fn set_finding_feedback(
        &self,
        review_id: ReviewId,
        finding_id: u32,
        reaction: Option<FindingReaction>,
    ) -> Result<Response, ProtocolError> {
        let (mut run, mut findings) = self
            .review_store
            .get(review_id)
            .map_err(db_err)?
            .ok_or_else(|| ProtocolError::not_found("review"))?;
        let project = self.workspace_project(run.workspace_id)?;
        let finding = findings
            .iter_mut()
            .find(|finding| finding.id == finding_id)
            .ok_or_else(|| ProtocolError::not_found("finding"))?;
        let was_dismissed = finding.reaction == Some(FindingReaction::Dismiss);
        match reaction {
            Some(reaction) => self
                .review_store
                .set_feedback(project, review_id, finding, reaction),
            None => self.review_store.clear_feedback(review_id, finding_id),
        }
        .map_err(db_err)?;
        finding.reaction = reaction;
        if was_dismissed != (reaction == Some(FindingReaction::Dismiss)) && run.score.is_some() {
            let settings = self.auto_approve_settings(run.workspace_id);
            rescore(&mut run, &findings, &settings);
            self.publish_review(&run);
        }
        Ok(Response::Ack)
    }

    pub(crate) fn clear_review_suppression(
        &self,
        workspace_id: WorkspaceId,
        suppression: &domain::ReviewSuppression,
    ) -> Result<Response, ProtocolError> {
        let project = self.workspace_project(workspace_id)?;
        let restored = self
            .review_store
            .clear_suppression(project, suppression)
            .map_err(db_err)?;
        for review_id in restored {
            match self.review_store.get(review_id) {
                Ok(Some((mut run, findings))) if run.score.is_some() => {
                    rescore(
                        &mut run,
                        &findings,
                        &self.auto_approve_settings(workspace_id),
                    );
                    self.publish_review(&run);
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%review_id, %error, "could not rescore a review"),
            }
        }
        Ok(Response::Ack)
    }

    pub(crate) fn review_stats(
        &self,
        workspace_id: Option<WorkspaceId>,
    ) -> Result<Response, ProtocolError> {
        let project = workspace_id
            .map(|ws| self.workspace_project(ws))
            .transpose()?;
        let rows = self.review_store.stats_rows(workspace_id).map_err(db_err)?;
        let ids: HashSet<String> = rows.iter().map(|row| row.id.clone()).collect();
        let sandboxes: Vec<(String, bool)> = self
            .review_store
            .linked_sandbox_runs()
            .map_err(db_err)?
            .into_iter()
            .filter(|(review, _)| ids.contains(review))
            .collect();
        let answered = self
            .graphs
            .answered
            .load(std::sync::atomic::Ordering::Relaxed);
        let rebuilds = self
            .graphs
            .rebuilds
            .load(std::sync::atomic::Ordering::Relaxed);
        let swarm: Vec<&persistence::repositories::reviews::StatsRow> =
            rows.iter().filter(|row| row.swarm_size > 1).collect();
        Ok(Response::ReviewStats(domain::ReviewStats {
            runs: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            latency: [Tier::Base, Tier::Plus, Tier::Apex]
                .into_iter()
                .filter_map(|tier| {
                    let mut seconds: Vec<i64> = rows
                        .iter()
                        .filter(|row| row.tier == tier)
                        .map(|row| row.seconds.max(0))
                        .collect();
                    seconds.sort_unstable();
                    (!seconds.is_empty()).then(|| domain::TierLatency {
                        tier,
                        runs: u32::try_from(seconds.len()).unwrap_or(u32::MAX),
                        p50_secs: percentile(&seconds, 50),
                        p95_secs: percentile(&seconds, 95),
                    })
                })
                .collect(),
            graph_hit_rate: rate(answered, answered + rebuilds),
            swarm_coverage: rate(
                swarm
                    .iter()
                    .map(|row| u64::from(row.hypotheses_found))
                    .sum(),
                swarm.iter().map(|row| u64::from(row.swarm_size)).sum(),
            ),
            kb_hit_rate: rate(
                rows.iter().filter(|row| row.cites_kb).count() as u64,
                rows.len() as u64,
            ),
            trex_failure_rate: rate(
                sandboxes.iter().filter(|(_, failed)| *failed).count() as u64,
                sandboxes.len() as u64,
            ),
            kb_bytes: self.review_store.kb_bytes(project).map_err(db_err)?,
        }))
    }

    fn auto_approve_settings(&self, workspace_id: WorkspaceId) -> review::AutoApprove {
        self.workspace_path(workspace_id)
            .map(|path| repo_config(&path).auto_approve)
            .unwrap_or_default()
    }

    pub(crate) fn workspace_project(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<ProjectId, ProtocolError> {
        self.lock()
            .workspaces
            .get(&workspace_id)
            .map(|ws| ws.project_id)
            .ok_or_else(|| ProtocolError::not_found("workspace"))
    }

    fn run_review(
        self: Arc<Self>,
        mut run: ReviewRun,
        path: PathBuf,
        launch: Launch,
        base_ref: Option<String>,
        resolved: &review::Resolved,
        context: &[(String, String)],
    ) {
        let guard = ReviewGuard {
            daemon: Arc::clone(&self),
            slot: review_slot(&run.target),
            review_id: run.id,
        };
        let pack_dir = PackDir(
            self.forge_paths()
                .home
                .join(PACKS_DIR)
                .join(run.id.to_string()),
        );
        run.status = ReviewStatus::Running;
        let stages = Stages {
            path: &path,
            launch: &launch,
            base_ref,
            resolved,
            context,
            pack_dir: &pack_dir.0,
        };
        // A panic is a failed run, not one left `Running` until the next restart.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.review_stages(&mut run, stages)
        }))
        .unwrap_or_else(|_| Err(End::Failed("the review worker crashed".into())));
        let (outcome, patterns) = match outcome {
            Ok((findings, patterns)) => (Ok(findings), patterns),
            Err(end) => (Err(end), Vec::new()),
        };
        // Before the pack goes: the reviewer was given it with `--add-dir`.
        if let Some(session_id) = run.session_id {
            self.stop_reviewer(session_id);
        }
        self.settle_review(&mut run, outcome);
        if run.status == ReviewStatus::Completed
            && run.score.is_some_and(|score| score >= GOOD_SCORE)
        {
            if let Ok(project) = self.workspace_project(run.workspace_id) {
                self.remember_patterns(project, &patterns);
            }
        }
        if let ReviewTarget::PullRequest { number, .. } = &run.target {
            // `GetReviewDiff` reads the range by oid; once unreachable, a `git gc` may prune it.
            if let Err(error) =
                git_service::delete_pull_request_refs(&path, *number, &run.id.to_string())
            {
                tracing::warn!(%error, "could not delete a review's pull-request refs");
            }
        }
        drop(pack_dir);
        drop(guard);
        self.finish_review(&mut run);
        crate::actions::run::wake_waiters(&mut self.lock());
        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(KEEP_DAYS);
        if let Err(error) =
            self.review_store
                .prune(run.workspace_id, KEEP_RUNS, Timestamp::from_offset(cutoff))
        {
            tracing::warn!(%error, "could not prune old reviews");
        }
        self.prune_kb();
    }

    /// A cancel that arrived while the answer was validated still wins, and
    /// no findings are saved for it.
    pub(crate) fn settle_review(
        &self,
        run: &mut ReviewRun,
        outcome: Result<Vec<domain::Finding>, End>,
    ) {
        let cancelled = self.lock().review_cancel.contains(&run.id);
        match outcome {
            Ok(_) | Err(End::Cancelled) if cancelled => {
                run.status = ReviewStatus::Cancelled;
                run.score = None;
                run.verdict = None;
            }
            Ok(mut findings) => {
                // `previous_review` only looks back from a completed run.
                run.status = ReviewStatus::Completed;
                if let Some((_, before)) = self.previous_review(run) {
                    review::mark_lineage(&mut findings, &before);
                }
                if let Err(error) = self.review_store.insert_findings(run.id, &findings) {
                    run.status = ReviewStatus::Failed;
                    run.error = Some(format!("could not save the findings: {error}"));
                }
            }
            Err(End::Cancelled) => run.status = ReviewStatus::Cancelled,
            Err(End::Failed(error)) => {
                run.status = ReviewStatus::Failed;
                run.error = Some(error);
            }
        }
    }

    fn review_stages(
        self: &Arc<Self>,
        run: &mut ReviewRun,
        Stages {
            path,
            launch,
            base_ref,
            resolved,
            context,
            pack_dir,
        }: Stages<'_>,
    ) -> Result<(Vec<domain::Finding>, Vec<String>), End> {
        let budget_ends = Instant::now() + answer_budget(self.config.review.timeout_secs);
        self.review_stage(run, ReviewStage::Diff)?;
        let pull_request = match &run.target {
            ReviewTarget::PullRequest { number, base, .. } => Some((*number, base.clone())),
            _ => None,
        };
        let (merge, base_ref, diff, commits) = match pull_request.clone() {
            Some((number, base)) => self.pull_request_diff(run, path, number, base)?,
            None => {
                let merge = base_ref.as_ref().and_then(|base| {
                    git_service::merge_base(path, &[base.clone(), run.head_oid.clone()])
                });
                if merge.is_none() {
                    run.coverage
                        .notes
                        .push("no base branch found; reviewed the uncommitted changes only".into());
                }
                let diff = git_service::working_tree_diff(
                    path,
                    merge.as_deref(),
                    git_service::DIFF_CONTEXT_LINES,
                )
                .map_err(|error| End::Failed(format!("git diff failed: {error}")))?;
                let commits = merge
                    .as_deref()
                    .map(|merge| git_service::commits_since(path, merge).0)
                    .unwrap_or_default();
                run.tree_oid = snapshot(path, &diff, pack_dir)
                    .inspect_err(|error| {
                        tracing::warn!(%error, "could not snapshot the reviewed working tree");
                    })
                    .ok();
                if run.tree_oid.is_none() {
                    run.coverage
                        .notes
                        .push("the reviewed working tree was not kept for a later diff".into());
                }
                (merge, base_ref, diff, commits)
            }
        };
        run.base_oid.clone_from(&merge);
        if diff.files.is_empty() {
            return Err(End::Failed(
                "nothing to review: there are no changes".into(),
            ));
        }
        let commits: Vec<domain::CommitLine> =
            commits.into_iter().map(crate::core::commit_line).collect();
        let files: Vec<domain::DiffFile> =
            diff.files.into_iter().map(crate::core::diff_file).collect();

        let mut resolved = resolved.clone();
        if resolved.auto_tier {
            resolved.tier = review::auto_tier(&files);
            run.coverage
                .notes
                .push(format!("tier auto: {}", tier_name(resolved.tier)));
        }
        run.tier = resolved.tier;
        let resolved = &resolved;
        let graph = if resolved.graph.enabled {
            match self
                .workspace_project(run.workspace_id)
                .map_err(|e| e.to_string())
                .and_then(|project| self.refresh_graph(project, path, &resolved.ignore, false))
            {
                Ok((graph, _)) => Some(graph),
                Err(error) => {
                    run.coverage
                        .notes
                        .push(format!("codebase graph unavailable: {error}"));
                    None
                }
            }
        } else {
            None
        };

        self.review_stage(run, ReviewStage::Pack)?;
        let meta = review::PackMeta {
            head_oid: run.head_oid.clone(),
            base_oid: merge,
            base_ref: base_ref.unwrap_or_else(|| "HEAD".into()),
            pull_request: pull_request.as_ref().map(|(number, _)| *number),
        };
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        let rules = review::in_scope(&resolved.rules, &paths);
        // A v7 uuid starts with its timestamp; its tail is the random part.
        let uuid = SessionId::new().as_uuid().simple().to_string();
        let nonce = &uuid[uuid.len() - 12..];
        let (mut pack, mut coverage) =
            review::build_pack(&meta, &commits, &files, resolved, &rules, context, nonce);
        let kb_budget = review::TierCaps::of(resolved.tier).kb_bytes;
        let kb = if kb_budget > 0 && resolved.kb_enabled && self.config.review.kb_enabled {
            self.workspace_project(run.workspace_id)
                .map(|project| self.kb_excerpts(project, &coverage.included, kb_budget))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let changed_functions = graph
            .as_deref()
            .map(|graph| crate::graph::changed_functions(graph, &files, &coverage.included))
            .unwrap_or_default();
        let cross_repo = match graph.as_deref() {
            Some(graph)
                if review::TierCaps::of(resolved.tier).cross_repo_bytes > 0
                    && resolved.graph.cross_repo
                    && self.config.review.cross_repo_enabled =>
            {
                self.cross_repo_signatures(
                    path,
                    &resolved.graph.repos,
                    graph,
                    &changed_functions,
                    &mut coverage.notes,
                )
            }
            _ => Vec::new(),
        };
        let runtime = if resolved.tier == Tier::Apex && self.config.review.trex_enabled {
            self.review_sandbox(
                run,
                path,
                pull_request.is_some(),
                budget_ends,
                &mut coverage.notes,
            )
        } else {
            Vec::new()
        };
        let extras = review::Extras {
            graph: changed_functions,
            kb,
            cross_repo,
            runtime,
        };
        let (blocks, prompt_blocks) = review::blocks(&extras, resolved.tier, nonce, &mut coverage);
        pack.push_str(&blocks);
        coverage
            .notes
            .splice(0..0, std::mem::take(&mut run.coverage.notes));
        run.coverage = coverage;
        let pack_file = pack_dir.join("pack.md");
        create_pack_dir(pack_dir)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&pack_file)
            })
            .and_then(|mut file| file.write_all(pack.as_bytes()))
            .map_err(|error| End::Failed(format!("could not write the review pack: {error}")))?;

        self.review_stage(run, ReviewStage::Model)?;
        let mut reviewers = Reviewers {
            daemon: self,
            sessions: Vec::new(),
        };
        let mut outputs: Vec<(Option<Hypothesis>, Result<review::ReviewOutput, String>)> =
            Vec::new();
        if launch.swarm.is_empty() {
            let transcript = SessionId::new().as_uuid().to_string();
            let prompt = review::prompt(resolved, &pack_file, nonce) + &prompt_blocks;
            let session_id = self.launch_reviewer(
                run.workspace_id,
                launch,
                REVIEWER_TITLE.into(),
                prompt,
                &transcript,
                pack_dir,
            )?;
            reviewers.sessions.push(session_id);
            run.session_id = Some(session_id);
            self.publish_review(run);
            // Read after the launch: the launch is what resolves the login-shell environment.
            let answer = Answer {
                transcript: &transcript,
                provider: &launch.provider,
                config_dir: self.launch_config_dir(&launch.provider),
            };
            outputs.push((
                None,
                Ok(self.await_answer(run.id, session_id, path, &answer)?),
            ));
        } else {
            let included: Vec<domain::DiffFile> = files
                .iter()
                .filter(|file| run.coverage.included.contains(&file.path))
                .cloned()
                .collect();
            let plan = review_swarm::plan(&launch.swarm, &included);
            let mut agents = Vec::new();
            for spec in &plan.agents {
                let name = spec.hypothesis.name();
                let slice = pack_dir.join(format!("slice-{name}.md"));
                write_private(&slice, &review_swarm::slice_file(spec, nonce)).map_err(|error| {
                    End::Failed(format!("could not write the slice for {name}: {error}"))
                })?;
                let transcript = SessionId::new().as_uuid().to_string();
                let prompt = review_swarm::prompt(resolved, &pack_file, &slice, spec, nonce)
                    + &prompt_blocks;
                match self.launch_reviewer(
                    run.workspace_id,
                    launch,
                    format!("{REVIEWER_TITLE} · {name}"),
                    prompt,
                    &transcript,
                    pack_dir,
                ) {
                    Ok(session) => {
                        reviewers.sessions.push(session);
                        agents.push((spec.hypothesis, session, transcript));
                    }
                    Err(End::Failed(error)) => outputs.push((Some(spec.hypothesis), Err(error))),
                    Err(End::Cancelled) => return Err(End::Cancelled),
                }
            }
            run.session_id = agents.first().map(|(_, session, _)| *session);
            self.publish_review(run);
            if let Err(error) = self
                .review_store
                .set_agent_sessions(run.id, &reviewers.sessions)
            {
                tracing::warn!(%error, "could not record the swarm's sessions");
            }
            let config_dir = self.launch_config_dir(&launch.provider);
            let review_id = run.id;
            let waited: Vec<Result<review::ReviewOutput, End>> = std::thread::scope(|scope| {
                let waits: Vec<_> = agents
                    .iter()
                    .map(|(_, session, transcript)| {
                        let answer = Answer {
                            transcript,
                            provider: &launch.provider,
                            config_dir: config_dir.clone(),
                        };
                        scope.spawn(move || self.await_answer(review_id, *session, path, &answer))
                    })
                    .collect();
                waits
                    .into_iter()
                    .map(|wait| {
                        wait.join()
                            .unwrap_or_else(|_| Err(End::Failed("the agent's wait crashed".into())))
                    })
                    .collect()
            });
            for ((hypothesis, _, _), result) in agents.iter().zip(waited) {
                match result {
                    Ok(output) => outputs.push((Some(*hypothesis), Ok(output))),
                    Err(End::Cancelled) => return Err(End::Cancelled),
                    Err(End::Failed(error)) => outputs.push((Some(*hypothesis), Err(error))),
                }
            }
        }
        drop(reviewers);

        self.review_stage(run, ReviewStage::Validate)?;
        let suppressions = match self
            .workspace_project(run.workspace_id)
            .map_err(|error| error.to_string())
            .and_then(|project| {
                self.review_store
                    .feedback(project)
                    .map_err(|e| e.to_string())
            }) {
            Ok(feedback) => review::suppressions(&feedback, unix_now()),
            Err(error) => {
                tracing::warn!(%error, "could not read review feedback; nothing suppressed");
                Vec::new()
            }
        };
        let mentioned: HashSet<String> = outputs
            .iter()
            .filter_map(|(_, output)| output.as_ref().ok())
            .flat_map(|output| output.findings.iter().map(|finding| finding.path.clone()))
            .collect();
        let patterns: Vec<String> = outputs
            .iter()
            .filter_map(|(_, output)| output.as_ref().ok())
            .flat_map(|output| output.patterns.iter().cloned())
            .collect();
        let changed: Vec<review::ChangedFile> = files
            .iter()
            .filter(|file| !file.binary)
            .map(|file| {
                let excluded = run.coverage.excluded.contains(&file.path);
                review::ChangedFile {
                    path: file.path.clone(),
                    patch: file.patch.clone(),
                    excluded,
                    text: (!excluded && mentioned.contains(&file.path))
                        .then(|| reviewed_text(path, run, &file.path))
                        .flatten(),
                }
            })
            .collect();
        let mut tally = Tally::default();
        let (findings, summary) = if launch.swarm.is_empty() {
            let Some((_, Ok(output))) = outputs.pop() else {
                return Err(End::Failed("the reviewer gave no answer".into()));
            };
            let validated = review::validate(output, &changed, &rules, resolved, &suppressions)
                .map_err(End::Failed)?;
            tally.add(&validated);
            tally.trimmed = validated.trimmed;
            run.coverage.withheld = validated.withheld;
            (validated.findings, validated.summary)
        } else {
            let answers: Vec<(Hypothesis, review_swarm::AgentAnswer)> = outputs
                .into_iter()
                .filter_map(|(hypothesis, output)| {
                    let answer = match output.map(|output| {
                        review::validate(output, &changed, &rules, resolved, &suppressions)
                    }) {
                        Ok(Ok(validated)) => {
                            tally.add(&validated);
                            review_swarm::AgentAnswer::Validated(validated)
                        }
                        Ok(Err(rejected)) => {
                            review_swarm::AgentAnswer::Uncovered(format!("rejected: {rejected}"))
                        }
                        Err(error) => review_swarm::AgentAnswer::Uncovered(error),
                    };
                    Some((hypothesis?, answer))
                })
                .collect();
            let reasons: Vec<String> = answers
                .iter()
                .filter_map(|(hypothesis, answer)| match answer {
                    review_swarm::AgentAnswer::Uncovered(reason) => {
                        Some(format!("{}: {reason}", hypothesis.name()))
                    }
                    review_swarm::AgentAnswer::Validated(_) => None,
                })
                .collect();
            let consolidated = review_swarm::consolidate(answers, resolved.max_findings)
                .ok_or_else(|| {
                    End::Failed(format!("every swarm agent failed: {}", reasons.join("; ")))
                })?;
            run.coverage.notes.extend(consolidated.notes);
            tally.unverified = consolidated.unverified;
            run.coverage.withheld = consolidated.withheld;
            (consolidated.findings, consolidated.summary)
        };
        tally.notes(run, resolved);
        let score = review::score(&findings);
        run.verdict = Some(review::verdict(&findings, score));
        run.score = Some(score);
        run.auto_approvable =
            review::auto_approvable(&resolved.auto_approve, &findings, score, &run.coverage);
        run.summary = summary;
        let drawn: Vec<diagram_gen::ChangedFile> = files
            .iter()
            .filter(|file| run.coverage.included.contains(&file.path))
            .map(|file| diagram_gen::ChangedFile {
                path: file.path.clone(),
                patch: file.patch.clone(),
                text: reviewed_text(path, run, &file.path),
            })
            .collect();
        run.diagrams = diagram_gen::generate(&drawn, graph.as_deref().unwrap_or(&Graph::default()));
        Ok((findings, patterns))
    }

    /// The sandbox targets for an `apex` run: in the checkout, or for a pull
    /// request in a detached worktree of its head that goes with the run.
    fn review_sandbox(
        &self,
        run: &mut ReviewRun,
        path: &Path,
        pull_request: bool,
        deadline: Instant,
        notes: &mut Vec<String>,
    ) -> Vec<review::RuntimeResult> {
        if !path.join(runtime_sandbox::CONFIG_PATH).exists() {
            notes.push(format!(
                "TREX: no {}; nothing ran",
                runtime_sandbox::CONFIG_PATH
            ));
            return Vec::new();
        }
        let worktree = if pull_request {
            match crate::sandbox::HeadWorktree::add(
                &self.forge_paths().home,
                path,
                &run.id.to_string(),
                &run.head_oid,
            ) {
                Ok(worktree) => Some(worktree),
                Err(error) => {
                    notes.push(format!("TREX: could not check out the head: {error}"));
                    return Vec::new();
                }
            }
        } else {
            None
        };
        let run_dir = worktree
            .as_ref()
            .map_or(path, |worktree| worktree.path.as_path());
        let id = SessionId::new().as_uuid().to_string();
        let review_id = run.id;
        let cancelled = || self.lock().review_cancel.contains(&review_id);
        let sandbox = self.run_sandbox_targets(
            id.clone(),
            run.workspace_id,
            Some(crate::sandbox::ForReview {
                id: review_id,
                deadline,
                cancelled: &cancelled,
            }),
            path,
            run_dir,
            None,
        );
        drop(worktree);
        run.sandbox_run_id = Some(id);
        if let Some(error) = &sandbox.error {
            notes.push(format!("TREX: {error}"));
        }
        sandbox
            .targets
            .into_iter()
            .map(|target| review::RuntimeResult {
                target: target.name,
                exit_code: target.exit_code,
                stdout: target.stdout,
                stderr: target.stderr,
            })
            .collect()
    }

    fn launch_reviewer(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        launch: &Launch,
        title: String,
        prompt: String,
        transcript: &str,
        pack_dir: &Path,
    ) -> Result<SessionId, End> {
        match self.create_session_with_options(
            workspace_id,
            SessionKind::Agent,
            Some(AgentProviderId::new(launch.provider.as_str())),
            None,
            None,
            SessionRole::Generic,
            None,
            Some(prompt),
            true,
            launch.options.clone(),
            Some(title),
            Some(domain::StructuredReview {
                session_id: transcript.to_owned(),
                add_dir: pack_dir.to_path_buf(),
            }),
            false,
        ) {
            Ok(Response::SessionCreated { session_id, .. }) => Ok(session_id),
            Ok(_) => Err(End::Failed("the reviewer did not start".into())),
            Err(error) => Err(End::Failed(format!("the reviewer did not start: {error}"))),
        }
    }

    /// Fetch the pull request's head into a private ref and diff it against its
    /// merge base with the base branch: `(merge, base ref, diff, commits)`.
    fn pull_request_diff(
        &self,
        run: &mut ReviewRun,
        path: &Path,
        number: u32,
        base: Option<String>,
    ) -> Result<DiffStage, End> {
        let (remote, known_base) = self.pull_request_remote(path, run.workspace_id, number)?;
        let base = base.or(known_base);
        let fetched = git_service::fetch_pull_request(
            path,
            &remote,
            number,
            &run.id.to_string(),
            base.as_deref(),
            self.config.git.fetch_timeout(),
        )
        .map_err(|error| End::Failed(format!("could not fetch pull request #{number}: {error}")))?;
        run.head_oid = fetched.head;
        let base_ref = format!("{remote}/{}", base.as_deref().unwrap_or("HEAD"));
        let merge = git_service::merge_base(path, &[fetched.base, run.head_oid.clone()])
            .ok_or_else(|| {
                End::Failed(format!(
                    "pull request #{number} shares no history with {base_ref}"
                ))
            })?;
        let diff = git_service::commit_range_diff(
            path,
            &merge,
            &run.head_oid,
            git_service::DIFF_CONTEXT_LINES,
        )
        .map_err(|error| End::Failed(format!("git diff failed: {error}")))?;
        let commits = git_service::commits_between(path, &merge, &run.head_oid).0;
        Ok((Some(merge), Some(base_ref), diff, commits))
    }

    /// The remote whose repository the cached pull request `number` belongs to,
    /// with its base branch, or else the remote the pull-request listing reads
    /// (`origin`, or the only one there is) with no base known.
    fn pull_request_remote(
        &self,
        path: &Path,
        workspace: WorkspaceId,
        number: u32,
    ) -> Result<(String, Option<String>), End> {
        let remotes = git_service::list_remotes(path)
            .map_err(|error| End::Failed(format!("could not list the remotes: {error}")))?;
        let project = self.workspace_project(workspace).ok();
        let mut known: Vec<domain::PullRequest> = self
            .pull_request_cache()
            .snapshot()
            .pull_requests
            .into_iter()
            .filter(|pr| {
                pr.number == number && (pr.project_id.is_none() || pr.project_id == project)
            })
            .collect();
        known.sort_by_key(|pr| pr.project_id.is_none());
        let hosting = known.iter().find_map(|pr| {
            remotes.iter().find_map(|(name, url)| {
                let repo = git_service::parse_remote_url(url)?;
                (repo.host.eq_ignore_ascii_case(&pr.host)
                    && repo.name_with_owner().eq_ignore_ascii_case(&pr.repository))
                .then(|| {
                    (
                        name.clone(),
                        (!pr.base_ref.is_empty()).then(|| pr.base_ref.clone()),
                    )
                })
            })
        });
        if let Some(hosting) = hosting {
            return Ok(hosting);
        }
        remotes
            .iter()
            .find(|(name, _)| name == "origin")
            .or_else(|| remotes.first())
            .map(|(name, _)| (name.clone(), None))
            .ok_or_else(|| {
                End::Failed(format!(
                    "pull request #{number}: the checkout has no remote"
                ))
            })
    }

    /// Block until the reviewer's transcript holds a parseable answer, it
    /// stops or exits without one, the run is cancelled or the budget runs out.
    /// The caller stops the reviewer whatever the outcome.
    fn await_answer(
        &self,
        review_id: ReviewId,
        session_id: SessionId,
        cwd: &Path,
        answer: &Answer<'_>,
    ) -> Result<review::ReviewOutput, End> {
        let (wake_tx, wake_rx) = flume::bounded(1);
        self.lock().task_waiters.push(wake_tx);
        let deadline = Instant::now() + answer_budget(self.config.review.timeout_secs);
        let mut found = None;
        let mut grace: Option<Instant> = None;
        let mut last_error = None;
        loop {
            let (cancelled, session) = {
                let inner = self.lock();
                (
                    inner.review_cancel.contains(&review_id),
                    inner
                        .sessions
                        .get(&session_id)
                        .map(|s| (s.state.is_terminal(), s.activity.clone())),
                )
            };
            if cancelled {
                return Err(End::Cancelled);
            }
            let Some((exited, activity)) = session else {
                return Err(End::Failed("the reviewer session was closed".into()));
            };
            let hooked = activity.evidence == Some(ActivityEvidence::Hook);
            let stopped = hooked && activity.state == ActivityState::Idle;
            let waiting = hooked && activity.state == ActivityState::Waiting;
            if exited || stopped || waiting {
                match read_answer(&mut found, cwd, answer) {
                    Ok(output) => return Ok(output),
                    Err(error) if exited => {
                        return Err(End::Failed(format!(
                            "the reviewer ended without a valid review: {error}"
                        )))
                    }
                    Err(error) => {
                        let until = *grace.get_or_insert_with(|| Instant::now() + ANSWER_GRACE);
                        if Instant::now() >= until {
                            let state = if stopped {
                                "stopped"
                            } else {
                                "is waiting for input"
                            };
                            return Err(End::Failed(format!(
                                "the reviewer {state} without a valid review: {error}"
                            )));
                        }
                        last_error = Some(error);
                    }
                }
            } else {
                grace = None;
            }
            let wait_until = grace.map_or(deadline, |grace| grace.min(deadline));
            match wake_rx.recv_deadline(wait_until) {
                Ok(()) | Err(flume::RecvTimeoutError::Disconnected) => {}
                Err(flume::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(flume::RecvTimeoutError::Timeout) => {
                    return Err(End::Failed(match last_error {
                        Some(error) => {
                            format!("timed out; the last answer was not a review: {error}")
                        }
                        None => "timed out".into(),
                    }));
                }
            }
        }
    }

    /// The provider's config directory as a profile-less launch got it: from
    /// the resolved login-shell environment, which the reviewer's spawn used.
    fn launch_config_dir(&self, provider: &str) -> Option<PathBuf> {
        let var = self
            .lock()
            .agents
            .descriptor(&AgentProviderId::new(provider))?
            .config_dir
            .as_ref()?
            .vars
            .first()?
            .clone();
        self.launch_var(&var).map(PathBuf::from)
    }

    fn stop_reviewer(self: &Arc<Self>, session_id: SessionId) {
        if let Err(error) = self.kill_and_close_session(session_id) {
            tracing::debug!(%session_id, %error, "reviewer already gone");
        }
    }

    fn review_stage(&self, run: &mut ReviewRun, stage: ReviewStage) -> Result<(), End> {
        if self.lock().review_cancel.contains(&run.id) {
            return Err(End::Cancelled);
        }
        run.stage = Some(stage);
        self.publish_review(run);
        Ok(())
    }

    fn publish_review(&self, run: &ReviewRun) {
        if let Err(error) = self.review_store.update_run(run) {
            tracing::warn!(review_id = %run.id, %error, "could not save a review update");
        }
        self.registry
            .broadcast_domain(DaemonEvent::ReviewUpdated { run: run.clone() });
    }

    fn finish_review(&self, run: &mut ReviewRun) {
        run.finished_at = Some(Timestamp::now());
        self.publish_review(run);
    }

    /// Ends one worker's claim: called once per accepted start, by its guard or
    /// by the path that failed to start it.
    fn release_review(&self, slot: &str, review_id: ReviewId) {
        let mut inner = self.lock();
        inner.review_workers = inner.review_workers.saturating_sub(1);
        if inner
            .reviews_running
            .get(slot)
            .is_some_and(|running| running.id == review_id)
        {
            inner.reviews_running.remove(slot);
        }
        inner.review_cancel.remove(&review_id);
    }
}

/// Score, verdict and `auto_approvable` over the findings that are not dismissed.
fn rescore(run: &mut ReviewRun, findings: &[domain::Finding], settings: &review::AutoApprove) {
    let standing: Vec<domain::Finding> = findings
        .iter()
        .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
        .cloned()
        .collect();
    let score = review::score(&standing);
    run.verdict = Some(review::verdict(&standing, score));
    run.score = Some(score);
    run.auto_approvable = review::auto_approvable(settings, &standing, score, &run.coverage);
}

/// `Instant + Duration` panics on overflow, so an absurd `timeout_secs` is capped.
const MAX_ANSWER_SECS: u64 = 24 * 3600;

fn answer_budget(timeout_secs: u64) -> Duration {
    Duration::from_secs(timeout_secs.min(MAX_ANSWER_SECS))
}

/// Nearest rank over sorted values.
fn percentile(sorted: &[i64], at: usize) -> u64 {
    let rank = (sorted.len() * at).div_ceil(100).max(1) - 1;
    sorted
        .get(rank.min(sorted.len().saturating_sub(1)))
        .and_then(|value| u64::try_from(*value).ok())
        .unwrap_or(0)
}

/// `part / whole` as a percentage to a tenth.
fn rate(part: u64, whole: u64) -> Option<f32> {
    #[allow(
        clippy::cast_precision_loss,
        reason = "counts of runs and queries are small"
    )]
    let percent = (whole > 0).then(|| (part as f64 * 1000.0 / whole as f64).round() / 10.0);
    #[allow(clippy::cast_possible_truncation, reason = "a percentage fits an f32")]
    percent.map(|percent| percent as f32)
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Base => "base",
        Tier::Plus => "plus",
        Tier::Apex => "apex",
    }
}

fn unix_now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// A workspace has one run in flight, and each of its pull requests one more.
fn review_slot(target: &ReviewTarget) -> String {
    match target {
        ReviewTarget::PullRequest {
            workspace_id,
            number,
            ..
        } => format!("{workspace_id}#pr{number}"),
        _ => target.workspace_id().to_string(),
    }
}

type DiffStage = (
    Option<String>,
    Option<String>,
    git_service::WorkingTreeDiff,
    Vec<git_service::CommitEntry>,
);

/// Keeps what a workspace review saw so `GetReviewDiff` can show it after the
/// checkout moves on. A patch dropped for its size, or a binary, is left out:
/// its content would be written into the object store for no patch.
fn snapshot(
    path: &Path,
    diff: &git_service::WorkingTreeDiff,
    pack_dir: &Path,
) -> Result<String, git_service::GitError> {
    let paths: Vec<&str> = diff
        .files
        .iter()
        .filter(|file| !file.truncated && !file.binary)
        .map(|file| file.path.as_str())
        .collect();
    create_pack_dir(pack_dir)?;
    git_service::snapshot_tree(path, &paths, &pack_dir.join("index"))
}

/// A file as the run reviewed it: the pull request's head, the kept tree, or
/// the working tree when no tree was kept.
fn reviewed_text(repo: &Path, run: &ReviewRun, file: &str) -> Option<String> {
    match (&run.target, &run.tree_oid) {
        (ReviewTarget::PullRequest { .. }, _) => read_blob(repo, &run.head_oid, file),
        (_, Some(tree)) => read_blob(repo, tree, file),
        (_, None) => read_inside(repo, Path::new(file), MAX_FILE_TEXT_BYTES)
            .ok()
            .flatten(),
    }
}

/// `<head>:<path>` from the object store, `None` over [`MAX_FILE_TEXT_BYTES`]
/// or when it cannot be read.
fn read_blob(repo: &Path, head: &str, path: &str) -> Option<String> {
    let object = format!("{head}:{path}");
    let cap = usize::try_from(MAX_FILE_TEXT_BYTES).unwrap_or(usize::MAX);
    let out = git_service::run_git_bounded(Some(repo), &["cat-file", "blob", &object], cap).ok()?;
    out.success().then_some(out.stdout)
}

struct Launch {
    provider: String,
    options: domain::AgentLaunchOptions,
    /// One agent per hypothesis; empty is the single v1 reviewer.
    swarm: Vec<Hypothesis>,
}

/// Every reviewer the run launched, stopped however the stage ends.
struct Reviewers<'a> {
    daemon: &'a Arc<Daemon>,
    sessions: Vec<SessionId>,
}

impl Drop for Reviewers<'_> {
    fn drop(&mut self) {
        for session in self.sessions.drain(..) {
            self.daemon.stop_reviewer(session);
        }
    }
}

/// What validation left out, summed over a run's agents.
#[derive(Default)]
struct Tally {
    trimmed: u32,
    low_confidence: u32,
    suppressed: u32,
    unverified: u32,
}

impl Tally {
    fn add(&mut self, validated: &review::Validated) {
        self.low_confidence += validated.low_confidence;
        self.suppressed += validated.suppressed;
        self.unverified += validated.unverified;
    }

    fn notes(&self, run: &mut ReviewRun, resolved: &review::Resolved) {
        if self.trimmed > 0 {
            run.coverage.notes.push(format!(
                "{} finding(s) past max_findings ({}) were left out",
                self.trimmed, resolved.max_findings
            ));
        }
        if self.low_confidence > 0 {
            run.coverage.notes.push(format!(
                "{} finding(s) below min_confidence {} were dropped",
                self.low_confidence, resolved.min_confidence
            ));
        }
        if self.suppressed > 0 {
            run.coverage.notes.push(format!(
                "{} finding(s) suppressed by feedback",
                self.suppressed
            ));
        }
        run.coverage.unverified = self.unverified;
        if self.unverified > 0 {
            run.coverage.notes.push(format!(
                "{} finding(s) could not be verified",
                self.unverified
            ));
        }
    }
}

fn write_private(file: &Path, text: &str) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(file)
        .and_then(|mut handle| handle.write_all(text.as_bytes()))
}

struct Stages<'a> {
    path: &'a Path,
    launch: &'a Launch,
    base_ref: Option<String>,
    resolved: &'a review::Resolved,
    context: &'a [(String, String)],
    pack_dir: &'a Path,
}

/// Where the reviewer's answer is written: the session id Forge minted for
/// its transcript, in the provider's config directory.
struct Answer<'a> {
    transcript: &'a str,
    provider: &'a str,
    config_dir: Option<PathBuf>,
}

/// The reviewer's last answer, parsed. `found` keeps the transcript's path
/// once it exists, so a re-read opens one known file rather than scanning.
fn read_answer(
    found: &mut Option<PathBuf>,
    cwd: &Path,
    answer: &Answer<'_>,
) -> Result<review::ReviewOutput, String> {
    let provider = answer.provider;
    if found.is_none() {
        *found = crate::external_agents::minted_transcript(
            provider,
            answer.config_dir.as_deref(),
            cwd,
            answer.transcript,
        );
    }
    let path = found
        .as_deref()
        .ok_or_else(|| "no transcript found".to_owned())?;
    let text = crate::external_agents::last_assistant_message(provider, path)
        .ok_or_else(|| "the transcript has no answer".to_owned())?;
    review::parse_output(&text)
}

/// Releases the run's claim however the worker ends: `Daemon::lock` recovers
/// from poisoning, so a panicked worker would otherwise hold a slot forever.
struct ReviewGuard {
    daemon: Arc<Daemon>,
    slot: String,
    review_id: ReviewId,
}

impl Drop for ReviewGuard {
    fn drop(&mut self) {
        self.daemon.release_review(&self.slot, self.review_id);
    }
}

/// Under the Forge home, never beside a configurable worktrees root, which
/// may sit inside a repository.
const PACKS_DIR: &str = "review-packs";

/// The pack holds diff excerpts and project context: only the user reads it.
fn create_pack_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// Remove every pack a crash left behind; run before any review can start.
pub(crate) fn sweep_packs(home: &Path) {
    if let Err(error) = std::fs::remove_dir_all(home.join(PACKS_DIR)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, "could not remove leftover review packs");
        }
    }
}

/// The pack lives only as long as the run.
struct PackDir(PathBuf);

impl Drop for PackDir {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(dir = %self.0.display(), %error, "could not remove a review pack");
            }
        }
    }
}

/// `.forge/review/` of a working tree, every problem a note rather than a refusal.
struct ReviewDir {
    config: review::Config,
    rules: Vec<review::Rule>,
    /// The built-ins merged with the repository's.
    presets: Vec<review::Preset>,
    problems: Vec<String>,
}

/// `.forge/review/config.toml` alone, problems dropped: defaults stand in for a bad file.
pub(crate) fn repo_config(root: &Path) -> review::Config {
    read_config(root, &mut Vec::new())
}

fn read_config(root: &Path, problems: &mut Vec<String>) -> review::Config {
    match read_inside(
        root,
        Path::new(".forge/review/config.toml"),
        MAX_REPO_FILE_BYTES,
    ) {
        Ok(None) => review::Config::default(),
        Ok(Some(text)) => {
            let (config, notes) = review::Config::parse(&text);
            problems.extend(notes);
            config
        }
        Err(error) => {
            problems.push(format!(".forge/review/config.toml: {error}; defaults used"));
            review::Config::default()
        }
    }
}

/// `global` is `[review] instructions`, the lowest instruction layer.
fn load_review_dir(root: &Path, global: &str) -> ReviewDir {
    let mut problems = Vec::new();
    let mut config = read_config(root, &mut problems);
    let global = global.trim();
    if global.len() > review::MAX_INSTRUCTIONS_BYTES {
        problems.push(format!(
            "[review] instructions is over {} bytes; ignored",
            review::MAX_INSTRUCTIONS_BYTES
        ));
    } else {
        config.base_instructions.push(global.to_owned());
    }
    match read_inside(root, Path::new(REPO_PROMPT), MAX_REPO_PROMPT_BYTES) {
        Ok(Some(text)) => config.base_instructions.push(text),
        Ok(None) => {}
        Err(error) => problems.push(format!("{REPO_PROMPT}: {error}; ignored")),
    }
    let rule_files = read_dir_files(root, "rules", "md", MAX_RULE_FILES, &mut problems);
    let (rules, errors) = review::parse_rules(&rule_files);
    problems.extend(errors);
    let preset_files = read_dir_files(root, "presets", "toml", MAX_PRESET_FILES, &mut problems);
    let (presets, errors) = review::parse_presets(&preset_files, &rules);
    problems.extend(errors);
    ReviewDir {
        config,
        rules,
        presets,
        problems,
    }
}

/// `(file name, text)` of `.forge/review/<dir>/*.<extension>`, sorted by name;
/// past `cap` files, or a file over [`MAX_REPO_FILE_BYTES`], is a problem.
fn read_dir_files(
    root: &Path,
    dir: &str,
    extension: &str,
    cap: usize,
    problems: &mut Vec<String>,
) -> Vec<(String, String)> {
    let relative = Path::new(".forge/review").join(dir);
    let Ok(root) = std::fs::canonicalize(root) else {
        return Vec::new();
    };
    let full = match std::fs::canonicalize(root.join(&relative)) {
        Ok(full) if full.starts_with(&root) => full,
        Ok(_) => {
            problems.push(format!(
                ".forge/review/{dir}: outside the checkout; ignored"
            ));
            return Vec::new();
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            problems.push(format!(".forge/review/{dir}: {error}; ignored"));
            return Vec::new();
        }
    };
    let Ok(entries) = std::fs::read_dir(&full) else {
        return Vec::new();
    };
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for name in entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext == extension)
        })
    {
        total += 1;
        names.insert(name);
        // Keep the `cap` names a full sort would have kept, without holding a
        // repository's whole directory in memory first.
        if names.len() > cap {
            names.pop_last();
        }
    }
    if total > cap {
        problems.push(format!(
            ".forge/review/{dir}: only the first {cap} of {total} files were read"
        ));
    }
    names
        .into_iter()
        .filter_map(
            |name| match read_inside(&root, &relative.join(&name), MAX_REPO_FILE_BYTES) {
                Ok(Some(text)) => Some((name, text)),
                Ok(None) => None,
                Err(error) => {
                    problems.push(format!("{name}: {error}"));
                    None
                }
            },
        )
        .collect()
}

/// `.forge/review/context.md`, then `AGENTS.md` and `CLAUDE.md` unless
/// `project_docs` is off, then each `[context] include`, within `max_bytes`
/// together. A file reached twice, by name or through a symlink, is read once.
fn read_context(
    root: &Path,
    settings: &review::ContextSettings,
    notes: &mut Vec<String>,
) -> Vec<(String, String)> {
    let mut left = u64::try_from(settings.max_bytes).unwrap_or(u64::MAX);
    let mut texts = Vec::new();
    let mut seen = Vec::new();
    let docs: &[&str] = if settings.project_docs {
        &PROJECT_DOCS
    } else {
        &[]
    };
    let paths = std::iter::once((".forge/review/context.md", true))
        .chain(docs.iter().map(|path| (*path, true)))
        .chain(settings.include.iter().map(|path| (path.as_str(), false)));
    for (path, optional) in paths {
        let full = root.join(path);
        let identity = std::fs::canonicalize(&full).unwrap_or(full);
        if seen.contains(&identity) {
            continue;
        }
        seen.push(identity);
        match read_inside(root, Path::new(path), left) {
            Ok(Some(text)) => {
                left = left.saturating_sub(u64::try_from(text.len()).unwrap_or(u64::MAX));
                texts.push((path.to_owned(), text));
            }
            Ok(None) if optional => {}
            Ok(None) => notes.push(format!("review context: {path} is missing; skipped")),
            Err(error) => notes.push(format!("review context: {path}: {error}; skipped")),
        }
    }
    texts
}

/// A repository file inside `root`, as [`read_inside`] reads it.
pub(crate) fn read_repo_file(
    root: &Path,
    relative: &str,
    cap: u64,
) -> Result<Option<String>, String> {
    read_inside(root, Path::new(relative), cap)
}

/// A file of the checkout up to `cap` bytes: `Ok(None)` when it does not
/// exist, an error when it escapes the checkout, is not a file, is over the
/// cap or cannot be read as text.
fn read_inside(root: &Path, relative: &Path, cap: u64) -> Result<Option<String>, String> {
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let full = match std::fs::canonicalize(root.join(relative)) {
        Ok(full) => full,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !full.starts_with(&root) {
        return Err("outside the checkout".into());
    }
    let meta = std::fs::metadata(&full).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > cap {
        return Err(format!("over the {cap}-byte budget"));
    }
    let mut text = String::new();
    std::fs::File::open(&full)
        .and_then(|file| file.take(cap).read_to_string(&mut text))
        .map_err(|error| error.to_string())?;
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_budget_is_capped_at_a_day() {
        assert_eq!(answer_budget(900), Duration::from_secs(900));
        assert_eq!(answer_budget(u64::MAX), Duration::from_secs(24 * 3600));
    }

    #[test]
    fn review_files_keep_the_sorted_prefix_and_report_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rules = dir.path().join(".forge/review/rules");
        std::fs::create_dir_all(&rules).expect("rules directory");
        for index in 0..(MAX_RULE_FILES + 2) {
            std::fs::write(rules.join(format!("{index:03}.md")), "").expect("rule file");
        }
        let mut problems = Vec::new();
        let files = read_dir_files(dir.path(), "rules", "md", MAX_RULE_FILES, &mut problems);
        assert_eq!(files.len(), MAX_RULE_FILES);
        assert_eq!(files.first().map(|(name, _)| name.as_str()), Some("000.md"));
        assert_eq!(files.last().map(|(name, _)| name.as_str()), Some("099.md"));
        assert!(problems.iter().any(|note| note.contains("only the first")));
    }

    #[test]
    fn review_rule_directory_outside_the_checkout_is_refused() {
        let root = tempfile::tempdir().expect("checkout");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("secret.md"), "text").expect("secret");
        let rules = root.path().join(".forge/review/rules");
        std::fs::create_dir_all(rules.parent().expect("review dir")).expect("review dir");
        std::os::unix::fs::symlink(outside.path(), &rules).expect("symlink");

        let mut problems = Vec::new();
        let files = read_dir_files(root.path(), "rules", "md", MAX_RULE_FILES, &mut problems);
        assert!(files.is_empty());
        assert!(problems
            .iter()
            .any(|note| note.contains("outside the checkout")));
        assert!(!problems.iter().any(|note| note.contains("secret.md")));
    }
}
