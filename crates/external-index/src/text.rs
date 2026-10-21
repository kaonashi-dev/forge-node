//! Bounded reads of the declaring line, and paths an index names checked against the checkout.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};

use fs_service::definition::utf16_column;

/// Longest candidate text, in bytes.
pub(crate) const MAX_TEXT_BYTES: usize = 512;

/// Most of a file read to find one line. A line past this is reported without text.
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

/// The 1-based `wanted` lines of `path`, in the order asked; `None` for a line
/// past the end, past the budget, or in something that is not a regular file.
pub(crate) fn read_lines(path: &Path, wanted: &[u32]) -> Vec<Option<String>> {
    let mut found = vec![None; wanted.len()];
    let Some(&last) = wanted.iter().max() else {
        return found;
    };
    // `metadata` before `open`: a FIFO would block the open itself.
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        _ => return found,
    }
    let Ok(file) = File::open(path) else {
        return found;
    };
    let mut reader = BufReader::new(file.take(MAX_SOURCE_BYTES));
    let mut buffer = Vec::new();
    let mut number = 0u32;
    while number < last {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        number += 1;
        if !wanted.contains(&number) {
            continue;
        }
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        let line = String::from_utf8_lossy(&buffer).into_owned();
        for (slot, want) in found.iter_mut().zip(wanted) {
            if *want == number {
                *slot = Some(line.clone());
            }
        }
    }
    found
}

/// `line` cut to [`MAX_TEXT_BYTES`] on a character boundary.
pub(crate) fn clamp(line: &str) -> String {
    if line.len() <= MAX_TEXT_BYTES {
        return line.to_owned();
    }
    let mut end = MAX_TEXT_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// 1-based UTF-16 column of the first whole-word `symbol` in `line`, or 0.
pub(crate) fn word_column(line: &str, symbol: &str) -> u32 {
    if symbol.is_empty() {
        return 0;
    }
    for (at, _) in line.match_indices(symbol) {
        let before = line[..at].chars().next_back();
        let after = line[at + symbol.len()..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            return utf16_column(line, at);
        }
    }
    0
}

/// `relative` joined onto the canonical `root`, refused when it is absolute,
/// climbs, or resolves (through a symlink) outside the checkout.
pub(crate) fn inside(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let resolved = root.join(relative).canonicalize().ok()?;
    resolved.starts_with(root).then_some(resolved)
}

/// Whether an index-supplied path is plain and relative, before anything touches the disk.
pub(crate) fn is_plain_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_read_in_the_order_asked_and_missing_ones_are_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "one\r\ntwo\nthree").unwrap();
        assert_eq!(
            read_lines(&path, &[3, 1, 9, 3]),
            [
                Some("three".into()),
                Some("one".into()),
                None,
                Some("three".into())
            ]
        );
        assert_eq!(
            read_lines(dir.path(), &[1]),
            [None],
            "a directory has no lines"
        );
    }

    #[test]
    fn a_word_column_skips_partial_matches_and_counts_utf16() {
        assert_eq!(word_column("let widgets = widget();", "widget"), 15);
        assert_eq!(word_column("😀 fn widget", "widget"), 7);
        assert_eq!(word_column("widgetry", "widget"), 0);
        assert_eq!(clamp(&"é".repeat(400)).len(), MAX_TEXT_BYTES);
    }

    #[test]
    fn paths_that_climb_or_escape_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("a.rs"), "").unwrap();
        assert!(inside(&root, "a.rs").is_some());
        assert!(inside(&root, "../a.rs").is_none());
        assert!(inside(&root, "/etc/hosts").is_none());
        std::os::unix::fs::symlink("/etc/hosts", root.join("out")).unwrap();
        assert!(inside(&root, "out").is_none());
        assert!(is_plain_relative("src/a.rs"));
        assert!(!is_plain_relative("src/../../a.rs"));
        assert!(!is_plain_relative("/a.rs"));
    }
}
