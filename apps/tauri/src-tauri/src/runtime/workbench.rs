//! Workbench reads and mutations on a bounded worker separate from terminal input.
//! Correlated results belong to the requesting WebView interaction, never the Store.
//! The shared client serializes socket writes and correlates daemon replies.

use std::sync::Arc;
use std::thread;

use client::Client;
use domain::JuvaKind;
use domain::{
    AgentProfileId, HarnessAdvanceAction, HarnessArtifactKind, HarnessStep, ProjectId, SearchKind,
    SessionId, ShareAction, ShareCandidate, ShareRuleId, ShareStatusEntry, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter as _};

/// A workbench read or write, from the WebView.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkbenchCommand {
    LoadDiff {
        workspace: WorkspaceId,
        context_lines: Option<u32>,
    },
    /// One session's changes since its baseline, for the changes split.
    LoadSessionChanges {
        session: SessionId,
    },
    /// One checkout's changes since its sessions began, for the review tab.
    LoadWorkspaceReview {
        workspace: WorkspaceId,
        context_lines: Option<u32>,
    },
    /// The tail of a session's terminal, for a handoff prompt.
    LoadSessionTranscript {
        session: SessionId,
        max_lines: Option<u32>,
        max_bytes: Option<u32>,
    },
    /// A discovered run's conversation, for a handoff prompt off History.
    LoadExternalTranscript {
        session: String,
        provider: String,
        profile: Option<AgentProfileId>,
        max_turns: Option<u32>,
        max_bytes: Option<u32>,
    },
    /// The two sides of a refused editor save, for the conflict banner.
    LoadEditorConflict {
        session: SessionId,
    },
    /// Take disk: replace the editor's buffer with what is on disk now.
    ReloadEditorBuffer {
        session: SessionId,
    },
    /// Keep mine: write the editor's draft over what is on disk now.
    OverwriteEditorBuffer {
        session: SessionId,
    },
    SetEditorAutosave {
        session: SessionId,
        autosave: bool,
    },
    /// Remove a discovered run's transcript from disk.
    DeleteExternalSession {
        session: String,
        provider: String,
        profile: Option<AgentProfileId>,
    },
    WatchFiles {
        workspace: WorkspaceId,
        directories: Vec<String>,
        generation: u64,
    },
    LoadFileTree {
        workspace: WorkspaceId,
        request_id: String,
    },
    /// Immediate children on disk; an empty path reads the root.
    LoadFileDirectory {
        workspace: WorkspaceId,
        path: String,
        request_id: String,
        generation: u64,
    },
    OpenFile {
        workspace: WorkspaceId,
        path: String,
    },
    /// An image a Markdown preview names.
    LoadImage {
        workspace: WorkspaceId,
        path: String,
    },
    /// Write conditioned on the revision of the last read (ADR-012).
    SaveFile {
        workspace: WorkspaceId,
        path: String,
        text: String,
        revision: String,
    },
    /// Never overwrites an existing entry.
    CreatePath {
        operation_id: String,
        workspace: WorkspaceId,
        path: String,
        directory: bool,
    },
    RenamePath {
        operation_id: String,
        workspace: WorkspaceId,
        from: String,
        to: String,
    },
    DeletePath {
        operation_id: String,
        workspace: WorkspaceId,
        path: String,
    },
    SearchFiles {
        workspace: WorkspaceId,
        query: String,
        /// `name`, `content` or `definition`. Unknown values fail closed.
        kind: String,
        limit: Option<u32>,
    },
    LoadRebaseState {
        workspace: WorkspaceId,
    },
    ContinueRebase {
        workspace: WorkspaceId,
    },
    AbortRebase {
        workspace: WorkspaceId,
    },
    MarkConflictResolved {
        workspace: WorkspaceId,
        /// Several at once: "resolve all" is one gesture in the Git panel.
        paths: Vec<String>,
    },
    ListBranches {
        project: ProjectId,
    },
    /// Ignored paths a project could share between its worktrees (§14.2).
    DetectShareCandidates {
        project: ProjectId,
    },
    /// What applying the project's rules to this workspace would do.
    PreviewShares {
        workspace: WorkspaceId,
    },
    /// Per-rule state of one workspace: applied, missing, severed, diverged.
    LoadShareStatus {
        workspace: WorkspaceId,
    },
    /// Provision a workspace. The daemon acks when the work *starts* and
    /// reports with `SharesApplied`, like a fetch.
    ApplyShares {
        workspace: WorkspaceId,
        #[serde(default)]
        only: Option<Vec<ShareRuleId>>,
    },
    /// Move a real file into the project's shared store, leaving a link.
    AdoptIntoShareStore {
        project: ProjectId,
        path: String,
    },
    /// Copy a store file back into a workspace as a real file.
    MaterializeFromShareStore {
        project: ProjectId,
        path: String,
        #[serde(default)]
        workspace: Option<WorkspaceId>,
    },
    /// Re-read the remote pull-request state. Coalesced by the daemon, which
    /// acks when the work *starts* and reports with `PullRequestsUpdated`.
    RefreshPullRequests,
    LoadUsageAnalytics {
        window_days: Option<u16>,
    },
    DraftWithJuva {
        workspace: WorkspaceId,
        kind: JuvaKind,
    },
    ApplyJuvaDraft {
        workspace: WorkspaceId,
        kind: JuvaKind,
        title: String,
        body: String,
    },

    // ----------------------------------------------------------- harness ---
    //
    // The harness reads answer *inline* — `features.json` and the markdown
    // under `harness/` are files the daemon parses on the spot — so they are
    // exactly the §16.7 shape this worker exists for. Running them where
    // keystrokes run would put a spec parse in front of typing.
    LoadHarnessFeatures {
        project: ProjectId,
    },
    /// The feature and its timeline together: the tab shows both, and asking
    /// twice is two round trips for one open.
    LoadHarnessDetail {
        project: ProjectId,
        feature: u32,
    },
    LoadHarnessArtifact {
        project: ProjectId,
        feature: u32,
        artifact: HarnessArtifactKind,
    },
    /// Approve / revise / block / start a step. Answers with the feature's new
    /// state, which is what the tab redraws from.
    HarnessAdvance {
        project: ProjectId,
        feature: u32,
        /// The row's `revision` as the tab last saw it, so a decision taken
        /// against a view the machine has moved past is a no-op rather than a
        /// second approval.
        #[serde(default)]
        revision: Option<u64>,
        action: HarnessAdvanceAction,
    },
    /// Start one step of the cycle as a headless job.
    RunHarnessStep {
        project: ProjectId,
        feature: u32,
        step: HarnessStep,
    },
    RegisterHarnessFeature {
        project: ProjectId,
        #[serde(default)]
        workspace: Option<WorkspaceId>,
        spec_raw: String,
        #[serde(default)]
        title: Option<String>,
    },
    RegisterHarnessFromIssue {
        project: ProjectId,
        #[serde(default)]
        workspace: Option<WorkspaceId>,
        issue: u32,
    },
    /// Run the harness's own checks and hand back their output verbatim.
    ValidateHarness {
        project: ProjectId,
    },
    /// Every headless run the daemon still knows of.
    ///
    /// Jobs live in the daemon's memory, so this is empty after a restart even
    /// though the transcripts are still on disk — which is why a feature's
    /// timeline carries the job id and this list is only ever a convenience.
    ListJobs,
}

/// A harness answer that failed, tagged with the surface that asked so a stale
/// error cannot land on a feature the user has since left.
#[derive(Clone, Debug, Serialize)]
struct HarnessFailure {
    project: ProjectId,
    feature: Option<u32>,
    error: String,
}

/// Which surface a result belongs to, so a stale answer can be dropped.
#[derive(Clone, Debug, Serialize)]
struct Failure {
    workspace: Option<WorkspaceId>,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PathOperationKind {
    Create,
    Rename,
    Delete,
}

#[derive(Debug, Serialize)]
struct PathResult {
    operation_id: String,
    workspace: WorkspaceId,
    kind: PathOperationKind,
    from: Option<String>,
    to: Option<String>,
    success: bool,
    error: Option<String>,
    uncertain: bool,
}

impl PathResult {
    fn new(
        operation_id: String,
        workspace: WorkspaceId,
        kind: PathOperationKind,
        from: Option<String>,
        to: Option<String>,
        result: Result<(), client::ClientError>,
    ) -> Self {
        // Only an acknowledged result or structured refusal establishes the write's outcome.
        let uncertain =
            matches!(&result, Err(error) if !matches!(error, client::ClientError::Protocol(_)));
        Self {
            operation_id,
            workspace,
            kind,
            from,
            to,
            success: result.is_ok(),
            error: result.err().map(|error| error.to_string()),
            uncertain,
        }
    }
}

#[derive(Debug, Serialize)]
struct DirectoryRequest {
    workspace: WorkspaceId,
    path: String,
    request_id: String,
    generation: u64,
}

#[derive(Debug, Serialize)]
struct IndexPayload {
    workspace: WorkspaceId,
    request_id: String,
    tree: domain::FileTree,
}

#[derive(Debug, Serialize)]
struct IndexFailure {
    workspace: WorkspaceId,
    request_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct DirectoryPayload {
    #[serde(flatten)]
    request: DirectoryRequest,
    entries: Vec<domain::FileEntry>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct DirectoryFailure {
    #[serde(flatten)]
    request: DirectoryRequest,
    message: String,
}

#[derive(Debug, Serialize)]
struct WatchReady {
    workspace: WorkspaceId,
    generation: u64,
}

#[derive(Debug, Serialize)]
struct WatchFailure {
    workspace: WorkspaceId,
    generation: u64,
    error: String,
}

/// A preview asks for several images at once, so a failure names its path.
#[derive(Clone, Debug, Serialize)]
struct ImageFailure {
    workspace: WorkspaceId,
    path: String,
    error: String,
}

#[derive(Clone, Debug, Serialize)]
struct SessionFailure {
    session: SessionId,
    error: String,
}

/// A discovered run's id is the provider's own string, not a [`SessionId`], so
/// [`SessionFailure`] cannot carry it — and the History panel needs the failure
/// on the right card.
#[derive(Clone, Debug, Serialize)]
struct ExternalFailure {
    session: String,
    error: String,
}

#[derive(Clone, Debug, Serialize)]
struct BranchesPayload {
    project: ProjectId,
    branches: Vec<domain::BranchRef>,
    remotes: Vec<domain::Remote>,
    default_branch: Option<String>,
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
        WorkbenchCommand::LoadDiff {
            workspace,
            context_lines,
        } => match client.workspace_diff(workspace, context_lines) {
            Ok(diff) => emit(app, "workbench:diff", &(workspace, diff)),
            Err(error) => fail(app, "workbench:diff_failed", Some(workspace), &error),
        },
        WorkbenchCommand::LoadEditorConflict { session } => match client.editor_conflict(session) {
            Ok((path, disk, mine)) => emit(
                app,
                "workbench:editor_conflict",
                &(session, path, disk, mine),
            ),
            Err(error) => fail_session(app, "workbench:editor_conflict_failed", session, &error),
        },
        WorkbenchCommand::ReloadEditorBuffer { session } => {
            if let Err(error) = client.reload_editor_buffer(session) {
                fail_session(app, "workbench:editor_conflict_failed", session, &error);
            }
        }
        WorkbenchCommand::OverwriteEditorBuffer { session } => {
            if let Err(error) = client.overwrite_editor_buffer(session) {
                fail_session(app, "workbench:editor_conflict_failed", session, &error);
            }
        }
        WorkbenchCommand::SetEditorAutosave { session, autosave } => {
            if let Err(error) = client.set_editor_autosave(session, autosave) {
                fail_session(app, "workbench:editor_conflict_failed", session, &error);
            }
        }
        WorkbenchCommand::LoadSessionChanges { session } => {
            match client.session_changes(session) {
                Ok(changes) => emit(app, "workbench:session_changes", &(session, changes)),
                // Keyed on the session, not a workspace: the split lives beside
                // one terminal and has no other place to show a failure.
                Err(error) => {
                    fail_session(app, "workbench:session_changes_failed", session, &error)
                }
            }
        }
        WorkbenchCommand::LoadWorkspaceReview {
            workspace,
            context_lines,
        } => match client.workspace_review(workspace, context_lines) {
            Ok(review) => emit(app, "workbench:review", &(workspace, review)),
            Err(error) => fail(app, "workbench:review_failed", Some(workspace), &error),
        },
        WorkbenchCommand::LoadSessionTranscript {
            session,
            max_lines,
            max_bytes,
        } => match client.session_transcript(session, max_lines, max_bytes) {
            Ok(transcript) => emit(app, "workbench:session_transcript", &(session, transcript)),
            Err(error) => fail_session(app, "workbench:session_transcript_failed", session, &error),
        },
        WorkbenchCommand::LoadExternalTranscript {
            session,
            provider,
            profile,
            max_turns,
            max_bytes,
        } => match client.external_transcript(
            session.clone(),
            provider,
            profile,
            max_turns,
            max_bytes,
        ) {
            Ok(transcript) => emit(app, "workbench:external_transcript", &(session, transcript)),
            Err(error) => {
                fail_external(app, "workbench:external_transcript_failed", session, &error)
            }
        },
        WorkbenchCommand::DeleteExternalSession {
            session,
            provider,
            profile,
        } => match client.delete_external_session(session.clone(), provider, profile) {
            Ok(()) => emit(app, "workbench:external_session_deleted", &session),
            Err(error) => fail_external(
                app,
                "workbench:external_session_delete_failed",
                session,
                &error,
            ),
        },
        WorkbenchCommand::WatchFiles {
            workspace,
            directories,
            generation,
        } => match client.watch_files(workspace, directories) {
            Ok(()) => emit(
                app,
                "workbench:watch_ready",
                &WatchReady {
                    workspace,
                    generation,
                },
            ),
            Err(error) => emit(
                app,
                "workbench:watch_failed",
                &WatchFailure {
                    workspace,
                    generation,
                    error: error.to_string(),
                },
            ),
        },
        WorkbenchCommand::LoadFileTree {
            workspace,
            request_id,
        } => match client.list_files(workspace) {
            Ok(tree) => emit(
                app,
                "workbench:file_tree",
                &IndexPayload {
                    workspace,
                    request_id,
                    tree,
                },
            ),
            Err(error) => emit(
                app,
                "workbench:file_tree_failed",
                &IndexFailure {
                    workspace,
                    request_id,
                    error: error.to_string(),
                },
            ),
        },
        WorkbenchCommand::LoadFileDirectory {
            workspace,
            path,
            request_id,
            generation,
        } => {
            let result = client.list_directory(workspace, path.clone());
            let request = DirectoryRequest {
                workspace,
                path,
                request_id,
                generation,
            };
            match result {
                Ok(listing) => emit(
                    app,
                    "workbench:directory",
                    &DirectoryPayload {
                        request,
                        entries: listing.entries,
                        truncated: listing.truncated,
                    },
                ),
                Err(error) => emit(
                    app,
                    "workbench:directory_failed",
                    &DirectoryFailure {
                        request,
                        message: error.to_string(),
                    },
                ),
            }
        }
        WorkbenchCommand::OpenFile { workspace, path } => {
            match client.read_file(workspace, path.clone()) {
                Ok(contents) => emit(app, "workbench:file", &(workspace, contents)),
                Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
            }
        }
        WorkbenchCommand::LoadImage { workspace, path } => {
            match client.read_image(workspace, path.clone()) {
                Ok(image) => emit(app, "workbench:image", &(workspace, image)),
                Err(error) => fail_image(app, workspace, path, &error),
            }
        }
        WorkbenchCommand::SaveFile {
            workspace,
            path,
            text,
            revision,
        } => match client.write_file(workspace, path.clone(), text, revision) {
            // The write answers `Ack`, and the pane needs the new revision to
            // save again — so the re-read follows it here rather than making
            // the WebView ask twice.
            Ok(()) => match client.read_file(workspace, path) {
                Ok(contents) => emit(app, "workbench:file_saved", &(workspace, contents)),
                Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
            },
            // A stale revision comes back as `PreconditionFailed`: an agent
            // wrote the same path. The pane re-reads rather than the error
            // carrying the content.
            Err(error) => fail(app, "workbench:save_failed", Some(workspace), &error),
        },
        WorkbenchCommand::CreatePath {
            operation_id,
            workspace,
            path,
            directory,
        } => {
            let result = client.create_path(workspace, path.clone(), directory);
            emit(
                app,
                "workbench:path_result",
                &PathResult::new(
                    operation_id,
                    workspace,
                    PathOperationKind::Create,
                    None,
                    Some(path),
                    result,
                ),
            );
        }
        WorkbenchCommand::RenamePath {
            operation_id,
            workspace,
            from,
            to,
        } => {
            let result = client.rename_path(workspace, from.clone(), to.clone());
            emit(
                app,
                "workbench:path_result",
                &PathResult::new(
                    operation_id,
                    workspace,
                    PathOperationKind::Rename,
                    Some(from),
                    Some(to),
                    result,
                ),
            );
        }
        WorkbenchCommand::DeletePath {
            operation_id,
            workspace,
            path,
        } => {
            let result = client.delete_path(workspace, path.clone());
            emit(
                app,
                "workbench:path_result",
                &PathResult::new(
                    operation_id,
                    workspace,
                    PathOperationKind::Delete,
                    Some(path),
                    None,
                    result,
                ),
            );
        }
        WorkbenchCommand::SearchFiles {
            workspace,
            query,
            kind,
            limit,
        } => match parse_search_kind(&kind) {
            Ok(kind) => {
                let (ok, failed) = if kind == SearchKind::Name {
                    ("workbench:name_search", "workbench:name_search_failed")
                } else {
                    ("workbench:search", "workbench:search_failed")
                };
                match client.search_files(workspace, query, kind, limit) {
                    Ok(results) => emit(app, ok, &(workspace, results)),
                    Err(error) => fail(app, failed, Some(workspace), &error),
                }
            }
            Err(error) => fail_text(app, "workbench:search_failed", Some(workspace), &error),
        },
        WorkbenchCommand::LoadRebaseState { workspace } => {
            rebase(app, client.rebase_state(workspace), workspace)
        }
        // Git refusing to move while paths are unmerged comes back as a state,
        // not an error: the panel redraws with the same conflicts.
        WorkbenchCommand::ContinueRebase { workspace } => {
            rebase(app, client.continue_rebase(workspace), workspace)
        }
        // An abort answers `Ack`; the panel needs the state it left behind, so
        // the read follows it here.
        WorkbenchCommand::AbortRebase { workspace } => rebase(
            app,
            client
                .abort_rebase(workspace)
                .and_then(|()| client.rebase_state(workspace)),
            workspace,
        ),
        // Staging a path is what takes it off the conflict list, and the
        // daemon answers with the state that leaves — so nothing is cached and
        // nothing is re-read.
        WorkbenchCommand::MarkConflictResolved { workspace, paths } => rebase(
            app,
            client.mark_conflict_resolved(workspace, paths),
            workspace,
        ),
        WorkbenchCommand::ListBranches { project } => match client.list_branches(project) {
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
        },
        WorkbenchCommand::DetectShareCandidates { project } => {
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
        WorkbenchCommand::PreviewShares { workspace } => match client.preview_shares(workspace) {
            Ok(actions) => emit(
                app,
                "workbench:share_plan",
                &PlanPayload {
                    workspace: Some(workspace),
                    actions,
                },
            ),
            Err(error) => fail(app, "workbench:share_plan_failed", Some(workspace), &error),
        },
        WorkbenchCommand::LoadShareStatus { workspace } => match client.share_status(workspace) {
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
        },
        WorkbenchCommand::ApplyShares { workspace, only } => {
            // Acks when the work starts; the answer arrives as `SharesApplied`
            // on the event stream, like a fetch.
            if let Err(error) = client.apply_shares(workspace, only) {
                fail(app, "workbench:share_plan_failed", Some(workspace), &error);
            }
        }
        WorkbenchCommand::AdoptIntoShareStore { project, path } => {
            if let Err(error) = client.adopt_into_share_store(project, path) {
                fail(app, "workbench:share_store_failed", None, &error);
            }
        }
        WorkbenchCommand::MaterializeFromShareStore {
            project,
            path,
            workspace,
        } => {
            if let Err(error) = client.materialize_from_share_store(project, path, workspace) {
                fail(app, "workbench:share_store_failed", workspace, &error);
            }
        }
        WorkbenchCommand::RefreshPullRequests => {
            // Acks when the work starts; the answer arrives as
            // `PullRequestsUpdated` on the event stream like any other change.
            if let Err(error) = client.refresh_pull_requests() {
                fail(app, "workbench:pull_requests_failed", None, &error);
            }
        }
        WorkbenchCommand::LoadUsageAnalytics { window_days } => {
            match client.usage_analytics(window_days) {
                Ok(analytics) => emit(app, "workbench:usage", &analytics),
                Err(error) => fail(app, "workbench:usage_failed", None, &error),
            }
        }
        // Acks when the work starts; the draft arrives on the runtime thread
        // as `JuvaDraftReady`, like every other request that opens a socket.
        WorkbenchCommand::DraftWithJuva { workspace, kind } => {
            if let Err(error) = client.draft_with_juva(workspace, kind) {
                fail(app, "workbench:juva_failed", Some(workspace), &error);
            }
        }
        WorkbenchCommand::ApplyJuvaDraft {
            workspace,
            kind,
            title,
            body,
        } => match kind {
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
            JuvaKind::PullRequest => {
                match client.create_pull_request(workspace, &title, &body, None) {
                    Ok(()) => emit(app, "workbench:juva_applied", &workspace),
                    Err(error) => fail(app, "workbench:juva_failed", Some(workspace), &error),
                }
            }
            _ => {
                let _ = app.emit(
                    "workbench:juva_failed",
                    Failure {
                        workspace: Some(workspace),
                        error: "unknown Juva kind".to_string(),
                    },
                );
            }
        },

        // ----------------------------------------------------------- harness ---
        WorkbenchCommand::LoadHarnessFeatures { project } => {
            match client.list_harness_features(project) {
                Ok(list) => emit(app, "harness:features", &(project, list)),
                Err(error) => harness_fail(app, "harness:failed", project, None, &error),
            }
        }
        // One command, two reads: the tab shows the feature and its timeline
        // together, and a tab that draws its header before its history is a
        // flash of half a panel for no gain.
        WorkbenchCommand::LoadHarnessDetail { project, feature } => {
            match client.get_harness_feature(project, feature) {
                Ok(detail) => match client.get_harness_timeline(project, feature) {
                    Ok(timeline) => {
                        emit(app, "harness:detail", &(project, detail, timeline));
                    }
                    Err(error) => {
                        harness_fail(app, "harness:failed", project, Some(feature), &error);
                    }
                },
                Err(error) => harness_fail(app, "harness:failed", project, Some(feature), &error),
            }
        }
        WorkbenchCommand::LoadHarnessArtifact {
            project,
            feature,
            artifact,
        } => match client.read_harness_artifact(project, feature, artifact) {
            Ok(text) => emit(app, "harness:artifact", &(project, feature, artifact, text)),
            // A missing artefact is the normal state of a feature that has not
            // reached that step yet, so the tab shows the reason in place of
            // the document rather than treating it as a broken panel.
            Err(error) => harness_fail(
                app,
                "harness:artifact_failed",
                project,
                Some(feature),
                &error,
            ),
        },
        // Every advance answers with the feature's new state, so the tab
        // redraws from the answer and never from a guess about what the
        // action did.
        WorkbenchCommand::HarnessAdvance {
            project,
            feature,
            revision,
            action,
        } => match client.harness_advance(project, feature, revision, action) {
            Ok(updated) => emit(app, "harness:advanced", &(project, updated)),
            Err(error) => harness_fail(app, "harness:failed", project, Some(feature), &error),
        },
        // Answers as soon as the job is *accepted*: the daemon runs the step,
        // records the outcome and starts the next one, stopping at the human
        // gate. Everything after this arrives as `JobUpdated`/`JobOutput`.
        WorkbenchCommand::RunHarnessStep {
            project,
            feature,
            step,
        } => match client.run_harness_step(project, feature, step) {
            Ok(job) => emit(app, "harness:step_started", &(project, feature, job)),
            Err(error) => harness_fail(app, "harness:failed", project, Some(feature), &error),
        },
        WorkbenchCommand::RegisterHarnessFeature {
            project,
            workspace,
            spec_raw,
            title,
        } => match client.register_harness_feature(project, workspace, spec_raw, title) {
            Ok(feature) => emit(app, "harness:registered", &(project, feature)),
            Err(error) => harness_fail(app, "harness:failed", project, None, &error),
        },
        WorkbenchCommand::RegisterHarnessFromIssue {
            project,
            workspace,
            issue,
        } => match client.register_harness_from_issue(project, workspace, issue) {
            Ok(feature) => emit(app, "harness:registered", &(project, feature)),
            Err(error) => harness_fail(app, "harness:failed", project, None, &error),
        },
        // `ok: false` is an answer, not a failure: the output is the report
        // the user asked for, and it is the interesting case.
        WorkbenchCommand::ValidateHarness { project } => match client.validate_harness(project) {
            Ok((ok, output)) => emit(app, "harness:validated", &(project, ok, output)),
            Err(error) => harness_fail(app, "harness:failed", project, None, &error),
        },
        WorkbenchCommand::ListJobs => match client.list_jobs() {
            Ok(jobs) => emit(app, "harness:jobs", &jobs),
            Err(error) => fail(app, "harness:jobs_failed", None, &error),
        },
    }
}

fn harness_fail(
    app: &AppHandle,
    event: &str,
    project: ProjectId,
    feature: Option<u32>,
    error: &client::ClientError,
) {
    tracing::warn!(%error, event, "harness read failed");
    let _ = app.emit(
        event,
        HarnessFailure {
            project,
            feature,
            error: error.to_string(),
        },
    );
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

fn parse_search_kind(kind: &str) -> Result<SearchKind, String> {
    if kind.eq_ignore_ascii_case("name") {
        Ok(SearchKind::Name)
    } else if kind.eq_ignore_ascii_case("content") {
        Ok(SearchKind::Content)
    } else if kind.eq_ignore_ascii_case("definition") {
        Ok(SearchKind::Definition)
    } else {
        Err(format!("unknown search kind: {kind}"))
    }
}

fn fail_image(app: &AppHandle, workspace: WorkspaceId, path: String, error: &client::ClientError) {
    tracing::warn!(%error, path, "workbench image read failed");
    let _ = app.emit(
        "workbench:image_failed",
        ImageFailure {
            workspace,
            path,
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

/// The same, keyed by a discovered run's provider-side id.
fn fail_external(app: &AppHandle, event: &str, session: String, error: &client::ClientError) {
    tracing::warn!(%error, event, "external transcript read failed");
    let _ = app.emit(
        event,
        ExternalFailure {
            session,
            error: error.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

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

    #[test]
    fn mutations_require_operation_identity() {
        let workspace = WorkspaceId::new();
        for mut command in [
            json!({"type": "create_path", "workspace": workspace, "path": ".agents", "directory": true}),
            json!({"type": "rename_path", "workspace": workspace, "from": "src", "to": "lib"}),
            json!({"type": "delete_path", "workspace": workspace, "path": "lib"}),
        ] {
            assert!(serde_json::from_value::<WorkbenchCommand>(command.clone()).is_err());
            command["operation_id"] = json!("operation-1");
            let parsed = serde_json::from_value::<WorkbenchCommand>(command).unwrap();
            let operation_id = match parsed {
                WorkbenchCommand::CreatePath { operation_id, .. }
                | WorkbenchCommand::RenamePath { operation_id, .. }
                | WorkbenchCommand::DeletePath { operation_id, .. } => operation_id,
                other => panic!("unexpected command: {other:?}"),
            };
            assert_eq!(operation_id, "operation-1");
        }
    }

    #[test]
    fn acknowledged_mutations_serialize_explicit_outcomes_and_paths() {
        let workspace = WorkspaceId::new();
        for (kind, name, from, to) in [
            (PathOperationKind::Create, "create", None, Some(".agents")),
            (
                PathOperationKind::Rename,
                "rename",
                Some("src"),
                Some("lib"),
            ),
            (PathOperationKind::Delete, "delete", Some("lib"), None),
        ] {
            let payload = PathResult::new(
                "operation-1".into(),
                workspace,
                kind,
                from.map(String::from),
                to.map(String::from),
                Ok(()),
            );
            assert_eq!(
                to_value(payload).unwrap(),
                json!({
                    "operation_id": "operation-1", "workspace": workspace, "kind": name,
                    "from": from, "to": to, "success": true, "error": null, "uncertain": false,
                })
            );
        }
    }

    #[test]
    fn lost_replies_are_uncertain_but_daemon_refusals_are_definite() {
        for (error, uncertain) in [
            (client::ClientError::Disconnected, true),
            (client::ClientError::Timeout, true),
            (
                client::ClientError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
                true,
            ),
            (
                client::ClientError::UnexpectedResponse { expected: "Ack" },
                true,
            ),
            (
                client::ClientError::Protocol(client::ProtocolError::invalid_request(
                    "destination exists",
                )),
                false,
            ),
        ] {
            let message = error.to_string();
            let payload = PathResult::new(
                "operation-2".into(),
                WorkspaceId::new(),
                PathOperationKind::Rename,
                Some("src".into()),
                Some("lib".into()),
                Err(error),
            );
            assert!(!payload.success);
            assert_eq!(payload.error.as_deref(), Some(message.as_str()));
            assert_eq!(payload.uncertain, uncertain);
            assert_eq!(payload.operation_id, "operation-2");
        }
    }

    #[test]
    fn directory_results_preserve_root_request_identity_on_both_outcomes() {
        let workspace = WorkspaceId::new();
        let command: WorkbenchCommand = serde_json::from_value(json!({
            "type": "load_file_directory", "workspace": workspace, "path": "",
            "request_id": "root-7", "generation": 7,
        }))
        .unwrap();
        let WorkbenchCommand::LoadFileDirectory {
            workspace,
            path,
            request_id,
            generation,
        } = command
        else {
            panic!("expected directory command");
        };
        let request = || DirectoryRequest {
            workspace,
            path: path.clone(),
            request_id: request_id.clone(),
            generation,
        };
        assert_eq!(
            to_value(DirectoryPayload {
                request: request(),
                entries: vec![],
                truncated: true
            })
            .unwrap(),
            json!({
                "workspace": workspace, "path": "", "request_id": "root-7", "generation": 7,
                "entries": [], "truncated": true,
            })
        );
        assert_eq!(
            to_value(DirectoryFailure {
                request: request(),
                message: "permission denied".into()
            })
            .unwrap(),
            json!({
                "workspace": workspace, "path": "", "request_id": "root-7", "generation": 7,
                "message": "permission denied",
            })
        );
    }

    #[test]
    fn watch_results_echo_the_interest_generation_on_both_outcomes() {
        let workspace = WorkspaceId::new();
        let command: WorkbenchCommand = serde_json::from_value(json!({
            "type": "watch_files", "workspace": workspace, "directories": ["", "src"], "generation": 42,
        })).unwrap();
        let WorkbenchCommand::WatchFiles {
            workspace,
            directories,
            generation,
        } = command
        else {
            panic!("expected watch command");
        };
        assert_eq!(directories, ["", "src"]);
        assert_eq!(
            to_value(WatchReady {
                workspace,
                generation
            })
            .unwrap(),
            json!({
                "workspace": workspace, "generation": 42,
            })
        );
        assert_eq!(
            to_value(WatchFailure {
                workspace,
                generation,
                error: "watch failed".into()
            })
            .unwrap(),
            json!({
                "workspace": workspace, "generation": 42, "error": "watch failed",
            })
        );
    }
}
