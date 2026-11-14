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
    /// A picture in the project's own checkout that stands for it — its
    /// favicon or app icon — as a path relative to `root_path`. Drawn instead
    /// of `icon` while it reads and decodes; `icon` stays the fallback.
    #[serde(default)]
    pub icon_image: Option<String>,
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

/// The longest [`Project::icon_image`] path accepted, in UTF-8 bytes.
pub const MAX_ICON_IMAGE_PATH_BYTES: usize = 1024;

/// What an icon image may be: formats a favicon or an app icon ships in that
/// the GUI can decode.
pub const ICON_IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "svg", "ico", "icns"];

/// What [`Project::normalize_icon_image`] refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a project icon image must be a relative path to a .png, .jpg, .svg, .ico or .icns file inside the project")]
pub struct InvalidIconImage;

/// Whether `raw` names an image inside a checkout: relative, never climbing
/// out through `..`, and ending in one of [`ICON_IMAGE_EXTENSIONS`].
///
/// Only the shape is checked: the daemon still resolves the path inside the
/// checkout before reading it.
#[must_use]
pub fn is_icon_image_path(raw: &str) -> bool {
    if raw.is_empty()
        || raw.len() > MAX_ICON_IMAGE_PATH_BYTES
        || raw.starts_with('/')
        || raw.chars().any(char::is_control)
        || raw.split('/').any(|part| part.is_empty() || part == "..")
    {
        return false;
    }
    raw.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && !stem.ends_with('/')
            && ICON_IMAGE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
    })
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

    /// The image path to store for `raw`, or `None` to clear it; the same
    /// trim-then-refuse contract as [`Project::normalize_icon`].
    ///
    /// # Errors
    /// Returns [`InvalidIconImage`] when `raw` is non-empty and fails
    /// [`is_icon_image_path`].
    pub fn normalize_icon_image(raw: &str) -> Result<Option<String>, InvalidIconImage> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        if is_icon_image_path(raw) {
            Ok(Some(raw.to_owned()))
        } else {
            Err(InvalidIconImage)
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

    #[test]
    fn an_icon_image_is_a_known_image_inside_the_checkout() {
        for path in [
            "favicon.ico",
            "public/favicon.svg",
            "src-tauri/icons/icon.icns",
            "Assets.xcassets/AppIcon.appiconset/Icon 1024.PNG",
        ] {
            assert!(is_icon_image_path(path), "{path} was refused");
        }
        for path in [
            "",
            "/etc/favicon.png",
            "../other/favicon.png",
            "public/../../favicon.png",
            "public//favicon.png",
            ".env",
            "notes.txt",
            "icons/.png",
            "favicon",
        ] {
            assert!(!is_icon_image_path(path), "{path:?} was accepted");
        }
        assert_eq!(Project::normalize_icon_image("  "), Ok(None));
        assert_eq!(
            Project::normalize_icon_image(" public/favicon.svg "),
            Ok(Some("public/favicon.svg".to_owned()))
        );
        assert_eq!(
            Project::normalize_icon_image("../x.png"),
            Err(InvalidIconImage)
        );
    }
}
