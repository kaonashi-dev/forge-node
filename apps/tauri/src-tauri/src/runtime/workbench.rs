//! Workbench reads, on their own thread.
//!
//! Every request here is one of the *local synchronous* reads §16.7 and
//! ADR-012 describe: `git diff`, a file tree, a file, a search, a stopped
//! rebase. The daemon answers them without acking-then-eventing, which is
//! exactly why they must not run where terminal input runs — a `git diff` of a
//! large checkout is seconds of subprocess, and `AGENTS.md` is explicit that
//! the GUI's command channel "also carries `RuntimeCommand::Input`, so a
//! synchronous network write would freeze typing".
//!
//! Sharing the client across two threads is what it was built for:
//! `Shared.write` is a `Mutex<UnixStream>` documented as serialized across
//! request writers, `pending` correlates by `request_id`, and every waiter has
//! its own channel. Nothing here touches the `Store` — a `WorkspaceDiff`, a
//! `FileTree` and a `RebaseState` are runtime-only, with no column and no
//! `Store` field — so the two threads share no mutable state at all.

use std::sync::Arc;
use std::thread;

use client::Client;
use domain::JuvaKind;
use domain::{
    HarnessAdvanceAction, HarnessArtifactKind, HarnessStep, ProjectId, SearchKind, SessionId,
    ShareAction, ShareCandidate, ShareRuleId, ShareStatusEntry, WorkspaceId,
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
    LoadFileTree {
        workspace: WorkspaceId,
    },
    OpenFile {
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
    /// Make an empty file or a directory (A11). Never overwrites.
    CreatePath {
        workspace: WorkspaceId,
        path: String,
        directory: bool,
    },
    RenamePath {
        workspace: WorkspaceId,
        from: String,
        to: String,
    },
    DeletePath {
        workspace: WorkspaceId,
        path: String,
    },
    SearchFiles {
        workspace: WorkspaceId,
        query: String,
        /// `name`, `content` or `definition`; anything else is a name search.
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

#[derive(Clone, Debug, Serialize)]
struct SessionFailure {
    session: SessionId,
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
        WorkbenchCommand::LoadFileTree { workspace } => match client.list_files(workspace) {
            Ok(tree) => emit(app, "workbench:file_tree", &(workspace, tree)),
            Err(error) => fail(app, "workbench:file_tree_failed", Some(workspace), &error),
        },
        WorkbenchCommand::OpenFile { workspace, path } => {
            match client.read_file(workspace, path.clone()) {
                Ok(contents) => emit(app, "workbench:file", &(workspace, contents)),
                Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
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
        /*
         * The three path mutations, and the re-read that follows each.
         *
         * The tree is a read rather than a subscription (see `core.rs`), so
         * nothing would redraw on its own: the listing is asked for again here
         * and lands on `workbench:file_tree`, which is the same event the
         * panel already reconciles. One round trip from the WebView's point of
         * view rather than two.
         */
        WorkbenchCommand::CreatePath {
            workspace,
            path,
            directory,
        } => match client.create_path(workspace, path, directory) {
            Ok(()) => relist(app, client, workspace),
            Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
        },
        WorkbenchCommand::RenamePath {
            workspace,
            from,
            to,
        } => match client.rename_path(workspace, from, to) {
            Ok(()) => relist(app, client, workspace),
            Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
        },
        WorkbenchCommand::DeletePath { workspace, path } => {
            match client.delete_path(workspace, path) {
                Ok(()) => relist(app, client, workspace),
                Err(error) => fail(app, "workbench:file_failed", Some(workspace), &error),
            }
        }
        WorkbenchCommand::SearchFiles {
            workspace,
            query,
            kind,
            limit,
        } => {
            let kind = if kind.eq_ignore_ascii_case("content") {
                SearchKind::Content
            } else if kind.eq_ignore_ascii_case("definition") {
                SearchKind::Definition
            } else {
                SearchKind::Name
            };
            match client.search_files(workspace, query, kind, limit) {
                Ok(results) => emit(app, "workbench:search", &(workspace, results)),
                Err(error) => fail(app, "workbench:search_failed", Some(workspace), &error),
            }
        }
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

/// Re-read the tree after a mutation, and report a failed read as a failed read.
fn relist(app: &AppHandle, client: &client::Client, workspace: WorkspaceId) {
    match client.list_files(workspace) {
        Ok(tree) => emit(app, "workbench:file_tree", &(workspace, tree)),
        Err(error) => fail(app, "workbench:file_tree_failed", Some(workspace), &error),
    }
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
    tracing::warn!(%error, event, "workbench read failed");
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
