//! What git says about a tree row, from the same `GetWorkspaceDiff` answer
//! the Git panel lists, so the two never disagree and decorating a listing
//! asks the daemon for nothing new. Built when the diff lands, never per frame.

use std::collections::HashMap;

use domain::{DiffFile, DiffStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Added,
    Modified,
    Deleted,
    Conflict,
}

/// The letter a decorated file carries beside its tint, so the state is
/// never colour alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub letter: &'static str,
    pub tone: Tone,
}

/// A status this build does not know is shown as modified, not dropped:
/// "something happened to this file" is true, and silence is not.
#[must_use]
pub fn mark_for(status: DiffStatus) -> Mark {
    let (letter, tone) = match status {
        DiffStatus::Added => ("A", Tone::Added),
        DiffStatus::Deleted => ("D", Tone::Deleted),
        DiffStatus::Conflicted => ("C", Tone::Conflict),
        _ => ("M", Tone::Modified),
    };
    Mark { letter, tone }
}

#[derive(Debug, Default)]
pub struct Decorations {
    files: HashMap<String, Mark>,
    /// Changed paths under each folder, so a folded `src/` says how much it hides.
    folders: HashMap<String, usize>,
}

impl Decorations {
    #[must_use]
    pub fn from_diff(files: &[DiffFile]) -> Self {
        let mut decorations = Self::default();
        for file in files {
            decorations
                .files
                .insert(file.path.clone(), mark_for(file.status));
            let mut end = 0;
            while let Some(slash) = file.path[end..].find('/') {
                end += slash;
                *decorations
                    .folders
                    .entry(file.path[..end].to_string())
                    .or_default() += 1;
                end += 1;
            }
        }
        decorations
    }

    #[must_use]
    pub fn file(&self, path: &str) -> Option<Mark> {
        self.files.get(path).copied()
    }

    #[must_use]
    pub fn folder(&self, path: &str) -> Option<usize> {
        self.folders.get(path).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, status: DiffStatus) -> DiffFile {
        DiffFile {
            path: path.into(),
            status,
            additions: 0,
            deletions: 0,
            patch: String::new(),
            binary: false,
            truncated: false,
        }
    }

    #[test]
    fn files_carry_a_letter_and_every_ancestor_counts_them() {
        let decorations = Decorations::from_diff(&[
            changed("src/a/b.rs", DiffStatus::Modified),
            changed("src/c.rs", DiffStatus::Added),
            changed("README.md", DiffStatus::Conflicted),
        ]);
        assert_eq!(
            decorations.file("src/c.rs"),
            Some(Mark {
                letter: "A",
                tone: Tone::Added
            })
        );
        assert_eq!(
            decorations.file("README.md").map(|mark| mark.tone),
            Some(Tone::Conflict)
        );
        assert_eq!(decorations.folder("src"), Some(2));
        assert_eq!(decorations.folder("src/a"), Some(1));
        assert_eq!(decorations.folder("src/a/b.rs"), None, "not itself");
        assert_eq!(decorations.folder(""), None, "the root has no row");
    }
}
