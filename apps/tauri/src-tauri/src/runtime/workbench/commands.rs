use domain::{
    AgentProfileId, HarnessAdvanceAction, HarnessArtifactKind, HarnessStep, JuvaKind, ProjectId,
    SessionId, ShareRuleId, WorkspaceId,
};
use serde::Deserialize;

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
    /// Ignored paths a project could share between its worktrees.
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
    // exactly the shape this worker exists for. Running them where
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

#[cfg(test)]
mod tests {
    use super::WorkbenchCommand;
    use domain::WorkspaceId;
    use serde_json::json;

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
}
