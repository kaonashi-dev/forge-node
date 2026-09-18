use client::Client;
use domain::{ProjectId, ShareAction, ShareCandidate, ShareRuleId, ShareStatusEntry, WorkspaceId};
use serde::Serialize;
use tauri::AppHandle;

use super::{emit, fail};

pub(super) fn detect_share_candidates(app: &AppHandle, client: &Client, project: ProjectId) {
    match client.detect_share_candidates(project) {
        Ok((candidates, truncated)) => emit(
            app,
            "workbench:share_candidates",
            &CandidatesPayload {
                project,
                candidates,
                truncated,
            },
        ),
        Err(error) => fail(app, "workbench:share_candidates_failed", None, &error),
    }
}

pub(super) fn preview_shares(app: &AppHandle, client: &Client, workspace: WorkspaceId) {
    match client.preview_shares(workspace) {
        Ok(actions) => emit(
            app,
            "workbench:share_plan",
            &PlanPayload {
                workspace: Some(workspace),
                actions,
            },
        ),
        Err(error) => fail(app, "workbench:share_plan_failed", Some(workspace), &error),
    }
}

pub(super) fn load_share_status(app: &AppHandle, client: &Client, workspace: WorkspaceId) {
    match client.share_status(workspace) {
        Ok(entries) => emit(
            app,
            "workbench:share_status",
            &StatusPayload { workspace, entries },
        ),
        Err(error) => fail(
            app,
            "workbench:share_status_failed",
            Some(workspace),
            &error,
        ),
    }
}

pub(super) fn apply_shares(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    only: Option<Vec<ShareRuleId>>,
) {
    // Acks when the work starts; the answer arrives as `SharesApplied`
    // on the event stream, like a fetch.
    if let Err(error) = client.apply_shares(workspace, only) {
        fail(app, "workbench:share_plan_failed", Some(workspace), &error);
    }
}

pub(super) fn adopt_into_share_store(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    path: String,
) {
    if let Err(error) = client.adopt_into_share_store(project, path) {
        fail(app, "workbench:share_store_failed", None, &error);
    }
}

pub(super) fn materialize_from_share_store(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    path: String,
    workspace: Option<WorkspaceId>,
) {
    if let Err(error) = client.materialize_from_share_store(project, path, workspace) {
        fail(app, "workbench:share_store_failed", workspace, &error);
    }
}

#[derive(Serialize)]
struct CandidatesPayload {
    project: ProjectId,
    candidates: Vec<ShareCandidate>,
    truncated: bool,
}

#[derive(Serialize)]
struct PlanPayload {
    workspace: Option<WorkspaceId>,
    actions: Vec<ShareAction>,
}

#[derive(Serialize)]
struct StatusPayload {
    workspace: WorkspaceId,
    entries: Vec<ShareStatusEntry>,
}
