//! Project domain type (§7.1).

use crate::ids::{ProjectGroupId, ProjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A user-added directory, optionally a Git repository.
///
/// A non-Git project is valid; worktree actions are simply disabled. Two
/// projects may not share the same canonical `root_path`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    /// Optional organizational group. `None` places the project in General.
    pub project_group_id: Option<ProjectGroupId>,
    /// Basename of `root_path` by default; user-editable.
    pub name: String,
    /// A glyph the user picked to stand for this project — an emoji, or any
    /// short character. `None` means the UI falls back to the project's
    /// initials, which is what every project starts with.
    ///
    /// Stored as an opaque string rather than an enum of known icons: the
    /// point is that a user can pick *their* mark, and an enum would make
    /// that a code change. `serde(default)` keeps a snapshot written before
    /// this field existed decodable.
    #[serde(default)]
    pub icon: Option<String>,
    /// Canonicalized absolute path.
    pub root_path: PathBuf,
    /// `None` if the directory is not inside a Git repository.
    pub git_root: Option<PathBuf>,
    pub created_at: Timestamp,
    pub last_opened_at: Timestamp,
}

/// A named, organizational container for related projects.
///
/// This is intentionally distinct from [`crate::Workspace`], which represents
/// an executable checkout or Git worktree within one project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGroup {
    pub id: ProjectGroupId,
    pub name: String,
    pub created_at: Timestamp,
}

/// The most characters a [`Project::icon`] may hold.
///
/// One emoji is rarely one `char`: a flag is two code points, a profession is
/// a person plus a zero-width joiner plus a tool, and a family can be seven.
/// Eight is comfortably above the longest sequence anyone picks from a
/// keyboard palette and far below anything that would break the tile.
pub const MAX_ICON_CHARS: usize = 8;

/// What [`Project::normalize_icon`] refuses.
///
/// A type rather than `()` so the one sentence explaining the rule lives with
/// the rule: the daemon puts this straight into its `InvalidRequest` message,
/// and the GUI shows the same words under the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a project icon must be a single mark with no spaces")]
pub struct InvalidIcon;

/// Whether `raw` can stand as a project's icon.
///
/// Shared by the GUI, which uses it to refuse a bad pick before sending it,
/// and by the daemon, which does not trust the GUI. Deliberately not a check
/// for "is this an emoji": a letter, a rune or a `⌘` are all legitimate marks,
/// and the Unicode question of what counts as an emoji has no answer that
/// survives a font change. What is actually being excluded is anything that
/// would not *draw* as a single mark — whitespace, control characters, and
/// strings long enough to be a label rather than a glyph.
#[must_use]
pub fn is_valid_icon(raw: &str) -> bool {
    let count = raw.chars().count();
    count > 0
        && count <= MAX_ICON_CHARS
        && !raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\u{fffc}')
}

impl Project {
    /// Whether Git-dependent actions (worktrees, branch/status) are available.
    #[must_use]
    pub fn is_git(&self) -> bool {
        self.git_root.is_some()
    }

    /// The icon to store for `raw`, or `None` to fall back to initials.
    ///
    /// Trims first, so a stray space from a paste does not become an icon
    /// nobody can see, and an entry that is only whitespace clears the icon
    /// instead of storing a blank tile. Refuses input that is
    /// neither empty nor a valid glyph, because silently clearing an icon the
    /// user was trying to *set* is the one outcome they would not expect.
    ///
    /// # Errors
    /// Returns [`InvalidIcon`] when `raw` is non-empty and fails
    /// [`is_valid_icon`].
    pub fn normalize_icon(raw: &str) -> Result<Option<String>, InvalidIcon> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        if is_valid_icon(raw) {
            Ok(Some(raw.to_owned()))
        } else {
            Err(InvalidIcon)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marks people actually pick, including the multi-code-point ones a
    /// naive one-`char` limit would reject.
    #[test]
    fn a_glyph_a_user_would_pick_is_an_icon() {
        for icon in ["🦀", "⌘", "A", "🇲🇽", "👨‍💻", "🏳️‍🌈", "→"] {
            assert!(is_valid_icon(icon), "{icon} was refused");
        }
    }

    /// What is excluded is anything that would not draw as one mark.
    #[test]
    fn a_label_is_not_an_icon() {
        assert!(!is_valid_icon(""), "nothing is not a mark");
        assert!(!is_valid_icon("HP Backend"), "a name is not a mark");
        assert!(!is_valid_icon("a b"), "two marks are not one");
        assert!(!is_valid_icon("\u{7}"), "a bell is not a mark");
        assert!(
            !is_valid_icon("🦀🦀🦀🦀🦀🦀🦀🦀🦀"),
            "nine crabs are a label"
        );
    }

    /// Empty clears, a glyph sets, and a label is refused rather than quietly
    /// clearing what the user was trying to set.
    #[test]
    fn normalizing_separates_clearing_from_a_bad_pick() {
        assert_eq!(Project::normalize_icon(""), Ok(None));
        assert_eq!(Project::normalize_icon("   "), Ok(None));
        assert_eq!(Project::normalize_icon(" 🦀 "), Ok(Some("🦀".to_owned())));
        assert_eq!(Project::normalize_icon("HP Backend"), Err(InvalidIcon));
    }
}
