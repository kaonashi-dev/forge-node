//! Forge Review's pure engine: everything between a diff and a validated,
//! scored review that does not touch a disk, a process or a clock.

mod blocks;
mod config;
mod lineage;
mod output;
mod pack;
mod presets;
mod rules;
mod scope;
mod suppress;
mod validate;

pub use blocks::{blocks, Extras, GraphFunction, RuntimeResult};
pub use config::{
    AutoApprove, Categories, Config, ContextSettings, GraphSettings, RiskCeiling, Strictness,
    CATEGORIES,
};
pub use lineage::{mark_lineage, resolved, same_lineage};
pub use output::{parse_output, RawFinding, ReviewOutput, SCHEMA, SCHEMA_V2};
pub use pack::{build_pack, excluded_path, prompt, redact, PackMeta, TierCaps};
pub use presets::{
    auto_tier, builtins, default_preset, parse_presets, resolve, Preset, Resolved, CUSTOM_PRESET,
    DEFAULT_PRESET, MAX_INSTRUCTIONS_BYTES,
};
pub use rules::{in_scope, parse_rules, Rule};
pub use scope::{acknowledged_scope, MAX_SCOPE_BYTES};
pub use suppress::{suppressions, Feedback};
pub use validate::{
    auto_approvable, fingerprint, hunks, score, validate, verdict, ChangedFile, Validated,
    MAX_FINDINGS, MAX_SUGGESTION_BYTES, MAX_TEXT_BYTES,
};

/// The longest prefix of `text` that fits in `max` bytes without splitting a char.
fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests;
