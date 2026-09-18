use client::Client;
use domain::{JuvaKind, ProjectId, SessionId, WorkspaceId};
use serde::Serialize;
use tauri::{AppHandle, Emitter as _};

use super::{emit, fail, fail_session, Failure};

pub(super) fn load_diff(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    context_lines: Option<u32>,
) {
    match client.workspace_diff(workspace, context_lines) {
        Ok(diff) => emit(app, "workbench:diff", &(workspace, diff)),
        Err(error) => fail(app, "workbench:diff_failed", Some(workspace), &error),
    }
}

pub(super) fn load_session_changes(app: &AppHandle, client: &Client, session: SessionId) {
    match client.session_changes(session) {
        Ok(changes) => emit(app, "workbench:session_changes", &(session, changes)),
        // Keyed on the session, not a workspace: the split lives beside
        // one terminal and has no other place to show a failure.
        Err(error) => fail_session(app, "workbench:session_changes_failed", session, &error),
    }
}

pub(super) fn load_workspace_review(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    context_lines: Option<u32>,
) {
    match client.workspace_review(workspace, context_lines) {
        Ok(review) => emit(app, "workbench:review", &(workspace, review)),
        Err(error) => fail(app, "workbench:review_failed", Some(workspace), &error),
    }
}

pub(super) fn load_rebase_state(app: &AppHandle, client: &Client, workspace: WorkspaceId) {
    rebase(app, client.rebase_state(workspace), workspace)
}

pub(super) fn continue_rebase(app: &AppHandle, client: &Client, workspace: WorkspaceId) {
    // Git refusing to move while paths are unmerged comes back as a state,
    // not an error: the panel redraws with the same conflicts.
    rebase(app, client.continue_rebase(workspace), workspace)
}

pub(super) fn abort_rebase(app: &AppHandle, client: &Client, workspace: WorkspaceId) {
    // An abort answers `Ack`; the panel needs the state it left behind, so
    // the read follows it here.
    rebase(
        app,
        client
            .abort_rebase(workspace)
            .and_then(|()| client.rebase_state(workspace)),
        workspace,
    )
}

pub(super) fn mark_conflict_resolved(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    paths: Vec<String>,
) {
    // Staging a path is what takes it off the conflict list, and the
    // daemon answers with the state that leaves — so nothing is cached and
    // nothing is re-read.
    rebase(
        app,
        client.mark_conflict_resolved(workspace, paths),
        workspace,
    )
}

pub(super) fn list_branches(app: &AppHandle, client: &Client, project: ProjectId) {
    match client.list_branches(project) {
        Ok(branches) => emit(
            app,
            "workbench:branches",
            &BranchesPayload {
                project,
                branches: branches.branches,
                remotes: branches.remotes,
                default_branch: branches.default_branch,
            },
        ),
        Err(error) => fail(app, "workbench:branches_failed", None, &error),
    }
}

pub(super) fn refresh_pull_requests(app: &AppHandle, client: &Client) {
    // Acks when the work starts; the answer arrives as
    // `PullRequestsUpdated` on the event stream like any other change.
    if let Err(error) = client.refresh_pull_requests() {
        fail(app, "workbench:pull_requests_failed", None, &error);
    }
}

pub(super) fn draft_with_juva(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    kind: JuvaKind,
) {
    // Acks when the work starts; the draft arrives on the runtime thread
    // as `JuvaDraftReady`, like every other request that opens a socket.
    if let Err(error) = client.draft_with_juva(workspace, kind) {
        fail(app, "workbench:juva_failed", Some(workspace), &error);
    }
}

pub(super) fn apply_juva_draft(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    kind: JuvaKind,
    title: String,
    body: String,
) {
    match kind {
        JuvaKind::CommitMessage => {
            let message = if body.trim().is_empty() {
                title
            } else {
                format!("{title}\n\n{body}")
            };
            match client.create_commit(workspace, &message) {
                Ok(()) => emit(app, "workbench:juva_applied", &workspace),
                Err(error) => fail(app, "workbench:juva_failed", Some(workspace), &error),
            }
        }
        JuvaKind::PullRequest => match client.create_pull_request(workspace, &title, &body, None) {
            Ok(()) => emit(app, "workbench:juva_applied", &workspace),
            Err(error) => fail(app, "workbench:juva_failed", Some(workspace), &error),
        },
        _ => {
            let _ = app.emit(
                "workbench:juva_failed",
                Failure {
                    workspace: Some(workspace),
                    error: "unknown Juva kind".to_string(),
                },
            );
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct BranchesPayload {
    project: ProjectId,
    branches: Vec<domain::BranchRef>,
    remotes: Vec<domain::Remote>,
    default_branch: Option<String>,
}

fn rebase(
    app: &AppHandle,
    result: Result<domain::RebaseState, client::ClientError>,
    workspace: WorkspaceId,
) {
    match result {
        Ok(state) => emit(app, "workbench:rebase", &(workspace, state)),
        Err(error) => fail(app, "workbench:rebase_failed", Some(workspace), &error),
    }
}
