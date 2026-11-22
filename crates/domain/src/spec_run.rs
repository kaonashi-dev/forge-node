//! The planner brief of a spec run. Forge never fetches the spec: an artifact needs the reader's login.

use std::path::{Path, PathBuf};

use crate::orchestration::ToolSurface;
use crate::{AgentDescriptor, EffortSpelling};

mod brief;
pub mod draft;
pub mod model;
pub mod principles;
pub mod schema;
pub mod stages;

pub use brief::{
    coder_rules, compose_drafter_prompt, compose_planner_prompt, guide, resolve_spec_section,
    DrafterInput, ReportTo, SpecFor, Tools, MAX_STAGE_INSTRUCTIONS, REPORT_FORMAT,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecSource {
    ClaudeArtifact(String),
    Link(String),
    File(PathBuf),
    Inline(String),
}

impl SpecSource {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            SpecSource::ClaudeArtifact(_) => "claude_artifact",
            SpecSource::Link(_) => "link",
            SpecSource::File(_) => "file",
            SpecSource::Inline(_) => "inline",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("the spec is empty")]
    Empty,
    #[error("spec file not found: {0}")]
    MissingFile(String),
}

pub(crate) const ARTIFACT_HOSTS: [&str; 2] = ["claude.ai", "claude.site"];
const SPEC_EXTENSIONS: [&str; 4] = ["md", "markdown", "mdx", "txt"];

/// A path-like value that is not a file is an error, never a one-line inline spec.
pub fn classify(raw: &str, is_file: impl Fn(&Path) -> bool) -> Result<SpecSource, SpecError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SpecError::Empty);
    }
    if let Some(host) = link_host(trimmed) {
        let artifact = ARTIFACT_HOSTS
            .iter()
            .any(|known| host == *known || host.ends_with(&format!(".{known}")));
        return Ok(if artifact {
            SpecSource::ClaudeArtifact(trimmed.to_owned())
        } else {
            SpecSource::Link(trimmed.to_owned())
        });
    }
    let path = Path::new(trimmed);
    if is_file(path) {
        return Ok(SpecSource::File(path.to_path_buf()));
    }
    if looks_like_path(trimmed) {
        return Err(SpecError::MissingFile(trimmed.to_owned()));
    }
    Ok(SpecSource::Inline(trimmed.to_owned()))
}

pub(crate) fn link_host(raw: &str) -> Option<String> {
    if raw.chars().any(char::is_whitespace) {
        return None;
    }
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn looks_like_path(raw: &str) -> bool {
    if raw.contains('\n') {
        return false;
    }
    let extension = Path::new(raw)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    ["/", "./", "../", "~/"].iter().any(|p| raw.starts_with(p))
        || extension.is_some_and(|e| SPEC_EXTENSIONS.contains(&e.as_str()))
}

/// Leaves room for the brief inside the 64 KiB launch prompt.
pub const MAX_INLINE_SPEC_BYTES: usize = 56 * 1024;
/// The planner's effort when its provider offers it.
pub const PLANNER_EFFORT: &str = "high";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// A Claude coder at medium effort.
    #[default]
    Claude,
    /// A Cursor coder, unless a coder provider is named.
    Mix,
}

impl Preset {
    #[must_use]
    pub fn coder(self) -> (&'static str, Option<&'static str>) {
        match self {
            Preset::Claude => ("claude", Some("medium")),
            Preset::Mix => ("cursor", None),
        }
    }
}

#[must_use]
pub fn planner_effort(descriptor: &AgentDescriptor) -> Option<String> {
    descriptor
        .effort
        .as_ref()
        .filter(|spec| spec.allowed.iter().any(|level| level == PLANNER_EFFORT))
        .map(|_| PLANNER_EFFORT.to_owned())
}

/// Why this provider cannot take this role's launch, checked before a planner starts rather than
/// by the daemon halfway through a run.
#[must_use]
pub fn agent_refusal(
    descriptor: &AgentDescriptor,
    effort: Option<&str>,
    model: Option<&str>,
) -> Option<String> {
    if !descriptor.capabilities.supports_initial_prompt {
        return Some("cannot take a prompt at launch".to_owned());
    }
    if model.is_some() && descriptor.model.is_none() {
        return Some("does not support model selection".to_owned());
    }
    let effort = effort?;
    match &descriptor.effort {
        None => Some("does not support effort selection".to_owned()),
        Some(spec) if !spec.allowed.iter().any(|level| level == effort) => Some(format!(
            "has no effort {effort}; it offers {}",
            spec.allowed.join(", ")
        )),
        Some(spec) if matches!(spec.spelling, EffortSpelling::ModelSuffix) && model.is_none() => {
            Some("needs a model to apply an effort".to_owned())
        }
        Some(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coder {
    pub provider: String,
    /// A profile UUID, which still resolves after a rename, or a profile name.
    pub profile: Option<String>,
    pub effort: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRun {
    pub source: SpecSource,
    pub coder: Coder,
    /// Absolute: a session's `PATH` need not contain `forgectl`.
    pub forgectl: PathBuf,
    pub name: String,
    pub worktree: Option<String>,
    pub base: Option<String>,
    pub rounds: u32,
    pub planner_is_session: bool,
    /// How the planner reaches Forge, so the brief names its tools.
    pub surface: ToolSurface,
}

fn quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-:=,@+".contains(&b))
    {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

pub fn source_section(source: &SpecSource) -> String {
    match source {
        SpecSource::ClaudeArtifact(url) => format!(
            "The spec is a Claude artifact: {url}\n\nRead it with your Claude artifact tool \
             (for example `read_artifact`). A plain web fetch cannot use your claude.ai login. \
             If no such tool is available, stop and ask the person to paste the spec or save \
             it to a file."
        ),
        SpecSource::Link(url) => format!(
            "The spec is at {url}\n\nRead it with your web tools. If it cannot be read, stop \
             and ask the person."
        ),
        SpecSource::File(path) => {
            format!("The spec is the file {}\n\nRead all of it.", path.display())
        }
        SpecSource::Inline(text) => {
            format!("The spec follows, between the markers.\n\n<<<SPEC\n{text}\nSPEC>>>")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &Path) -> bool {
        false
    }

    #[test]
    fn sources_are_links_files_or_text_and_a_missing_path_is_an_error() {
        assert_eq!(
            classify("https://claude.ai/public/artifacts/abc", none).unwrap(),
            SpecSource::ClaudeArtifact("https://claude.ai/public/artifacts/abc".into())
        );
        assert!(matches!(
            classify(" https://www.CLAUDE.site/x ", none).unwrap(),
            SpecSource::ClaudeArtifact(_)
        ));
        assert!(matches!(
            classify("https://claude.ai.evil.example/x", none).unwrap(),
            SpecSource::Link(_)
        ));
        assert!(matches!(
            classify("https://user@claude.ai:443/x", none).unwrap(),
            SpecSource::ClaudeArtifact(_)
        ));
        assert_eq!(
            classify("docs/spec.md", |p| p == Path::new("docs/spec.md")).unwrap(),
            SpecSource::File("docs/spec.md".into())
        );
        assert_eq!(
            classify("docs/spec.md", none),
            Err(SpecError::MissingFile("docs/spec.md".into()))
        );
        assert_eq!(
            classify("./plan", none),
            Err(SpecError::MissingFile("./plan".into()))
        );
        assert_eq!(
            classify("Add a cache to the loader.\nKeep the API.", none).unwrap(),
            SpecSource::Inline("Add a cache to the loader.\nKeep the API.".into())
        );
        assert_eq!(classify("  \n", none), Err(SpecError::Empty));
    }
}
