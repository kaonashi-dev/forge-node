//! The fix loop: findings to a new writable agent, then the same review
//! again, until a review finds nothing or `max_iterations` reviews ran. It
//! never commits or pushes; the person asked for it before it started.

use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{
    ActivityEvidence, ActivityState, AgentProfileId, AgentProviderId, FindingReaction, ReviewId,
    ReviewRun, ReviewStatus, ReviewTarget, SessionKind, SessionRole, WorkspaceId,
};
use protocol::{DaemonEvent, ErrorCode, ProtocolError, Response};

use crate::core::{db_err, Daemon};

const DEFAULT_ITERATIONS: u32 = 3;
const MAX_ITERATIONS: u32 = 10;
const FIXER_TITLE: &str = "Forge Review · fix loop";

/// Clears the workspace's loop however the worker ends.
struct LoopGuard {
    daemon: Arc<Daemon>,
    workspace_id: WorkspaceId,
}

impl Drop for LoopGuard {
    fn drop(&mut self) {
        self.daemon.lock().fix_loops.remove(&self.workspace_id);
    }
}

enum Stop {
    Cancelled,
    Failed(String),
}

pub(crate) struct FixLoop {
    review_id: ReviewId,
    workspace_id: WorkspaceId,
    max_iterations: u32,
    provider: AgentProviderId,
    profile_id: Option<AgentProfileId>,
}

impl Daemon {
    pub(crate) fn start_fix_loop(
        self: &Arc<Self>,
        review_id: ReviewId,
        max_iterations: Option<u32>,
        provider: Option<String>,
        profile_id: Option<AgentProfileId>,
    ) -> Result<Response, ProtocolError> {
        let (run, _) = self
            .review_store
            .get(review_id)
            .map_err(db_err)?
            .ok_or_else(|| ProtocolError::not_found("review"))?;
        if !matches!(run.target, ReviewTarget::Workspace { .. }) {
            return Err(ProtocolError::invalid_request(
                "a fix loop works on a workspace review, never a pull request's",
            ));
        }
        let workspace_id = run.workspace_id;
        if self.lock().fix_loops.insert(workspace_id, false).is_some() {
            return Err(ProtocolError::conflict(
                "a fix loop is already running for this workspace",
            ));
        }
        let guard = LoopGuard {
            daemon: Arc::clone(self),
            workspace_id,
        };
        let job = FixLoop {
            review_id,
            workspace_id,
            max_iterations: max_iterations
                .unwrap_or(DEFAULT_ITERATIONS)
                .clamp(1, MAX_ITERATIONS),
            provider: AgentProviderId::new(
                provider.unwrap_or_else(|| self.config.review.provider.clone()),
            ),
            profile_id,
        };
        let daemon = Arc::clone(self);
        std::thread::Builder::new()
            .name("forge-fix-loop".into())
            .spawn(move || {
                let _guard = guard;
                let (iteration, findings, error) = daemon.fix_loop(&job);
                daemon
                    .registry
                    .broadcast_domain(DaemonEvent::FixLoopUpdated {
                        workspace_id: job.workspace_id,
                        iteration,
                        findings,
                        done: true,
                        error,
                    });
            })
            .map_err(|error| {
                ProtocolError::new(
                    ErrorCode::IoError,
                    format!("cannot start the fix loop: {error}"),
                )
            })?;
        Ok(Response::FixLoopStarted { workspace_id })
    }

    pub(crate) fn cancel_fix_loop(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Response, ProtocolError> {
        let mut inner = self.lock();
        match inner.fix_loops.get_mut(&workspace_id) {
            Some(cancel) => {
                *cancel = true;
                crate::actions::run::wake_waiters(&mut inner);
                Ok(Response::Ack)
            }
            None => Err(ProtocolError::not_found("fix loop")),
        }
    }

    /// `(reviews run, findings standing after the last, error)`.
    fn fix_loop(self: &Arc<Self>, job: &FixLoop) -> (u32, u32, Option<String>) {
        let mut current = job.review_id;
        let mut iteration = 0;
        loop {
            let (run, standing) = match self.standing(current) {
                Ok(found) => found,
                Err(error) => return (iteration, 0, Some(error)),
            };
            let count = u32::try_from(standing.len()).unwrap_or(u32::MAX);
            if standing.is_empty() || iteration >= job.max_iterations {
                return (iteration, count, None);
            }
            let step = self
                .fix_once(job, &standing)
                .and_then(|()| self.review_again(job, &run));
            match step {
                Ok(next) => current = next,
                Err(Stop::Cancelled) => return (iteration, count, Some("cancelled".into())),
                Err(Stop::Failed(error)) => return (iteration, count, Some(error)),
            }
            iteration += 1;
            let after = self
                .standing(current)
                .map_or(0, |(_, findings)| findings.len());
            self.registry.broadcast_domain(DaemonEvent::FixLoopUpdated {
                workspace_id: job.workspace_id,
                iteration,
                findings: u32::try_from(after).unwrap_or(u32::MAX),
                done: false,
                error: None,
            });
        }
    }

    fn standing(&self, review_id: ReviewId) -> Result<(ReviewRun, Vec<domain::Finding>), String> {
        let (run, findings) = self
            .review_store
            .get(review_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the review is gone".to_owned())?;
        if run.status != ReviewStatus::Completed {
            return Err(format!(
                "the review ended {:?}: {}",
                run.status,
                run.error.clone().unwrap_or_default()
            ));
        }
        let standing = findings
            .into_iter()
            .filter(|finding| finding.reaction != Some(FindingReaction::Dismiss))
            .collect();
        Ok((run, standing))
    }

    /// The findings to a new writable agent, as "Fix with a new agent" sends
    /// them; done when it idles or exits, then closed.
    fn fix_once(self: &Arc<Self>, job: &FixLoop, standing: &[domain::Finding]) -> Result<(), Stop> {
        let listed: Vec<&domain::Finding> = standing.iter().collect();
        let (prompt, _) = domain::review::findings_prompt(&listed, client::MAX_PROMPT_BYTES);
        if prompt.is_empty() {
            return Err(Stop::Failed("no finding fits the agent's prompt".into()));
        }
        let session = match self.create_session_with_options(
            job.workspace_id,
            SessionKind::Agent,
            Some(job.provider.clone()),
            job.profile_id,
            None,
            SessionRole::Generic,
            None,
            Some(prompt),
            false,
            domain::AgentLaunchOptions::default(),
            Some(FIXER_TITLE.into()),
            None,
            false,
        ) {
            Ok(Response::SessionCreated { session_id, .. }) => session_id,
            Ok(_) => return Err(Stop::Failed("the fixer did not start".into())),
            Err(error) => return Err(Stop::Failed(format!("the fixer did not start: {error}"))),
        };
        let done = self.wait_loop(job.workspace_id, |daemon| {
            let inner = daemon.lock();
            inner.sessions.get(&session).is_none_or(|session| {
                let idle = session.activity.evidence == Some(ActivityEvidence::Hook)
                    && session.activity.state == ActivityState::Idle;
                idle || session.state.is_terminal()
            })
        });
        if let Err(error) = self.kill_and_close_session(session) {
            tracing::debug!(%session, %error, "fixer already gone");
        }
        done
    }

    fn review_again(self: &Arc<Self>, job: &FixLoop, run: &ReviewRun) -> Result<ReviewId, Stop> {
        let base = match &run.target {
            ReviewTarget::Workspace { base, .. } => base.clone(),
            _ => None,
        };
        let started = self.start_review(
            ReviewTarget::Workspace {
                workspace_id: job.workspace_id,
                base,
            },
            Some(run.tier),
            Some(run.provider.clone()),
            Some(run.preset.clone()),
            None,
            domain::AgentLaunchOptions::default(),
            domain::SwarmOptions {
                size: Some(run.swarm_size),
                hypotheses: Vec::new(),
            },
        );
        let review_id = match started {
            Ok(Response::ReviewStarted { review_id }) => review_id,
            Ok(_) => return Err(Stop::Failed("the review did not start".into())),
            Err(error) => return Err(Stop::Failed(format!("the review did not start: {error}"))),
        };
        self.wait_loop(job.workspace_id, |daemon| {
            let running = daemon
                .lock()
                .reviews_running
                .values()
                .any(|running| running.id == review_id);
            !running
                && daemon
                    .review_store
                    .get(review_id)
                    .ok()
                    .flatten()
                    .is_some_and(|(run, _)| run.status.is_finished())
        })?;
        Ok(review_id)
    }

    /// Wakes on every daemon change a task waiter hears; gives up at
    /// `[review] timeout_secs`, or when the person cancels the loop. `ready`
    /// takes the core lock itself, and only briefly.
    fn wait_loop(
        &self,
        workspace_id: WorkspaceId,
        mut ready: impl FnMut(&Self) -> bool,
    ) -> Result<(), Stop> {
        let (wake_tx, wake_rx) = flume::bounded(1);
        self.lock().task_waiters.push(wake_tx);
        let budget = Duration::from_secs(self.config.review.timeout_secs.clamp(1, 24 * 3600));
        let deadline = Instant::now() + budget;
        loop {
            let cancelled = self.lock().fix_loops.get(&workspace_id) == Some(&true);
            if cancelled {
                return Err(Stop::Cancelled);
            }
            if ready(self) {
                return Ok(());
            }
            if let Err(flume::RecvTimeoutError::Timeout) = wake_rx.recv_deadline(deadline) {
                return Err(Stop::Failed("timed out".into()));
            }
        }
    }
}
