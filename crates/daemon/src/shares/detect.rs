//! What a project could share, and what kind of thing each path is (§14.2).
//!
//! Detection exists to make the first rule cheap, not to define what may be
//! shared: anything the user names is a valid rule. The classification is a
//! suggestion made once, never a decision taken at apply time.

use std::path::Path;

use domain::{ShareCandidate, ShareClass, ShareRule};

use super::apply::measure;
use super::EntryKind;

/// Paths reported before the scan stops. A project with more ignored entries
/// than this at its root is not a list anybody reads.
pub const MAX_CANDIDATES: usize = 200;

/// Classify one path by name — the only information available before anything
/// is read, and enough for the five buckets that matter.
#[must_use]
pub fn classify(path: &str) -> ShareClass {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();

    if lower.starts_with(".env")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".keystore")
        || lower == ".npmrc"
        || lower == ".netrc"
        || lower == "credentials.json"
        || lower == "secrets.json"
    {
        return ShareClass::Secret;
    }
    if matches!(
        lower.as_str(),
        "node_modules" | ".venv" | "venv" | "vendor" | "pods" | ".bundle" | "bower_components"
    ) {
        return ShareClass::Dependencies;
    }
    if matches!(
        lower.as_str(),
        "target" | "dist" | "build" | "out" | ".next" | ".nuxt" | ".svelte-kit"
    ) {
        return ShareClass::BuildOutput;
    }
    if matches!(
        lower.as_str(),
        ".turbo" | ".cache" | ".parcel-cache" | ".gradle" | ".pytest_cache" | "__pycache__"
    ) {
        return ShareClass::Cache;
    }
    if matches!(
        lower.as_str(),
        ".vscode" | ".idea" | ".claude" | ".cursor" | ".zed" | ".tool-versions" | ".direnv"
    ) {
        return ShareClass::EditorState;
    }
    ShareClass::Other
}

/// Turn the paths git reports into candidates, newest question first: what is
/// it, how big is it, and what would Forge do with it.
///
/// `ruled` is the set of paths that already have a rule; those rows are still
/// returned, flagged, so the section can show that the scan agrees with the
/// list rather than silently hiding what is configured.
#[must_use]
pub fn candidates(
    source: &Path,
    reported: &[git_service::IgnoredPath],
    rules: &[ShareRule],
) -> (Vec<ShareCandidate>, bool) {
    let mut out = Vec::new();
    let truncated = reported.len() > MAX_CANDIDATES;

    for entry in reported.iter().take(MAX_CANDIDATES) {
        let path = entry.path.trim_end_matches('/');
        if path.is_empty() || path.starts_with(".git/") {
            continue;
        }
        let full = source.join(path);
        let kind = EntryKind::of(&full);
        if !kind.exists() {
            continue;
        }
        let is_dir = kind == EntryKind::Dir;
        let class = classify(path);
        // Measuring every ignored path would walk the whole build tree; the
        // number is only worth having where the answer changes what the user
        // picks, which is for directories.
        let (size_bytes, entries) = if is_dir {
            measure(&full)
        } else {
            (
                std::fs::symlink_metadata(&full).ok().map(|m| m.len()),
                Some(1),
            )
        };
        out.push(ShareCandidate {
            path: path.to_owned(),
            class,
            is_dir,
            size_bytes,
            entries,
            suggested: class.suggested(is_dir),
            already_ruled: rules.iter().any(|rule| rule.path == path),
        });
    }

    // Secrets first, then dependencies: the order the user acts in.
    out.sort_by_key(|candidate| {
        (
            match candidate.class {
                ShareClass::Secret => 0,
                ShareClass::Dependencies => 1,
                ShareClass::EditorState => 2,
                ShareClass::BuildOutput => 3,
                ShareClass::Cache => 4,
                _ => 5,
            },
            candidate.path.clone(),
        )
    });
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_buckets_catch_what_projects_actually_ignore() {
        assert_eq!(classify(".env"), ShareClass::Secret);
        assert_eq!(classify(".env.local"), ShareClass::Secret);
        assert_eq!(classify("config/local.pem"), ShareClass::Secret);
        assert_eq!(classify("node_modules"), ShareClass::Dependencies);
        assert_eq!(classify("api/.venv"), ShareClass::Dependencies);
        assert_eq!(classify("target"), ShareClass::BuildOutput);
        assert_eq!(classify(".next"), ShareClass::BuildOutput);
        assert_eq!(classify(".turbo"), ShareClass::Cache);
        assert_eq!(classify(".claude"), ShareClass::EditorState);
        assert_eq!(classify("notes.txt"), ShareClass::Other);
    }
}
