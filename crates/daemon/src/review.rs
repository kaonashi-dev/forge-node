//! Forge Review: one worker thread per run, `diff → pack → model → validate`,
//! with git, the pack, the transcript and SQLite outside the core lock. The
//! reviewer is an ordinary read-only agent session, read when its hook idles.

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{
    ActivityEvidence, ActivityState, AgentProviderId, FindingReaction, ProjectId, ReviewId,
    ReviewRun, ReviewStage, ReviewStatus, ReviewTarget, SessionId, SessionKind, SessionRole, Tier,
    Timestamp, WorkspaceId,
};
use protocol::{DaemonEvent, ErrorCode, ProtocolError, Response};

use crate::core::{db_err, Daemon};

const MAX_RUNNING_REVIEWS: usize = 2;
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

enum End {
    Cancelled,
    Failed(String),
}

impl Daemon {
    pub(crate) fn start_review(
        self: &Arc<Self>,
        target: ReviewTarget,
        tier: Option<Tier>,
        provider: Option<String>,
        preset: Option<String>,
        instructions: Option<String>,
        options: domain::AgentLaunchOptions,
    ) -> Result<Response, ProtocolError> {
        let workspace_id = target.workspace_id();
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
            "{target_key}\0{provider}\0{:?}\0{:?}\0{}\0{:?}\0{}",
            options.model, options.effort, resolved.preset, resolved.tier, resolved.instructions
        );

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
                Some(running) => {
                    let superseded = running.id;
                    inner.review_cancel.insert(superseded);
                    crate::actions::run::wake_waiters(&mut inner);
                }
                None if inner.reviews_running.len() >= MAX_RUNNING_REVIEWS => {
                    return Err(ProtocolError::conflict(format!(
                        "{MAX_RUNNING_REVIEWS} reviews are already running"
                    )));
                }
                None => {}
            }
            inner
                .reviews_running
                .insert(slot.clone(), RunningReview { id, key });
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
        };
        if let Err(error) = self.review_store.insert_run(&run) {
            self.release_review(&slot, id);
            return Err(db_err(error));
        }
        self.registry
            .broadcast_domain(DaemonEvent::ReviewUpdated { run: run.clone() });

        let daemon = Arc::clone(self);
        let mut queued = run.clone();
        let launch = Launch { provider, options };
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
            let standing: Vec<domain::Finding> = findings
                .into_iter()
                .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
                .collect();
            let score = review::score(&standing);
            run.verdict = Some(review::verdict(&standing, score));
            run.score = Some(score);
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
        self.review_store
            .clear_suppression(project, suppression)
            .map_err(db_err)?;
        Ok(Response::Ack)
    }

    fn workspace_project(&self, workspace_id: WorkspaceId) -> Result<ProjectId, ProtocolError> {
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
            self.worktrees_root_path()
                .parent()
                .unwrap_or(self.worktrees_root_path())
                .join("review-packs")
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
        // Before the pack goes: the reviewer was given it with `--add-dir`.
        if let Some(session_id) = run.session_id {
            self.stop_reviewer(session_id);
        }
        match outcome {
            Ok(mut findings) => {
                run.status = ReviewStatus::Completed;
                if let Some((_, before)) = self.previous_review(&run) {
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
        drop(pack_dir);
        drop(guard);
        self.finish_review(&mut run);
        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::days(KEEP_DAYS);
        if let Err(error) =
            self.review_store
                .prune(run.workspace_id, KEEP_RUNS, Timestamp::from_offset(cutoff))
        {
            tracing::warn!(%error, "could not prune old reviews");
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
    ) -> Result<Vec<domain::Finding>, End> {
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

        self.review_stage(run, ReviewStage::Pack)?;
        let meta = review::PackMeta {
            head_oid: run.head_oid.clone(),
            base_oid: merge,
            base_ref: base_ref.unwrap_or_else(|| "HEAD".into()),
            pull_request: pull_request.as_ref().map(|(number, _)| *number),
        };
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        let rules = review::in_scope(&resolved.rules, &paths);
        let (pack, mut coverage) =
            review::build_pack(&meta, &commits, &files, resolved, &rules, context);
        coverage
            .notes
            .splice(0..0, std::mem::take(&mut run.coverage.notes));
        run.coverage = coverage;
        let pack_file = pack_dir.join("pack.md");
        std::fs::create_dir_all(pack_dir)
            .and_then(|()| std::fs::write(&pack_file, pack))
            .map_err(|error| End::Failed(format!("could not write the review pack: {error}")))?;

        self.review_stage(run, ReviewStage::Model)?;
        let transcript = SessionId::new().as_uuid().to_string();
        let session_id = match self.create_session_with_options(
            run.workspace_id,
            SessionKind::Agent,
            Some(AgentProviderId::new(launch.provider.as_str())),
            None,
            None,
            SessionRole::Generic,
            None,
            Some(review::prompt(resolved, &pack_file)),
            true,
            launch.options.clone(),
            Some(REVIEWER_TITLE.into()),
            Some(domain::StructuredReview {
                session_id: transcript.clone(),
                add_dir: pack_dir.to_path_buf(),
            }),
            false,
        ) {
            Ok(Response::SessionCreated { session_id, .. }) => session_id,
            Ok(_) => return Err(End::Failed("the reviewer did not start".into())),
            Err(error) => return Err(End::Failed(format!("the reviewer did not start: {error}"))),
        };
        run.session_id = Some(session_id);
        self.publish_review(run);
        let output = self.await_answer(run.id, session_id, path, &transcript, &launch.provider)?;

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
        let changed: Vec<review::ChangedFile> = files
            .iter()
            .filter(|file| !file.binary)
            .map(|file| review::ChangedFile {
                path: file.path.clone(),
                patch: file.patch.clone(),
                text: output
                    .findings
                    .iter()
                    .any(|finding| finding.path == file.path)
                    .then(|| match (pull_request.is_some(), &run.tree_oid) {
                        (true, _) => read_blob(path, &run.head_oid, &file.path),
                        (false, Some(tree)) => read_blob(path, tree, &file.path),
                        (false, None) => {
                            read_inside(path, Path::new(&file.path), MAX_FILE_TEXT_BYTES)
                                .ok()
                                .flatten()
                        }
                    })
                    .flatten(),
            })
            .collect();
        let validated = review::validate(output, &changed, &rules, resolved, &suppressions)
            .map_err(End::Failed)?;
        if validated.trimmed > 0 {
            run.coverage.notes.push(format!(
                "{} finding(s) past max_findings ({}) were left out",
                validated.trimmed, resolved.max_findings
            ));
        }
        if validated.low_confidence > 0 {
            run.coverage.notes.push(format!(
                "{} finding(s) below min_confidence {} were dropped",
                validated.low_confidence, resolved.min_confidence
            ));
        }
        if validated.suppressed > 0 {
            run.coverage.notes.push(format!(
                "{} finding(s) suppressed by feedback",
                validated.suppressed
            ));
        }
        run.coverage.withheld = validated.withheld;
        let score = review::score(&validated.findings);
        run.verdict = Some(review::verdict(&validated.findings, score));
        run.score = Some(score);
        run.summary = validated.summary;
        Ok(validated.findings)
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
        transcript: &str,
        provider: &str,
    ) -> Result<review::ReviewOutput, End> {
        let (wake_tx, wake_rx) = flume::bounded(1);
        self.lock().task_waiters.push(wake_tx);
        let deadline = Instant::now() + Duration::from_secs(self.config.review.timeout_secs);
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
                match read_answer(&mut found, cwd, transcript, provider) {
                    Ok(output) => return Ok(output),
                    Err(error) if exited => {
                        return Err(End::Failed(format!(
                            "the reviewer ended without a valid review: {error}"
                        )))
                    }
                    Err(error) if stopped => {
                        let until = *grace.get_or_insert_with(|| Instant::now() + ANSWER_GRACE);
                        if Instant::now() >= until {
                            return Err(End::Failed(format!(
                                "the reviewer stopped without a valid review: {error}"
                            )));
                        }
                        last_error = Some(error);
                    }
                    Err(error) => {
                        grace = None;
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

    fn stop_reviewer(self: &Arc<Self>, session_id: SessionId) {
        if let Err(error) = self.kill_session(session_id) {
            tracing::debug!(%session_id, %error, "reviewer already stopped");
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

    fn release_review(&self, slot: &str, review_id: ReviewId) {
        let mut inner = self.lock();
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
    std::fs::create_dir_all(pack_dir)?;
    git_service::snapshot_tree(path, &paths, &pack_dir.join("index"))
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
}

struct Stages<'a> {
    path: &'a Path,
    launch: &'a Launch,
    base_ref: Option<String>,
    resolved: &'a review::Resolved,
    context: &'a [(String, String)],
    pack_dir: &'a Path,
}

/// The reviewer's last answer, parsed. `found` keeps the transcript's path
/// once it exists, so a re-read opens one known file rather than scanning.
fn read_answer(
    found: &mut Option<PathBuf>,
    cwd: &Path,
    transcript: &str,
    provider: &str,
) -> Result<review::ReviewOutput, String> {
    if found.is_none() {
        *found = crate::external_agents::minted_transcript(provider, cwd, transcript);
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

/// `global` is `[review] instructions`, the lowest instruction layer.
fn load_review_dir(root: &Path, global: &str) -> ReviewDir {
    let mut problems = Vec::new();
    let config = match read_inside(
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
    };
    let mut config = config;
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
