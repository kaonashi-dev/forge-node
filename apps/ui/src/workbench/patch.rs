//! Reading a unified patch into rows to paint, and pairing removed lines with
//! the added lines that replaced them for a side-by-side layout.
//!
//! The daemon produces the patch with `git diff`; nothing here re-derives it.
//! The only job is turning the text into rows that carry their own line
//! numbers, because a diff without them is unusable for talking about a change.

/// the original client's `ui.diff.split`: the layout both the working-tree diff and the
/// branch compare read, so one choice holds across the two.
pub const DIFF_SPLIT_KEY: &str = "ui.diff.split";

/// One row of a unified patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Added,
    Removed,
    Hunk,
    Meta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    /// The line without its `+`/`-`/space marker, which is drawn separately.
    pub text: String,
    /// Line number on the old side, when the row has one.
    pub before: Option<u32>,
    /// Line number on the new side (the working-tree line), when it has one.
    pub after: Option<u32>,
}

/// One visual row of the split layout: indexes into the unified [`Row`] list.
/// A `None` side is a filler, so the two columns never drift apart by the net
/// line count of the hunks above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub left: Option<usize>,
    pub right: Option<usize>,
}

/// `@@ -12,3 +14,4 @@ fn main` → the old and new starting line.
fn hunk_start(line: &str) -> Option<(u32, u32)> {
    let mut parts = line.split(' ');
    if parts.next()? != "@@" {
        return None;
    }
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |range: &str| range.split(',').next()?.parse().ok();
    Some((start(old)?, start(new)?))
}

/// A patch as rows. A patch with no hunk header still renders as meta, because
/// a truncated or unusual patch should show what it has instead of nothing.
#[must_use]
pub fn rows(patch: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let (mut before, mut after) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in patch.split('\n') {
        if let Some((old, new)) = hunk_start(line) {
            before = old;
            after = new;
            in_hunk = true;
            rows.push(Row {
                kind: Kind::Hunk,
                text: line.to_string(),
                before: None,
                after: None,
            });
            continue;
        }
        if !in_hunk {
            if !line.is_empty() {
                rows.push(Row {
                    kind: Kind::Meta,
                    text: line.to_string(),
                    before: None,
                    after: None,
                });
            }
            continue;
        }
        if line.starts_with('\\') {
            rows.push(Row {
                kind: Kind::Meta,
                text: line.to_string(),
                before: None,
                after: None,
            });
            continue;
        }
        let marker = line.chars().next();
        let text = line.get(1..).unwrap_or_default();
        let row = |kind, text: &str, before: Option<u32>, after: Option<u32>| Row {
            kind,
            text: text.to_string(),
            before,
            after,
        };
        match marker {
            Some('+') => {
                rows.push(row(Kind::Added, text, None, Some(after)));
                after += 1;
            }
            Some('-') => {
                rows.push(row(Kind::Removed, text, Some(before), None));
                before += 1;
            }
            Some(' ') | None => {
                rows.push(row(Kind::Context, text, Some(before), Some(after)));
                before += 1;
                after += 1;
            }
            _ => rows.push(row(Kind::Meta, line, None, None)),
        }
    }
    // A patch ends with a newline, which splits into a trailing empty row that
    // is not a line of the file.
    if rows
        .last()
        .is_some_and(|row| row.kind == Kind::Context && row.text.is_empty())
    {
        rows.pop();
    }
    rows
}

/// The two columns of a split view, paired per hunk as the original client's `splitRows`:
/// a run of removed lines followed by a run of added lines is paired
/// positionally, an unequal run leaves the shorter side as fillers.
#[must_use]
pub fn pairs(rows: &[Row]) -> Vec<Pair> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if rows[index].kind == Kind::Removed {
            let removed = index;
            while index < rows.len() && rows[index].kind == Kind::Removed {
                index += 1;
            }
            let added = index;
            while index < rows.len() && rows[index].kind == Kind::Added {
                index += 1;
            }
            let (del, add) = (added - removed, index - added);
            for offset in 0..del.max(add) {
                out.push(Pair {
                    left: (offset < del).then_some(removed + offset),
                    right: (offset < add).then_some(added + offset),
                });
            }
            continue;
        }
        if rows[index].kind == Kind::Added {
            out.push(Pair {
                left: None,
                right: Some(index),
            });
            index += 1;
            continue;
        }
        out.push(Pair {
            left: Some(index),
            right: Some(index),
        });
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = concat!(
        "diff --git a/a.rs b/a.rs\n",
        "index 1111111..2222222 100644\n",
        "--- a/a.rs\n",
        "+++ b/a.rs\n",
        "@@ -1,3 +1,4 @@\n",
        " one\n",
        "-two\n",
        "+2\n",
        "+two and a half\n",
        " three\n",
    );

    #[test]
    fn rows_carry_their_own_line_numbers() {
        let rows = rows(PATCH);
        assert_eq!(rows[0].kind, Kind::Meta);
        let hunk = rows.iter().position(|row| row.kind == Kind::Hunk).unwrap();
        assert_eq!((rows[hunk].before, rows[hunk].after), (None, None));
        let context: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == Kind::Context)
            .map(|row| (row.text.as_str(), row.before, row.after))
            .collect();
        assert_eq!(
            context,
            [("one", Some(1), Some(1)), ("three", Some(3), Some(4))]
        );
        let removed = rows.iter().find(|row| row.kind == Kind::Removed).unwrap();
        assert_eq!((removed.before, removed.after), (Some(2), None));
        let added: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == Kind::Added)
            .map(|row| (row.text.as_str(), row.before, row.after))
            .collect();
        assert_eq!(
            added,
            [("2", None, Some(2)), ("two and a half", None, Some(3))]
        );
        assert_eq!(
            rows.last().map(|row| row.text.as_str()),
            Some("three"),
            "the trailing newline is not a line of the file"
        );
    }

    #[test]
    fn a_change_pairs_positionally_and_leaves_filler_rows() {
        let patch = "@@ -1,2 +1,3 @@\n-a\n-b\n+c\n+d\n+e\n";
        let rows = rows(patch);
        let pairs = pairs(&rows);
        assert_eq!(
            pairs,
            [
                Pair {
                    left: Some(0),
                    right: Some(0)
                },
                Pair {
                    left: Some(1),
                    right: Some(3)
                },
                Pair {
                    left: Some(2),
                    right: Some(4)
                },
                Pair {
                    left: None,
                    right: Some(5)
                },
            ]
        );
    }

    #[test]
    fn an_added_line_alone_is_a_right_column_and_context_is_shared() {
        let patch = "@@ -1,0 +1,1 @@\n+only\n@@ -4,1 +5,1 @@\n same\n";
        let rows = rows(patch);
        let pairs = pairs(&rows);
        assert!(pairs.iter().any(|pair| {
            pair.left.is_none() && pair.right.is_some_and(|row| rows[row].text == "only")
        }));
        // Headers and context share one index so the row is painted full width.
        assert!(pairs.iter().any(|pair| {
            pair.left
                .is_some_and(|index| pair.right == Some(index) && rows[index].kind == Kind::Hunk)
        }));
        assert!(pairs.iter().any(|pair| {
            pair.left
                .is_some_and(|index| pair.right == Some(index) && rows[index].kind == Kind::Context)
        }));
    }

    #[test]
    fn a_removed_run_does_not_pair_across_the_next_hunk() {
        let patch = "@@ -1,1 +1,0 @@\n-gone\n@@ -8,0 +8,1 @@\n+new\n";
        let rows = rows(patch);
        let pairs = pairs(&rows);
        let removed = pairs
            .iter()
            .position(|pair| pair.left.is_some_and(|row| rows[row].kind == Kind::Removed))
            .expect("removed");
        assert_eq!(pairs[removed].right, None, "no added line in that hunk");
        let added = pairs
            .iter()
            .position(|pair| pair.right.is_some_and(|row| rows[row].kind == Kind::Added))
            .expect("added");
        assert_eq!(pairs[added].left, None, "no removed line in that hunk");
    }

    #[test]
    fn a_deleted_line_marks_the_old_side_only() {
        let patch = "@@ -1,1 +1,0 @@\n-gone\n";
        let rows = rows(patch);
        assert_eq!(rows[1].before, Some(1));
        assert_eq!(rows[1].after, None);
    }
}
