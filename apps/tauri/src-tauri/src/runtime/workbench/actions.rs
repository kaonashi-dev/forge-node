use client::{ActionLaunch, Client};
use domain::{ActionSuggestion, ProjectActionId, RepoActionsFile, SessionId, WorkspaceId};
use serde::Serialize;
use tauri::AppHandle;

use super::{emit, fail};

#[derive(Serialize)]
struct SuggestionsPayload {
    workspace: WorkspaceId,
    suggestions: Vec<ActionSuggestion>,
    repo_file: Option<RepoActionsFile>,
    truncated: bool,
}

pub(super) fn suggest_actions(app: &AppHandle, client: &Client, workspace: WorkspaceId) {
    match client.suggest_actions(workspace) {
        Ok(answer) => emit(
            app,
            "workbench:action_suggestions",
            &SuggestionsPayload {
                workspace,
                suggestions: answer.suggestions,
                repo_file: answer.repo_file,
                truncated: answer.truncated,
            },
        ),
        Err(error) => fail(
            app,
            "workbench:action_suggestions_failed",
            Some(workspace),
            &error,
        ),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Started,
    Queued,
    AlreadyRunning,
}

#[derive(Serialize)]
struct LaunchPayload {
    workspace: WorkspaceId,
    action: ProjectActionId,
    session: SessionId,
    outcome: Outcome,
}

/// The WebView focuses `session` itself: a launch and an already-running
/// answer both end on that action's terminal.
pub(super) fn run_action(
    app: &AppHandle,
    client: &Client,
    workspace: WorkspaceId,
    action: ProjectActionId,
) {
    let (session, outcome) = match client.run_action(workspace, action) {
        Ok(ActionLaunch::Started { session_id, .. }) => (session_id, Outcome::Started),
        Ok(ActionLaunch::Queued { session_id }) => (session_id, Outcome::Queued),
        Ok(ActionLaunch::AlreadyRunning { session_id }) => (session_id, Outcome::AlreadyRunning),
        Err(error) => {
            fail(app, "workbench:action_failed", Some(workspace), &error);
            return;
        }
    };
    emit(
        app,
        "workbench:action_launched",
        &LaunchPayload {
            workspace,
            action,
            session,
            outcome,
        },
    );
}
