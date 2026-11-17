//! Fixtures shared by tests across modules.

use std::path::PathBuf;

use client::ProviderInfo;
use domain::{
    AgentCapabilities, AgentDescriptor, AgentProviderId, DetectionResult, DetectionStatus,
    VersionProbe,
};

/// A provider as the daemon reports it, `installed` or not, taking a first
/// prompt and declaring a read-only review mode as asked.
pub fn provider(id: &str, installed: bool, prompt: bool, review: bool) -> ProviderInfo {
    let id = AgentProviderId::new(id);
    ProviderInfo {
        descriptor: AgentDescriptor {
            id: id.clone(),
            display_name: id.to_string(),
            binary_candidates: vec![id.to_string()],
            default_args: vec![],
            version_probe: VersionProbe {
                args: vec!["--version".to_string()],
                expect_substring: None,
                timeout_ms: 2000,
            },
            usage_source: None,
            config_dir: None,
            model: None,
            effort: None,
            resume: None,
            prompt: None,
            review: review.then(|| domain::ReviewStyle {
                args: vec!["--permission-mode".to_string(), "plan".to_string()],
                label: "plan mode".to_string(),
            }),
            structured_review: None,
            capabilities: AgentCapabilities {
                interactive_tui: true,
                supports_initial_prompt: prompt,
                supports_resume: false,
                supports_review: review,
            },
        },
        detection: DetectionResult {
            provider_id: id,
            status: if installed {
                DetectionStatus::Installed {
                    executable: PathBuf::from("/usr/local/bin/agent"),
                    version: Some("1.0.0".to_string()),
                }
            } else {
                DetectionStatus::NotFound
            },
            checked_at: domain::Timestamp::now(),
        },
    }
}

/// A spec run at `stage`, updated `updated_at`, in a worktree on `spec/x`.
pub fn spec_run(
    stage: domain::spec_run::model::SpecStage,
    updated_at: domain::Timestamp,
) -> domain::spec_run::model::SpecRunView {
    let stage = serde_json::to_value(stage).expect("a stage serializes");
    serde_json::from_value(serde_json::json!({
        "id": domain::SpecRunId::new(),
        "project_id": domain::ProjectId::new(),
        "workspace_id": domain::WorkspaceId::new(),
        "name": "auth-flow",
        "stage": stage,
        "source": {"kind": "inline"},
        "planner": {"provider": "claude"},
        "coder": {"provider": "claude"},
        "options": {"placement": {"kind": "worktree", "branch": "spec/x"}, "rounds": 3},
        "created_at": updated_at,
        "updated_at": updated_at,
    }))
    .expect("a spec run decodes")
}
