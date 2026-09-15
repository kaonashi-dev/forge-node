//! Gutter marks for one file: which lines the working tree changed.
//!
//! `--unified=0` and the hunk headers alone. The bodies are what a patch view
//! needs; a gutter needs only *which lines*, and reading the headers keeps this
//! flat in the size of the file rather than in the size of its diff.

use std::path::Path;

use crate::command::run_git;
use crate::GitError;

/// What one line's mark says happened to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Added,
    Modified,
    /// Something was removed *at* this line. The line itself is still there —
    /// the mark is an anchor, not a statement about its content.
    Deleted,
}

/// One line and what happened to it, 1-based like git counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub line: u32,
    pub kind: MarkKind,
}

/// The marks for `relative` in `repo`'s working tree.
///
/// An untracked file has no diff against the index, so it is reported as added
/// whole — which is what it is. A path git knows nothing about yields nothing
/// rather than an error: a gutter is decoration, and failing the buffer over it
/// would be the wrong trade.
pub fn file_marks(repo: &Path, relative: &str, line_count: u32) -> Result<Vec<Mark>, GitError> {
    let tracked = run_git(Some(repo), &["ls-files", "--error-unmatch", "--", relative])
        .map(|out| out.status == 0)
        .unwrap_or(false);
    if !tracked {
        return Ok((1..=line_count)
            .map(|line| Mark {
                line,
                kind: MarkKind::Added,
            })
            .collect());
    }
    let out = run_git(
        Some(repo),
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--unified=0",
            "HEAD",
            "--",
            relative,
        ],
    )?;
    Ok(marks_from_hunks(&out.stdout, line_count))
}

/// Parse `@@ -a,b +c,d @@` headers into per-line marks.
///
/// Pure, so the shape of the answer is testable without a repository.
#[must_use]
pub fn marks_from_hunks(patch: &str, line_count: u32) -> Vec<Mark> {
    let mut marks = Vec::new();
    for line in patch.lines() {
        let Some(header) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some((removed, rest)) = header.split_once(' ') else {
            continue;
        };
        let added = rest.split(' ').next().unwrap_or("");
        let Some(removed) = parse_range(removed.strip_prefix('-')) else {
            continue;
        };
        let Some(added) = parse_range(added.strip_prefix('+')) else {
            continue;
        };
        match (removed.1, added.1) {
            // Nothing added: the removal is anchored at the line that now
            // stands where it was. `+c,0` means "after line c".
            (_, 0) => {
                let at = added.0.max(1).min(line_count.max(1));
                marks.push(Mark {
                    line: at,
                    kind: MarkKind::Deleted,
                });
            }
            // Nothing removed: every added line is new.
            (0, count) => push_run(&mut marks, added.0, count, MarkKind::Added, line_count),
            // Both: the overlap reads as modified, the surplus as added.
            (removed_count, added_count) => {
                let shared = removed_count.min(added_count);
                push_run(&mut marks, added.0, shared, MarkKind::Modified, line_count);
                push_run(
                    &mut marks,
                    added.0 + shared,
                    added_count - shared,
                    MarkKind::Added,
                    line_count,
                );
            }
        }
    }
    marks.sort_by_key(|mark| mark.line);
    marks.dedup_by_key(|mark| mark.line);
    marks
}

fn push_run(marks: &mut Vec<Mark>, start: u32, count: u32, kind: MarkKind, line_count: u32) {
    for offset in 0..count {
        let line = start + offset;
        if line == 0 || (line_count > 0 && line > line_count) {
            continue;
        }
        marks.push(Mark { line, kind });
    }
}

/// `12` is one line at 12; `12,3` is three from 12; `12,0` is none.
fn parse_range(spec: Option<&str>) -> Option<(u32, u32)> {
    let spec = spec?;
    match spec.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((spec.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_added_run_marks_every_line_it_covers() {
        let marks = marks_from_hunks("@@ -0,0 +3,2 @@\n", 10);
        assert_eq!(
            marks,
            [
                Mark {
                    line: 3,
                    kind: MarkKind::Added
                },
                Mark {
                    line: 4,
                    kind: MarkKind::Added
                },
            ]
        );
    }

    #[test]
    fn a_one_line_hunk_has_an_implicit_count_of_one() {
        assert_eq!(
            marks_from_hunks("@@ -5 +5 @@\n", 10),
            [Mark {
                line: 5,
                kind: MarkKind::Modified
            }]
        );
    }

    #[test]
    fn a_removal_anchors_at_the_line_that_took_its_place() {
        assert_eq!(
            marks_from_hunks("@@ -4,3 +3,0 @@\n", 10),
            [Mark {
                line: 3,
                kind: MarkKind::Deleted
            }]
        );
    }

    /// A replacement that grew: the overlap is modified, the surplus is new.
    #[test]
    fn a_grown_replacement_is_modified_then_added() {
        assert_eq!(
            marks_from_hunks("@@ -2,1 +2,3 @@\n", 10),
            [
                Mark {
                    line: 2,
                    kind: MarkKind::Modified
                },
                Mark {
                    line: 3,
                    kind: MarkKind::Added
                },
                Mark {
                    line: 4,
                    kind: MarkKind::Added
                },
            ]
        );
    }

    /// The gutter is drawn against the buffer, so a mark past its end would
    /// point at a row that does not exist.
    #[test]
    fn marks_are_clamped_to_the_document() {
        assert_eq!(
            marks_from_hunks("@@ -0,0 +9,4 @@\n", 10),
            [
                Mark {
                    line: 9,
                    kind: MarkKind::Added
                },
                Mark {
                    line: 10,
                    kind: MarkKind::Added
                },
            ]
        );
        assert_eq!(
            marks_from_hunks("@@ -4,3 +99,0 @@\n", 10),
            [Mark {
                line: 10,
                kind: MarkKind::Deleted
            }]
        );
    }

    #[test]
    fn one_line_carries_one_mark_and_they_come_in_order() {
        let marks = marks_from_hunks("@@ -0,0 +5,1 @@\n@@ -1,1 +5,1 @@\n@@ -0,0 +2,1 @@\n", 10);
        let lines: Vec<u32> = marks.iter().map(|mark| mark.line).collect();
        assert_eq!(lines, [2, 5], "sorted, and one mark per line");
    }

    #[test]
    fn noise_and_malformed_headers_are_skipped() {
        for patch in [
            "",
            "diff --git a/x b/x\n",
            "@@ broken @@\n",
            "@@ -x,y +z,w @@\n",
            "@@\n",
            "@@ -1,1 @@\n",
        ] {
            assert!(marks_from_hunks(patch, 10).is_empty(), "{patch:?}");
        }
    }
}
