//! A save the daemon refused because the file changed on disk under the
//! buffer: keep mine, take disk, or compare the two. The two texts are fetched on demand with
//! `GetEditorConflict`; `EditorState.conflict` only says there is something
//! to fetch.

use std::rc::Rc;

/// Past either bound the comparison falls back to pairing lines by position,
/// rather than spending O(ND) memory on two unrelated files.
const MAX_TRACE_BYTES: usize = 8 * 1024 * 1024;
const MAX_EDIT_DISTANCE: usize = 512;
const MAX_LINES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Equal,
    Removed,
    Added,
}

/// One row of the comparison: `left` numbers the disk's line, `right` mine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub change: Change,
    pub text: String,
    pub left: Option<u32>,
    pub right: Option<u32>,
}

/// A session's conflict as this side holds it; it outlives a tab switch and
/// is dropped once the flag clears.
#[derive(Clone, Debug, Default)]
pub struct Conflict {
    pub comparing: bool,
    pub loading: bool,
    pub rows: Option<Rc<[Row]>>,
    pub error: Option<String>,
}

fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A final newline is not a line of its own on either side.
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Disk against mine, line by line: Myers' O(ND) diff, bounded.
#[must_use]
pub fn line_diff(disk: &str, mine: &str) -> Vec<Row> {
    let a = lines(disk);
    let b = lines(mine);
    let (n, m) = (a.len(), b.len());
    if n + m > MAX_LINES {
        return coarse(&a, &b);
    }
    let max = n + m;
    let offset = max as isize;
    let mut v = vec![0usize; 2 * max + 2];
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let at = |k: isize| (offset + k) as usize;
    'outer: for d in 0..=max {
        if (trace.len() + 1) * v.len() * size_of::<usize>() > MAX_TRACE_BYTES
            || d > MAX_EDIT_DISTANCE
        {
            return coarse(&a, &b);
        }
        trace.push(v.clone());
        let d = d as isize;
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                break 'outer;
            }
        }
    }

    let mut edits: Vec<(Change, &str)> = Vec::new();
    let (mut x, mut y) = (n as isize, m as isize);
    for (d, snap) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && snap[at(k - 1)] < snap[at(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = snap[at(prev_k)] as isize;
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            edits.push((Change::Equal, a[x as usize]));
        }
        if d == 0 {
            break;
        }
        if x == prev_x {
            y -= 1;
            edits.push((Change::Added, b[y as usize]));
        } else {
            x -= 1;
            edits.push((Change::Removed, a[x as usize]));
        }
    }
    edits.reverse();
    number(edits)
}

fn number(edits: Vec<(Change, &str)>) -> Vec<Row> {
    let (mut left, mut right) = (1u32, 1u32);
    edits
        .into_iter()
        .map(|(change, text)| {
            let (l, r) = match change {
                Change::Equal => (Some(left), Some(right)),
                Change::Removed => (Some(left), None),
                Change::Added => (None, Some(right)),
            };
            left += u32::from(l.is_some());
            right += u32::from(r.is_some());
            Row {
                change,
                text: text.to_string(),
                left: l,
                right: r,
            }
        })
        .collect()
}

fn coarse(a: &[&str], b: &[&str]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(a.len().max(b.len()));
    for i in 0..a.len().max(b.len()) {
        let line = i as u32 + 1;
        match (a.get(i), b.get(i)) {
            (Some(left), Some(right)) if left == right => rows.push(Row {
                change: Change::Equal,
                text: (*left).to_string(),
                left: Some(line),
                right: Some(line),
            }),
            (left, right) => {
                if let Some(left) = left {
                    rows.push(Row {
                        change: Change::Removed,
                        text: (*left).to_string(),
                        left: Some(line),
                        right: None,
                    });
                }
                if let Some(right) = right {
                    rows.push(Row {
                        change: Change::Added,
                        text: (*right).to_string(),
                        left: None,
                        right: Some(line),
                    });
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(rows: &[Row]) -> Vec<(char, &str)> {
        rows.iter()
            .map(|row| {
                let mark = match row.change {
                    Change::Equal => ' ',
                    Change::Removed => '-',
                    Change::Added => '+',
                };
                (mark, row.text.as_str())
            })
            .collect()
    }

    #[test]
    fn an_edit_in_the_middle_is_one_line_out_and_one_in() {
        let rows = line_diff("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(
            shape(&rows),
            vec![(' ', "a"), ('-', "b"), ('+', "B"), (' ', "c")]
        );
        assert_eq!(rows[2].right, Some(2));
        assert_eq!(rows[3].left, Some(3));
        assert_eq!(rows[3].right, Some(3));
    }

    #[test]
    fn inserted_and_dropped_lines_keep_both_numberings() {
        let rows = line_diff("one\ntwo\nthree", "zero\none\nthree\nfour");
        assert_eq!(
            shape(&rows),
            vec![
                ('+', "zero"),
                (' ', "one"),
                ('-', "two"),
                (' ', "three"),
                ('+', "four")
            ]
        );
        assert_eq!((rows[3].left, rows[3].right), (Some(3), Some(3)));
    }

    #[test]
    fn identical_and_empty_sides_need_no_search() {
        assert!(line_diff("", "").is_empty());
        assert_eq!(shape(&line_diff("x\n", "x")), vec![(' ', "x")]);
        assert_eq!(shape(&line_diff("", "new\n")), vec![('+', "new")]);
    }

    #[test]
    fn two_unrelated_files_fall_back_to_pairing_by_position() {
        let disk: String = (0..600).map(|n| format!("disk {n}\n")).collect();
        let mine: String = (0..600).map(|n| format!("mine {n}\n")).collect();
        let rows = line_diff(&disk, &mine);
        assert_eq!(rows.len(), 1200);
        assert_eq!(shape(&rows[..2]), vec![('-', "disk 0"), ('+', "mine 0")]);
    }
}
