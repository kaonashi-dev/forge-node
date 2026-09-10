//! Harness filesystem handlers (local, synchronous reads like `GetWorkspaceDiff`).

use domain::{HarnessAdvanceAction, HarnessArtifactKind};
use harness_service::{self, HarnessError};
use protocol::{error::ProtocolError, response::Response};

use crate::core::Daemon;

pub(crate) fn harness_err(e: HarnessError) -> ProtocolError {
    match e {
        HarnessError::Conflict(msg) => ProtocolError::conflict(msg),
        HarnessError::NotInitialized => {
            ProtocolError::not_found("harness not initialized in this project")
        }
        HarnessError::NotFound(id) => ProtocolError::not_found(format!("harness feature {id}")),
        HarnessError::Invalid(msg) => ProtocolError::new(protocol::ErrorCode::InvalidRequest, msg),
        HarnessError::Io(e) => ProtocolError::new(protocol::ErrorCode::IoError, e.to_string()),
        HarnessError::Json(e) => {
            ProtocolError::new(protocol::ErrorCode::InvalidRequest, e.to_string())
        }
    }
}

impl Daemon {
    pub(crate) fn list_harness_features(
        &self,
        project_id: domain::ProjectId,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        let list = harness_service::list_features(&root).map_err(harness_err)?;
        Ok(Response::HarnessFeatureList(list))
    }

    pub(crate) fn get_harness_feature(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        let feature = harness_service::get_feature(&root, feature_id).map_err(harness_err)?;
        Ok(Response::HarnessFeature(feature))
    }

    pub(crate) fn get_harness_timeline(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        let events = harness_service::read_timeline(&root, feature_id).map_err(harness_err)?;
        Ok(Response::HarnessTimeline(events))
    }

    pub(crate) fn read_harness_artifact(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
        artifact: HarnessArtifactKind,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        let text =
            harness_service::read_artifact(&root, feature_id, artifact).map_err(harness_err)?;
        Ok(Response::HarnessArtifact { text })
    }

    /// The checkout a harness feature is being registered for.
    ///
    /// Answers `InvalidRequest` rather than silently unattaching the feature
    /// when the workspace belongs to another project: the per-checkout limit
    /// is only as good as the checkout it is told about.
    fn harness_checkout(
        &self,
        project_id: domain::ProjectId,
        workspace_id: Option<domain::WorkspaceId>,
    ) -> Result<Option<String>, ProtocolError> {
        let Some(workspace_id) = workspace_id else {
            return Ok(None);
        };
        let path = self.workspace_path_in(project_id, workspace_id)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    }

    pub(crate) fn register_harness_feature(
        &self,
        project_id: domain::ProjectId,
        workspace_id: Option<domain::WorkspaceId>,
        spec_raw: String,
        title: Option<String>,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        let workspace_path = self.harness_checkout(project_id, workspace_id)?;
        let feature = harness_service::register_feature(
            &root,
            harness_service::RegisterInput {
                spec_raw,
                title,
                source_issue: None,
                workspace_id,
                workspace_path,
            },
        )
        .map_err(harness_err)?;
        Ok(Response::HarnessFeature(feature))
    }

    pub(crate) fn register_harness_from_issue(
        &self,
        project_id: domain::ProjectId,
        workspace_id: Option<domain::WorkspaceId>,
        issue_number: u32,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        let workspace_path = self.harness_checkout(project_id, workspace_id)?;
        let feature =
            harness_service::register_from_issue(&root, issue_number, workspace_id, workspace_path)
                .map_err(harness_err)?;
        Ok(Response::HarnessFeature(feature))
    }

    /// Apply a human decision to a feature, and start what it unblocks.
    ///
    /// Approving a spec is the one gate the cycle stops at, so this is where it
    /// starts again: the implementation step is launched here rather than left
    /// to whichever client happened to click, which is what lets the run
    /// continue after that client goes away. A launch that fails is reported
    /// as the response error — the approval itself is already on disk.
    ///
    /// *Revise* re-runs the spec for the same reason, and it is the half that
    /// was missing: the decision put the feature back to `pending` and then
    /// nothing ran it, so the feature sat open — blocking its checkout — with
    /// no button anywhere that would start it again.
    pub(crate) fn harness_advance(
        self: &std::sync::Arc<Self>,
        project_id: domain::ProjectId,
        feature_id: u32,
        revision: Option<u64>,
        action: HarnessAdvanceAction,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        // The decision and the step it unblocks are no longer two independent
        // guesses: the transition table says both, so a client cannot approve
        // a spec whose gate is already closed and get a second implementer for
        // it. A stale `revision` is a no-op that answers with the current row.
        let blocking = matches!(action, HarnessAdvanceAction::Block { .. });
        let applied =
            harness_service::advance(&root, feature_id, revision, action).map_err(harness_err)?;
        if !applied.applied {
            tracing::info!(
                feature_id,
                ?revision,
                "harness decision ignored: the feature moved on since it was read"
            );
            return Ok(Response::HarnessFeature(applied.feature));
        }
        // A block is a decision about the feature, and the attempt it settled
        // may still have a process behind it. Left running, that process
        // would reach the reaper with an exit code and try to settle a step
        // nobody is waiting on. A cancel is not a failure, so the row keeps
        // the `blocked` the human just wrote.
        if blocking {
            if let Some(job) = self.live_job_for_feature(feature_id) {
                let _ = self.cancel_job(job);
            }
        }
        if let Some(step) = applied.next_step {
            self.run_harness_step(project_id, feature_id, step)?;
        }
        let feature = harness_service::get_feature(&root, feature_id).map_err(harness_err)?;
        Ok(Response::HarnessFeature(feature))
    }

    pub(crate) fn link_harness_session(
        &self,
        project_id: domain::ProjectId,
        feature_id: u32,
        session_id: domain::SessionId,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        harness_service::link_orchestrator(&root, feature_id, session_id).map_err(harness_err)?;
        Ok(Response::Ack)
    }

    pub(crate) fn validate_harness(
        &self,
        project_id: domain::ProjectId,
    ) -> Result<Response, ProtocolError> {
        let root = self.harness_root_for(project_id)?;
        match harness_service::run_validate(&root) {
            Ok(output) => Ok(Response::HarnessValidate { ok: true, output }),
            Err(HarnessError::Invalid(output)) => {
                Ok(Response::HarnessValidate { ok: false, output })
            }
            Err(e) => Err(harness_err(e)),
        }
    }
}
