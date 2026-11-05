//! The palette's spec run: the same brief `forgectl spec run` composes, launched as a planner session.

use std::path::{Path, PathBuf};

use client::Store;
use domain::spec_run::{
    agent_refusal, classify, compose_planner_prompt, planner_effort, Coder, Preset, SpecRun,
    SpecSource, MAX_INLINE_SPEC_BYTES,
};
use domain::{AgentDescriptor, AgentProviderId, Workspace};
use serde::Deserialize;

const PLANNER: &str = "claude";
const DEFAULT_ROUNDS: u32 = 3;
const MAX_ROUNDS: u32 = 10;

#[derive(Clone, Debug, Deserialize)]
pub struct SpecRequest {
    /// A path (relative to the checkout or `~/`), a link, or the spec's text.
    pub spec: String,
    #[serde(default)]
    pub preset: Preset,
    /// Replaces the preset's coder and its effort.
    #[serde(default)]
    pub coder: Option<String>,
    /// A new worktree on this branch for the coder, instead of the planner's checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub rounds: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlannerLaunch {
    pub provider: AgentProviderId,
    pub effort: Option<String>,
    pub title: String,
    pub brief: String,
}

pub fn planner_launch(
    store: &Store,
    workspace: &Workspace,
    request: SpecRequest,
    home: Option<&Path>,
    forgectl: PathBuf,
) -> Result<PlannerLaunch, String> {
    let resolve = |raw: &Path| -> PathBuf {
        match (raw.strip_prefix("~"), home) {
            (Ok(rest), Some(home)) => home.join(rest),
            _ => workspace.path.join(raw),
        }
    };
    let source = match classify(&request.spec, |path| resolve(path).is_file())
        .map_err(|error| error.to_string())?
    {
        SpecSource::File(path) => {
            let path = resolve(&path);
            SpecSource::File(std::fs::canonicalize(&path).unwrap_or(path))
        }
        SpecSource::Inline(text) if text.len() > MAX_INLINE_SPEC_BYTES => {
            return Err(format!(
                "a pasted spec is capped at {} KiB; save it to a file",
                MAX_INLINE_SPEC_BYTES / 1024
            ))
        }
        other => other,
    };
    let (provider, effort) = match request.coder.filter(|coder| !coder.trim().is_empty()) {
        Some(coder) => (coder, None),
        None => {
            let (provider, effort) = request.preset.coder();
            (provider.to_owned(), effort.map(str::to_owned))
        }
    };
    check(store, &provider, effort.as_deref(), "coder")?;
    let planner = descriptor(store, PLANNER, "planner")?;
    let planner_effort = planner_effort(planner);
    check(store, PLANNER, planner_effort.as_deref(), "planner")?;
    let name = match &source {
        SpecSource::File(path) => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "spec".to_owned()),
        _ => "spec".to_owned(),
    };
    let run = SpecRun {
        source,
        coder: Coder {
            provider,
            effort,
            model: None,
        },
        forgectl,
        name: name.clone(),
        worktree: request
            .worktree
            .map(|branch| branch.trim().to_owned())
            .filter(|branch| !branch.is_empty()),
        base: None,
        rounds: request
            .rounds
            .unwrap_or(DEFAULT_ROUNDS)
            .clamp(1, MAX_ROUNDS),
        planner_is_session: true,
    };
    Ok(PlannerLaunch {
        provider: planner.id.clone(),
        effort: planner_effort,
        title: format!("{name} planner"),
        brief: compose_planner_prompt(&run),
    })
}

fn descriptor<'a>(store: &'a Store, id: &str, role: &str) -> Result<&'a AgentDescriptor, String> {
    store
        .providers
        .iter()
        .find(|info| info.descriptor.id.as_str() == id)
        .map(|info| &info.descriptor)
        .ok_or_else(|| format!("the {role} provider {id} is not available"))
}

fn check(store: &Store, id: &str, effort: Option<&str>, role: &str) -> Result<(), String> {
    match agent_refusal(descriptor(store, id, role)?, effort, None) {
        Some(reason) => Err(format!("the {role} provider {id} {reason}")),
        None => Ok(()),
    }
}

/// The `forgectl` packaged beside this binary; a bare name relies on the session's `PATH`.
pub fn forgectl() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("forgectl"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("forgectl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{DetectionResult, DetectionStatus, ProjectId, Timestamp, WorkspaceId};

    fn store() -> Store {
        let mut store = Store::default();
        store.providers = ["claude", "cursor", "opencode"]
            .iter()
            .map(|id| client::ProviderInfo {
                descriptor: agents::builtins::builtin(id).expect("builtin"),
                detection: DetectionResult {
                    provider_id: AgentProviderId::new(*id),
                    status: DetectionStatus::NotFound,
                    checked_at: Timestamp::now(),
                },
            })
            .collect();
        store
    }

    fn checkout(path: &Path) -> Workspace {
        Workspace {
            id: WorkspaceId::new(),
            project_id: ProjectId::new(),
            kind: domain::WorkspaceKind::Main,
            path: path.to_path_buf(),
            branch: None,
            display_name: None,
            managed_by_app: false,
            created_at: Timestamp::now(),
            status: domain::WorkspaceStatus::default(),
        }
    }

    fn request(spec: &str) -> SpecRequest {
        SpecRequest {
            spec: spec.to_owned(),
            preset: Preset::Claude,
            coder: None,
            worktree: None,
            rounds: None,
        }
    }

    #[test]
    fn a_relative_spec_path_is_read_from_the_checkout_and_names_the_planner() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("docs")).expect("docs");
        std::fs::write(dir.path().join("docs/auth-flow.md"), "# Auth").expect("spec");
        let launch = planner_launch(
            &store(),
            &checkout(dir.path()),
            request("docs/auth-flow.md"),
            None,
            PathBuf::from("forgectl"),
        )
        .expect("launch");
        assert_eq!(launch.provider.as_str(), "claude");
        assert_eq!(launch.effort.as_deref(), Some("high"));
        assert_eq!(launch.title, "auth-flow planner");
        let canonical = std::fs::canonicalize(dir.path().join("docs/auth-flow.md")).expect("path");
        assert!(launch.brief.contains(&canonical.display().to_string()));
        assert!(launch
            .brief
            .contains("--provider claude --role executor --effort medium"));
        assert!(launch.brief.contains("--parent self"));
    }

    #[test]
    fn the_mix_preset_codes_with_cursor_and_a_named_coder_replaces_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut mix = request("Add a health endpoint");
        mix.preset = Preset::Mix;
        let brief = planner_launch(
            &store(),
            &checkout(dir.path()),
            mix,
            None,
            "forgectl".into(),
        )
        .expect("launch")
        .brief;
        assert!(brief.contains("--provider cursor --role executor --parent self"));

        let mut named = request("Add a health endpoint");
        named.coder = Some("missing".into());
        let refused = planner_launch(
            &store(),
            &checkout(dir.path()),
            named,
            None,
            "forgectl".into(),
        );
        assert_eq!(
            refused,
            Err("the coder provider missing is not available".to_owned())
        );
    }

    #[test]
    fn every_coder_the_dialog_offers_launches_under_the_claude_planner() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (payload, spawn) in [
            (
                r#"{"spec":"Add a health endpoint","preset":"claude","coder":null}"#,
                "--provider claude --role executor --effort medium --parent self",
            ),
            (
                r#"{"spec":"Add a health endpoint","preset":"mix","coder":"cursor"}"#,
                "--provider cursor --role executor --parent self",
            ),
            (
                r#"{"spec":"Add a health endpoint","preset":"mix","coder":"opencode"}"#,
                "--provider opencode --role executor --parent self",
            ),
        ] {
            let request: SpecRequest = serde_json::from_str(payload).expect("payload");
            let launch = planner_launch(
                &store(),
                &checkout(dir.path()),
                request,
                None,
                "forgectl".into(),
            )
            .expect(payload);
            assert_eq!(launch.provider.as_str(), "claude");
            assert_eq!(launch.effort.as_deref(), Some("high"));
            assert!(launch.brief.contains(spawn), "{payload}");
        }
    }

    #[test]
    fn a_missing_spec_file_is_refused_rather_than_sent_as_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let refused = planner_launch(
            &store(),
            &checkout(dir.path()),
            request("docs/missing.md"),
            None,
            "forgectl".into(),
        );
        assert_eq!(
            refused,
            Err("spec file not found: docs/missing.md".to_owned())
        );
    }
}
