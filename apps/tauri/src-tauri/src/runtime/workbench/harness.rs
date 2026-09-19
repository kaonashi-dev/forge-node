use client::Client;
use domain::{HarnessAdvanceAction, HarnessArtifactKind, HarnessStep, ProjectId, WorkspaceId};
use serde::Serialize;
use tauri::{AppHandle, Emitter as _};

use super::{emit, fail};

pub(super) fn load_harness_features(app: &AppHandle, client: &Client, project: ProjectId) {
    match client.list_harness_features(project) {
        Ok(list) => emit(app, "harness:features", &(project, list)),
        Err(error) => harness_fail(app, "harness:failed", project, None, &error),
    }
}

pub(super) fn load_harness_detail(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    feature: u32,
) {
    // One command, two reads: the tab shows the feature and its timeline
    // together, and a tab that draws its header before its history is a
    // flash of half a panel for no gain.
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

pub(super) fn load_harness_artifact(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    feature: u32,
    artifact: HarnessArtifactKind,
) {
    match client.read_harness_artifact(project, feature, artifact) {
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
    }
}

pub(super) fn harness_advance(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    feature: u32,
    revision: Option<u64>,
    action: HarnessAdvanceAction,
) {
    // Every advance answers with the feature's new state, so the tab
    // redraws from the answer and never from a guess about what the
    // action did.
    match client.harness_advance(project, feature, revision, action) {
        Ok(updated) => emit(app, "harness:advanced", &(project, updated)),
        Err(error) => harness_fail(app, "harness:failed", project, Some(feature), &error),
    }
}

pub(super) fn run_harness_step(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    feature: u32,
    step: HarnessStep,
) {
    // Answers as soon as the job is *accepted*: the daemon runs the step,
    // records the outcome and starts the next one, stopping at the human
    // gate. Everything after this arrives as `JobUpdated`/`JobOutput`.
    match client.run_harness_step(project, feature, step) {
        Ok(job) => emit(app, "harness:step_started", &(project, feature, job)),
        Err(error) => harness_fail(app, "harness:failed", project, Some(feature), &error),
    }
}

pub(super) fn register_harness_feature(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    workspace: Option<WorkspaceId>,
    spec_raw: String,
    title: Option<String>,
) {
    match client.register_harness_feature(project, workspace, spec_raw, title) {
        Ok(feature) => emit(app, "harness:registered", &(project, feature)),
        Err(error) => harness_fail(app, "harness:failed", project, None, &error),
    }
}

pub(super) fn register_harness_from_issue(
    app: &AppHandle,
    client: &Client,
    project: ProjectId,
    workspace: Option<WorkspaceId>,
    issue: u32,
) {
    match client.register_harness_from_issue(project, workspace, issue) {
        Ok(feature) => emit(app, "harness:registered", &(project, feature)),
        Err(error) => harness_fail(app, "harness:failed", project, None, &error),
    }
}

pub(super) fn validate_harness(app: &AppHandle, client: &Client, project: ProjectId) {
    // `ok: false` is an answer, not a failure: the output is the report
    // the user asked for, and it is the interesting case.
    match client.validate_harness(project) {
        Ok((ok, output)) => emit(app, "harness:validated", &(project, ok, output)),
        Err(error) => harness_fail(app, "harness:failed", project, None, &error),
    }
}

pub(super) fn list_jobs(app: &AppHandle, client: &Client) {
    match client.list_jobs() {
        Ok(jobs) => emit(app, "harness:jobs", &jobs),
        Err(error) => fail(app, "harness:jobs_failed", None, &error),
    }
}

/// A harness answer that failed, tagged with the surface that asked so a stale
/// error cannot land on a feature the user has since left.
#[derive(Clone, Debug, Serialize)]
struct HarnessFailure {
    project: ProjectId,
    feature: Option<u32>,
    error: String,
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
