//! The in-process agent registry (§9.3, §13).
//!
//! [`AgentRegistry`] holds the built-in descriptors (each wrapped in a
//! [`DescriptorAdapter`], ADR-007), the user's per-provider executable
//! overrides (`SetProviderExecutable`, §10.2), and the most recent detection
//! results. The daemon's `AgentService` is thin orchestration over it. It is
//! deliberately synchronous: detection runs on the caller's thread (the daemon
//! runs it off the snapshot path, §13.1).

use std::collections::HashMap;
use std::path::PathBuf;

use domain::{
    AgentDescriptor, AgentProviderId, DetectionResult, LaunchAgentRequest, ResolvedEnvironment,
    SpawnSpec,
};

use crate::builtins;
use crate::descriptor::{AgentAdapter, AgentError, DescriptorAdapter};
use crate::detection;

/// Registry of agent providers, their overrides, and cached detection results.
pub struct AgentRegistry {
    adapters: Vec<Box<dyn AgentAdapter>>,
    overrides: HashMap<AgentProviderId, PathBuf>,
    detections: HashMap<AgentProviderId, DetectionResult>,
}

impl AgentRegistry {
    /// Build a registry seeded with the built-in descriptors, each wrapped
    /// in a generic [`DescriptorAdapter`].
    #[must_use]
    pub fn new() -> Self {
        let adapters = builtins::builtins()
            .into_iter()
            .map(|d| Box::new(DescriptorAdapter::new(d)) as Box<dyn AgentAdapter>)
            .collect();
        Self {
            adapters,
            overrides: HashMap::new(),
            detections: HashMap::new(),
        }
    }

    /// The descriptors of every registered provider, in registration order.
    #[must_use]
    pub fn descriptors(&self) -> Vec<&AgentDescriptor> {
        self.adapters.iter().map(|a| a.descriptor()).collect()
    }

    /// Set (`Some`) or clear (`None`) the user's executable override for a
    /// provider (`SetProviderExecutable`, §10.2).
    pub fn set_override(&mut self, id: &AgentProviderId, path: Option<PathBuf>) {
        match path {
            Some(path) => {
                self.overrides.insert(id.clone(), path);
            }
            None => {
                self.overrides.remove(id);
            }
        }
    }

    /// Detect every provider, honoring per-provider overrides, cache the
    /// results, and return them in registration order (§13.1).
    pub fn detect_all(&mut self, env: &ResolvedEnvironment) -> Vec<DetectionResult> {
        let results: Vec<DetectionResult> = self
            .adapters
            .iter()
            .map(|adapter| {
                let descriptor = adapter.descriptor();
                let over = self.overrides.get(&descriptor.id).map(PathBuf::as_path);
                detection::detect(descriptor, env, over)
            })
            .collect();

        for result in &results {
            self.detections
                .insert(result.provider_id.clone(), result.clone());
        }
        results
    }

    /// The descriptor of one provider, by id.
    #[must_use]
    pub fn descriptor(&self, id: &AgentProviderId) -> Option<&AgentDescriptor> {
        self.adapter(id).map(AgentAdapter::descriptor)
    }

    /// The last cached detection result for a provider, if any.
    #[must_use]
    pub fn detection(&self, id: &AgentProviderId) -> Option<&DetectionResult> {
        self.detections.get(id)
    }

    /// Build a launch spec for a provider, routing through its adapter (§13.3).
    ///
    /// A stored per-provider override is applied when the request does not
    /// already carry its own `executable_override`.
    ///
    /// # Errors
    /// - [`AgentError::UnknownProvider`] if `req.provider_id` is not registered.
    /// - Whatever the adapter's `build_launch` returns (e.g.
    ///   [`AgentError::NotInstalled`]).
    pub fn build_launch(
        &self,
        req: &LaunchAgentRequest,
        env: &ResolvedEnvironment,
    ) -> Result<SpawnSpec, AgentError> {
        self.build_launch_with_config_dir(req, env, None)
    }

    /// [`Self::build_launch`] pointed at a profile's config directory (§13.4).
    ///
    /// # Errors
    /// As [`Self::build_launch`].
    pub fn build_launch_with_config_dir(
        &self,
        req: &LaunchAgentRequest,
        env: &ResolvedEnvironment,
        config_dir: Option<&std::path::Path>,
    ) -> Result<SpawnSpec, AgentError> {
        let adapter = self
            .adapter(&req.provider_id)
            .ok_or_else(|| AgentError::UnknownProvider(req.provider_id.clone()))?;

        // Fold a stored override into the request when it has none of its own.
        if req.executable_override.is_none() {
            if let Some(over) = self.overrides.get(&req.provider_id) {
                let effective = LaunchAgentRequest {
                    executable_override: Some(over.clone()),
                    ..req.clone()
                };
                return adapter.build_launch_with_config_dir(&effective, env, config_dir);
            }
        }
        adapter.build_launch_with_config_dir(req, env, config_dir)
    }

    fn adapter(&self, id: &AgentProviderId) -> Option<&dyn AgentAdapter> {
        self.adapters
            .iter()
            .find(|a| &a.descriptor().id == id)
            .map(|a| &**a)
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{echo_script, env_with_path, write_script};
    use domain::DetectionStatus;

    #[test]
    fn seeds_the_builtins_in_order() {
        let reg = AgentRegistry::new();
        let ids: Vec<String> = reg.descriptors().iter().map(|d| d.id.to_string()).collect();
        assert_eq!(ids, ["claude", "codex", "opencode", "cursor", "grok"]);
    }

    #[test]
    fn detect_all_caches_results_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("Claude Code 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut reg = AgentRegistry::new();
        let results = reg.detect_all(&env);
        assert_eq!(results.len(), 5);

        let claude = AgentProviderId::new("claude");
        assert!(reg.detection(&claude).unwrap().status.is_installed());

        let codex = AgentProviderId::new("codex");
        assert!(matches!(
            reg.detection(&codex).unwrap().status,
            DetectionStatus::NotFound
        ));
    }

    #[test]
    fn build_launch_routes_and_applies_stored_override() {
        let dir = tempfile::tempdir().unwrap();
        let over = write_script(dir.path(), "codex-custom", &echo_script("x"));
        // Empty PATH: only the stored override can supply the program.
        let env = env_with_path(vec![]);

        let mut reg = AgentRegistry::new();
        reg.set_override(&AgentProviderId::new("codex"), Some(over.clone()));

        let req = LaunchAgentRequest {
            provider_id: AgentProviderId::new("codex"),
            cwd: dir.path().to_path_buf(),
            extra_args: Vec::new(),
            executable_override: None,
            resume_session_id: None,
            initial_prompt: None,
            read_only: false,
        };
        let spec = reg.build_launch(&req, &env).unwrap();
        assert_eq!(spec.program, over);

        // Clearing the override removes it again.
        reg.set_override(&AgentProviderId::new("codex"), None);
        assert!(matches!(
            reg.build_launch(&req, &env),
            Err(AgentError::NotInstalled(_))
        ));
    }

    #[test]
    fn build_launch_rejects_unknown_provider() {
        let reg = AgentRegistry::new();
        let env = env_with_path(vec![]);
        let req = LaunchAgentRequest {
            provider_id: AgentProviderId::new("nope"),
            cwd: PathBuf::from("/"),
            extra_args: Vec::new(),
            executable_override: None,
            resume_session_id: None,
            initial_prompt: None,
            read_only: false,
        };
        assert!(matches!(
            reg.build_launch(&req, &env),
            Err(AgentError::UnknownProvider(_))
        ));
    }
}
