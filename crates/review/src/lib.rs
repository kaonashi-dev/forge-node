//! Forge Review's pure engine: everything between a diff and a validated,
//! scored review that does not touch a disk, a process or a clock.

mod config;
mod lineage;
mod output;
mod pack;
mod presets;
mod rules;
mod suppress;
mod validate;

pub use config::{Categories, Config, ContextSettings, Strictness, CATEGORIES};
pub use lineage::{mark_lineage, resolved, same_lineage};
pub use output::{parse_output, RawFinding, ReviewOutput, SCHEMA};
pub use pack::{build_pack, prompt, redact, PackMeta, TierCaps};
pub use presets::{
    builtins, default_preset, parse_presets, resolve, Preset, Resolved, CUSTOM_PRESET,
    DEFAULT_PRESET, MAX_INSTRUCTIONS_BYTES,
};
pub use rules::{in_scope, parse_rules, Rule};
pub use suppress::{suppressions, Feedback};
pub use validate::{
    fingerprint, hunks, score, validate, verdict, ChangedFile, Validated, MAX_FINDINGS,
    MAX_SUGGESTION_BYTES, MAX_TEXT_BYTES,
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
