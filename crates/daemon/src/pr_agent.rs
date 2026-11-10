//! `OpenPullRequestWithAgent`: a background agent drafts the pull request,
//! then the daemon commits, pushes and opens it.
//!
//! The agent is an ordinary agent session marked `background`, launched with
//! `[pull_requests] system_prompt` and the change, and closed as soon as it
//! hands its draft over through `pr_draft`. The worker holds the workspace's
//! `pr_opening` from the request to `PullRequestOpened`, so a second press
//! while one is in flight is absorbed, as for every other pull-request request.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use domain::{AgentProfileId, AgentProviderId, SessionId, SessionKind, SessionRole, WorkspaceId};
use protocol::{DaemonEvent, ErrorCode, ProtocolError, Response};

use crate::core::Daemon;

/// The background session's title, as the Processes view shows it.
const AGENT_TITLE: &str = "Open PR";

/// Why a run ended without a draft, said to the user as is.
type Failure = String;

impl Daemon {
    /// Validate, launch the drafting agent and ack; the rest happens on a worker.
    pub(crate) fn open_pull_request_with_agent(
        self: &Arc<Self>,
        workspace_id: WorkspaceId,
        provider_id: AgentProviderId,
        profile_id: Option<AgentProfileId>,
        draft: bool,
    ) -> Result<Response, ProtocolError> {
        let path = self.workspace_path(workspace_id)?;
        let diff = git_service::working_tree_diff(&path, None, git_service::DIFF_CONTEXT_LINES)
            .map_err(git_refusal)?;
        let status = git_service::status(&path).map_err(git_refusal)?;
        let drafted = CheckoutPrint::of(&status, &diff);
        let default_branch = git_service::default_branch(&path).ok().flatten();
        let on_default =
            domain::branch_step(status.branch.as_deref(), default_branch.as_deref(), None).is_err();
        if diff.files.is_empty() && on_default {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "nothing to propose: this checkout is on its default branch with no uncommitted \
                 changes",
            ));
        }
        let prompt = domain::agent_prompt(
            &self.config.pull_requests.system_prompt,
            &domain::WorkspaceDiff {
                workspace_id,
                branch: diff.branch,
                files: diff.files.into_iter().map(crate::core::diff_file).collect(),
                truncated: diff.truncated,
            },
            &domain::BranchFacts {
                default_branch,
                ahead: status.ahead,
            },
        );

        if !self.lock().pr_opening.insert(workspace_id) {
            return Ok(Response::Ack);
        }
        let daemon = Arc::clone(self);
        std::thread::Builder::new()
            .name("forge-pr-agent".into())
            .spawn(move || {
                let _guard = crate::core::PrOpeningGuard::new(Arc::clone(&daemon), workspace_id);
                daemon.run_pr_agent(
                    &path,
                    workspace_id,
                    provider_id,
                    profile_id,
                    prompt,
                    draft,
                    &drafted,
                );
            })
            .map_err(|error| {
                self.lock().pr_opening.remove(&workspace_id);
                ProtocolError::new(
                    ErrorCode::IoError,
                    format!("cannot start the pull-request agent: {error}"),
                )
            })?;
        Ok(Response::Ack)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the request's fields plus what was read before the ack, passed through once"
    )]
    fn run_pr_agent(
        self: &Arc<Self>,
        path: &Path,
        workspace_id: WorkspaceId,
        provider_id: AgentProviderId,
        profile_id: Option<AgentProfileId>,
        prompt: String,
        draft: bool,
        drafted: &CheckoutPrint,
    ) {
        let options = domain::AgentLaunchOptions {
            model: self.config.pull_requests.model.clone(),
            effort: self.config.pull_requests.effort.clone(),
        };
        let session_id = match self.create_session_with_options(
            workspace_id,
            SessionKind::Agent,
            Some(provider_id),
            profile_id,
            None,
            SessionRole::Generic,
            None,
            Some(prompt),
            false,
            options,
            Some(AGENT_TITLE.into()),
            None,
            true,
        ) {
            Ok(Response::SessionCreated { session_id, .. }) => session_id,
            Ok(_) => {
                return self.pr_agent_failed(workspace_id, None, "the agent did not start".into())
            }
            Err(error) => {
                return self.pr_agent_failed(
                    workspace_id,
                    None,
                    format!("the agent did not start: {}", error.message),
                )
            }
        };
        let outcome = self.await_draft(session_id);
        if let Err(error) = self.kill_and_close_session(session_id) {
            tracing::debug!(%session_id, error = %error.message, "pull-request agent already gone");
        }
        self.lock().pr_agents.remove(&session_id);
        let pr = match outcome {
            Ok(pr) => pr,
            Err(failure) => return self.pr_agent_failed(workspace_id, Some(session_id), failure),
        };
        // The text describes the checkout as it was when the agent read it; a
        // branch switch or a new edit since would publish something else.
        let now = git_service::working_tree_diff(path, None, git_service::DIFF_CONTEXT_LINES)
            .and_then(|diff| Ok(CheckoutPrint::of(&git_service::status(path)?, &diff)));
        if now.as_ref().ok() != Some(drafted) {
            return self.pr_agent_failed(
                workspace_id,
                Some(session_id),
                "the checkout changed while the agent was drafting (branch, commit or \
                 uncommitted files); nothing was committed or pushed, so open the pull request \
                 again"
                    .into(),
            );
        }
        let commit = drafted.dirty.then(|| domain::CommitPlan {
            branch: Some(pr.branch.clone()).filter(|branch| !branch.trim().is_empty()),
            message: pr.commit_message.clone(),
        });
        self.finish_pull_request(
            path,
            workspace_id,
            &pr.title,
            &pr.body,
            None,
            draft,
            commit.as_ref(),
            Some(session_id),
        );
    }

    /// Block until the agent hands its draft over, exits, or runs out of time.
    fn await_draft(&self, session_id: SessionId) -> Result<domain::PullRequestDraft, Failure> {
        let (wake_tx, wake_rx) = flume::bounded(1);
        self.lock().task_waiters.push(wake_tx);
        let budget = self.config.pull_requests.timeout();
        let deadline = Instant::now() + budget;
        loop {
            {
                let mut inner = self.lock();
                if let Some(draft) = inner.pr_agents.get_mut(&session_id).and_then(Option::take) {
                    return Ok(draft);
                }
                match inner.sessions.get(&session_id) {
                    None => return Err("the agent's session was closed".into()),
                    Some(session) if session.state.is_terminal() => {
                        return Err("the agent stopped without handing a draft over".into())
                    }
                    Some(_) => {}
                }
            }
            match wake_rx.recv_deadline(deadline) {
                Ok(()) => {}
                Err(flume::RecvTimeoutError::Disconnected) => {
                    return Err("the daemon was reset while the agent was drafting".into())
                }
                Err(flume::RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "the agent handed no draft over within {} s",
                        budget.as_secs()
                    ))
                }
            }
        }
    }

    fn pr_agent_failed(
        &self,
        workspace_id: WorkspaceId,
        session_id: Option<SessionId>,
        error: Failure,
    ) {
        self.registry
            .broadcast_domain(DaemonEvent::PullRequestOpened {
                workspace_id,
                url: None,
                error: Some(error),
                session_id,
            });
    }
}

/// What the draft was written against: the branch, its commit, and every
/// uncommitted change down to its patch.
#[derive(Debug, PartialEq, Eq)]
struct CheckoutPrint {
    branch: Option<String>,
    head: Option<String>,
    dirty: bool,
    files: u64,
}

impl CheckoutPrint {
    fn of(status: &git_service::RepoStatus, diff: &git_service::WorkingTreeDiff) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for file in &diff.files {
            (&file.path, file.additions, file.deletions, &file.patch).hash(&mut hasher);
        }
        diff.truncated.hash(&mut hasher);
        Self {
            branch: status.branch.clone(),
            head: status.head.clone(),
            dirty: status.dirty,
            files: hasher.finish(),
        }
    }
}

fn git_refusal(error: git_service::GitError) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidRequest, error.to_string())
}
