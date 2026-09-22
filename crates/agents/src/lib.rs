//! Provider registry, descriptors, detection, and launch-spec construction.
//!
//! All provider-specific knowledge lives here (P2, ADR-007). No other crate
//! branches on a provider id. Descriptors are static data; [`DescriptorAdapter`]
//! wraps one with no extra behavior.

pub mod attention;
pub mod builtins;
pub mod descriptor;
pub mod detection;
pub mod history;
pub mod registry;
pub mod usage;

#[cfg(test)]
pub(crate) mod test_support;

pub use attention::{
    inject_attention, inject_attention_plugin, install_assets, install_plugin, AttentionAssets,
    PLUGIN_FILENAME,
};
pub use builtins::{builtin, builtins};
pub use descriptor::{
    build_launch, build_launch_with_config_dir, ensure_config_dir, env_for_config_dir,
    resolve_executable, AgentAdapter, AgentError, DescriptorAdapter,
};
pub use detection::detect;
pub use registry::AgentRegistry;
pub use usage::{collect as collect_usage, collect_analytics, UsageAccount};
