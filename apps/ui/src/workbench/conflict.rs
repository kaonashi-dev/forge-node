//! Conflict regions read back out of a working-tree patch.
//!
//! A stopped replay leaves `<<<<<<<` / `|||||||` / `=======` / `>>>>>>>` in the
//! file, and `git diff HEAD` carries them as ordinary added lines. So the
//! three-way view is a reading of the patch the Git panel already has, never a
//! read of the index stages the daemon does not expose.

use super::patch::{Kind, Row, rows};

/// One conflict region as it stands in the working tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Working-tree line of the opening `<<<<<<<` marker.
    pub line: Option<u32>,
    pub ours: Vec<String>,
    /// `None` when git wrote the conflict without diff3 markers.
    pub base: Option<Vec<String>>,
    pub theirs: Vec<String>,
    /// False when the patch's context skipped lines inside the region.
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Ours,
    Base,
    Theirs,
}

struct Open {
    line: Option<u32>,
    ours: Vec<String>,
    base: Option<Vec<String>>,
    theirs: Vec<String>,
    section: Section,
    next: Option<u32>,
    complete: bool,
}

fn seven(text: &str, marker: char) -> bool {
    let mut chars = text.chars();
    for _ in 0..7 {
        if chars.next() != Some(marker) {
            return false;
        }
    }
    matches!(chars.next(), None | Some(' '))
}

fn split_line(text: &str) -> bool {
    text == "======="
}

fn finish(open: Open, complete: bool) -> Block {
    Block {
        line: open.line,
        ours: open.ours,
        base: open.base,
        theirs: open.theirs,
        complete,
    }
}

/// Every complete or partially visible conflict region in `rows`, in order.
#[must_use]
pub fn blocks_from_rows(rows: &[Row]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut open: Option<Open> = None;
    for row in rows {
        if row.kind == Kind::Hunk {
            if let Some(current) = &mut open {
                current.complete = false;
            }
            continue;
        }
        if row.after.is_none() {
            continue;
        }
        if let Some(current) = &open
            && let (Some(next), Some(at)) = (current.next, row.after)
            && at != next
        {
            open.as_mut().expect("checked").complete = false;
        }
        if seven(&row.text, '<') {
            if let Some(current) = open.take() {
                blocks.push(finish(current, false));
            }
            open = Some(Open {
                line: row.after,
                ours: Vec::new(),
                base: None,
                theirs: Vec::new(),
                section: Section::Ours,
                next: row.after.map(|line| line + 1),
                complete: true,
            });
            continue;
        }
        let Some(current) = &mut open else {
            continue;
        };
        current.next = row.after.map(|line| line + 1);
        if seven(&row.text, '|') && current.section == Section::Ours {
            current.section = Section::Base;
            current.base = Some(Vec::new());
        } else if split_line(&row.text) && current.section != Section::Theirs {
            current.section = Section::Theirs;
        } else if seven(&row.text, '>') && current.section == Section::Theirs {
            let finished = open.take().expect("open");
            let complete = finished.complete;
            blocks.push(finish(finished, complete));
        } else {
            match current.section {
                Section::Ours => current.ours.push(row.text.clone()),
                Section::Base => current
                    .base
                    .get_or_insert_with(Vec::new)
                    .push(row.text.clone()),
                Section::Theirs => current.theirs.push(row.text.clone()),
            }
        }
    }
    if let Some(current) = open {
        blocks.push(finish(current, false));
    }
    blocks
}

/// The conflict regions of a working-tree patch.
#[must_use]
pub fn blocks(patch: &str) -> Vec<Block> {
    blocks_from_rows(&rows(patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(lines: &[&str]) -> String {
        let mut text = String::from("@@ -1,10 +1,20 @@\n");
        for line in lines {
            text.push(' ');
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    #[test]
    fn a_diff3_region_keeps_ours_base_and_theirs_apart() {
        let text = patch(&[
            "<<<<<<< HEAD",
            "ours",
            "||||||| 4f2b1ac",
            "base",
            "=======",
            "theirs",
            ">>>>>>> feature",
        ]);
        let blocks = blocks(&text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].line, Some(1));
        assert_eq!(blocks[0].ours, ["ours"]);
        assert_eq!(
            blocks[0].base.as_deref(),
            Some(["base".to_string()].as_slice())
        );
        assert_eq!(blocks[0].theirs, ["theirs"]);
        assert!(blocks[0].complete);
    }

    #[test]
    fn a_plain_region_has_no_base_and_says_so() {
        let text = patch(&["<<<<<<< HEAD", "ours", "=======", "theirs", ">>>>>>> x"]);
        let blocks = blocks(&text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].base, None);
        assert_eq!(blocks[0].ours, ["ours"]);
        assert_eq!(blocks[0].theirs, ["theirs"]);
    }

    #[test]
    fn an_unfinished_region_is_incomplete() {
        let text = patch(&["<<<<<<< HEAD", "ours", "=======", "theirs"]);
        let blocks = blocks(&text);
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].complete);
    }

    #[test]
    fn a_hunk_boundary_inside_a_region_is_incomplete() {
        let text =
            "@@ -1,3 +1,3 @@\n <<<<<<< HEAD\n ours\n@@ -20,3 +20,3 @@\n theirs\n >>>>>>> x\n";
        let blocks = blocks(text);
        assert_eq!(blocks.len(), 1);
        assert!(
            !blocks[0].complete,
            "the context skipped part of the region"
        );
    }

    #[test]
    fn a_non_contiguous_region_is_incomplete() {
        let mut rows =
            super::rows("@@ -1,3 +1,8 @@\n <<<<<<< HEAD\n ours\n=======\n theirs\n >>>>>>> x\n");
        rows[3].after = Some(9);
        let blocks = blocks_from_rows(&rows);
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].complete);
    }

    #[test]
    fn two_regions_and_none_left() {
        let text = patch(&[
            "<<<<<<< HEAD",
            "a",
            "=======",
            "b",
            ">>>>>>> x",
            "between",
            "<<<<<<< HEAD",
            "c",
            "=======",
            "d",
            ">>>>>>> x",
        ]);
        let regions = blocks(&text);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[1].line, Some(7));
        assert!(blocks(&patch(&["resolved", "already"])).is_empty());
    }

    #[test]
    fn marker_text_needs_seven_marks_at_the_start() {
        let text = patch(&["<<<<<< nope", "x", "=======", "y"]);
        assert!(blocks(&text).is_empty(), "six marks are not a marker");
    }
}
