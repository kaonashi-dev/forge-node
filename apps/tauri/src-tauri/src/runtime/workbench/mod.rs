//! Workbench reads and mutations on a bounded worker separate from terminal input.
//! Correlated results belong to the requesting WebView interaction, never the Store.
//! The shared client serializes socket writes and correlates daemon replies.

mod commands;
mod editor;
mod files;
mod git;
mod projects;
mod sessions;
mod usage;

use std::sync::Arc;
use std::thread;

use client::Client;
use domain::{SessionId, WorkspaceId};
use serde::Serialize;
use tauri::{AppHandle, Emitter as _};

pub use commands::WorkbenchCommand;

/// The sender half, held by the shell so a command can reach the worker.
pub type WorkbenchSender = flume::Sender<WorkbenchCommand>;

pub(super) fn enqueue(
    sender: Option<&WorkbenchSender>,
    command: WorkbenchCommand,
) -> Result<(), String> {
    sender
        .ok_or_else(|| "workbench worker is unavailable".to_string())?
        .try_send(command)
        .map_err(|error| match error {
            flume::TrySendError::Full(_) => "workbench command queue is full".to_string(),
            flume::TrySendError::Disconnected(_) => {
                "workbench command queue is disconnected".to_string()
            }
        })
}

/// Start a worker for one connection.
///
/// Dropping the returned sender ends the worker, which is how a reconnect
/// retires the one bound to the old client rather than leaving it writing to a
/// dead socket.
pub fn start(app: AppHandle, client: Arc<Client>) -> WorkbenchSender {
    // Bounded like every other queue that can be produced into faster than it
    // is drained: a user dragging a file tree open can outrun `git`.
    let (tx, rx) = flume::bounded(64);
    thread::Builder::new()
        .name("forge-tauri-workbench".to_string())
        .spawn(move || {
            for command in rx.iter() {
                run(&app, &client, command);
            }
        })
        .expect("failed to start the workbench worker");
    tx
}

fn run(app: &AppHandle, client: &Client, command: WorkbenchCommand) {
    match command {
        WorkbenchCommand::LaunchAgent { request } => sessions::launch_agent(app, client, request),
        WorkbenchCommand::LoadHandoffProgress {
            request_id,
            session,
        } => sessions::load_handoff_progress(app, client, request_id, session),
        WorkbenchCommand::LoadDiff {
            workspace,
            context_lines,
        } => git::load_diff(app, client, workspace, context_lines),
        WorkbenchCommand::LoadSessionChanges { session } => {
            git::load_session_changes(app, client, session)
        }
        WorkbenchCommand::LoadWorkspaceReview {
            workspace,
            context_lines,
        } => git::load_workspace_review(app, client, workspace, context_lines),
        WorkbenchCommand::LoadEditorConflict { session } => {
            editor::load_editor_conflict(app, client, session)
        }
        WorkbenchCommand::ReloadEditorBuffer { session } => {
            editor::reload_editor_buffer(app, client, session)
        }
        WorkbenchCommand::OverwriteEditorBuffer { session } => {
            editor::overwrite_editor_buffer(app, client, session)
        }
        WorkbenchCommand::SetEditorAutosave { session, autosave } => {
            editor::set_editor_autosave(app, client, session, autosave)
        }
        WorkbenchCommand::EditorFind { session, command } => {
            editor::editor_find(client, session, command)
        }
        WorkbenchCommand::LoadSessionTranscript {
            session,
            max_lines,
            max_bytes,
        } => sessions::load_session_transcript(app, client, session, max_lines, max_bytes),
        WorkbenchCommand::LoadExternalTranscript {
            session,
            provider,
            profile,
            max_turns,
            max_bytes,
        } => sessions::load_external_transcript(
            app, client, session, provider, profile, max_turns, max_bytes,
        ),
        WorkbenchCommand::DeleteExternalSession {
            session,
            provider,
            profile,
        } => sessions::delete_external_session(app, client, session, provider, profile),
        WorkbenchCommand::WatchFiles {
            workspace,
            directories,
            generation,
        } => files::watch_files(app, client, workspace, directories, generation),
        WorkbenchCommand::LoadFileTree {
            workspace,
            request_id,
        } => files::load_file_tree(app, client, workspace, request_id),
        WorkbenchCommand::LoadFileDirectory {
            workspace,
            path,
            request_id,
            generation,
        } => files::load_file_directory(app, client, workspace, path, request_id, generation),
        WorkbenchCommand::OpenFile { workspace, path } => {
            files::open_file(app, client, workspace, path)
        }
        WorkbenchCommand::LoadImage { workspace, path } => {
            files::load_image(app, client, workspace, path)
        }
        WorkbenchCommand::SaveFile {
            workspace,
            path,
            text,
            revision,
        } => files::save_file(app, client, workspace, path, text, revision),
        WorkbenchCommand::CreatePath {
            operation_id,
            workspace,
            path,
            directory,
        } => files::create_path(app, client, operation_id, workspace, path, directory),
        WorkbenchCommand::RenamePath {
            operation_id,
            workspace,
            from,
            to,
        } => files::rename_path(app, client, operation_id, workspace, from, to),
        WorkbenchCommand::DeletePath {
            operation_id,
            workspace,
            path,
        } => files::delete_path(app, client, operation_id, workspace, path),
        WorkbenchCommand::SearchFiles {
            workspace,
            query,
            kind,
            limit,
        } => files::search_files(app, client, workspace, query, kind, limit),
        WorkbenchCommand::LoadRebaseState { workspace } => {
            git::load_rebase_state(app, client, workspace)
        }
        WorkbenchCommand::ContinueRebase { workspace } => {
            git::continue_rebase(app, client, workspace)
        }
        WorkbenchCommand::AbortRebase { workspace } => git::abort_rebase(app, client, workspace),
        WorkbenchCommand::MarkConflictResolved { workspace, paths } => {
            git::mark_conflict_resolved(app, client, workspace, paths)
        }
        WorkbenchCommand::ListBranches { project } => git::list_branches(app, client, project),
        WorkbenchCommand::DetectShareCandidates { project } => {
            projects::detect_share_candidates(app, client, project)
        }
        WorkbenchCommand::PreviewShares { workspace } => {
            projects::preview_shares(app, client, workspace)
        }
        WorkbenchCommand::LoadShareStatus { workspace } => {
            projects::load_share_status(app, client, workspace)
        }
        WorkbenchCommand::ApplyShares { workspace, only } => {
            projects::apply_shares(app, client, workspace, only)
        }
        WorkbenchCommand::AdoptIntoShareStore { project, path } => {
            projects::adopt_into_share_store(app, client, project, path)
        }
        WorkbenchCommand::MaterializeFromShareStore {
            project,
            path,
            workspace,
        } => projects::materialize_from_share_store(app, client, project, path, workspace),
        WorkbenchCommand::RefreshPullRequests => git::refresh_pull_requests(app, client),
        WorkbenchCommand::LoadUsageAnalytics { window_days } => {
            usage::load_usage_analytics(app, client, window_days)
        }
        WorkbenchCommand::DraftWithJuva { workspace, kind } => {
            git::draft_with_juva(app, client, workspace, kind)
        }
        WorkbenchCommand::ApplyJuvaDraft {
            workspace,
            kind,
            title,
            body,
        } => git::apply_juva_draft(app, client, workspace, kind, title, body),
    }
}

/// Which surface a result belongs to, so a stale answer can be dropped.
#[derive(Clone, Debug, Serialize)]
struct Failure {
    workspace: Option<WorkspaceId>,
    error: String,
}

#[derive(Clone, Debug, Serialize)]
struct SessionFailure {
    session: SessionId,
    error: String,
}

fn emit<T: Serialize>(app: &AppHandle, event: &str, payload: &T) {
    let _ = app.emit(event, payload);
}

fn fail(app: &AppHandle, event: &str, workspace: Option<WorkspaceId>, error: &client::ClientError) {
    fail_text(app, event, workspace, &error.to_string());
}

fn fail_text(app: &AppHandle, event: &str, workspace: Option<WorkspaceId>, error: &str) {
    tracing::warn!(error, event, "workbench read failed");
    let _ = app.emit(
        event,
        Failure {
            workspace,
            error: error.to_string(),
        },
    );
}

/// A failure the split keys on its session rather than on a checkout.
fn fail_session(app: &AppHandle, event: &str, session: SessionId, error: &client::ClientError) {
    tracing::warn!(%error, event, "workbench session read failed");
    let _ = app.emit(
        event,
        SessionFailure {
            session,
            error: error.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_reports_unavailable_full_and_closed_without_losing_accepted_commands() {
        let command = || WorkbenchCommand::LoadFileTree {
            workspace: WorkspaceId::new(),
            request_id: "index-1".into(),
        };
        assert_eq!(
            enqueue(None, command()),
            Err("workbench worker is unavailable".into())
        );
        let (sender, receiver) = flume::bounded(1);
        assert_eq!(enqueue(Some(&sender), command()), Ok(()));
        assert_eq!(
            enqueue(Some(&sender), command()),
            Err("workbench command queue is full".into())
        );
        assert!(matches!(
            receiver.try_recv(),
            Ok(WorkbenchCommand::LoadFileTree { .. })
        ));
        assert!(receiver.try_recv().is_err());
        assert_eq!(enqueue(Some(&sender), command()), Ok(()));
        drop(receiver);
        assert_eq!(
            enqueue(Some(&sender), command()),
            Err("workbench command queue is disconnected".into())
        );
    }
}
