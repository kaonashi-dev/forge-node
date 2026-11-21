//! Many files' contents at one revision, read from the object store with a
//! number of `git cat-file` processes that does not grow with the file count.

use std::path::Path;

use crate::command::{run_git_input, MAX_CAPTURE_BYTES};

/// Header bytes a `--batch` or `--batch-check` line can take besides its size
/// digits: a sha256 oid, a type name and two separators.
const HEADER_BYTES: usize = 96;

/// Each of `paths` at `rev` (a commit or a tree), lossily decoded: `None` for
/// a path that is missing, not a blob, over `per_file` bytes, past `total`
/// bytes in order, or that git could not be asked about.
///
/// One `cat-file --batch-check` for the sizes, then `cat-file --batch` in
/// chunks of at most the capture budget, so a size is known before any
/// content is held.
#[must_use]
pub fn read_blobs(
    repo: &Path,
    rev: &str,
    paths: &[&str],
    per_file: u64,
    total: u64,
) -> Vec<Option<String>> {
    let mut found: Vec<Option<String>> = vec![None; paths.len()];
    if rev.is_empty() || rev.starts_with('-') || paths.is_empty() {
        return found;
    }
    // A path with a newline cannot be a `--batch-check` line.
    let asked: Vec<usize> = (0..paths.len())
        .filter(|&at| !paths[at].contains('\n'))
        .collect();
    let input: String = asked
        .iter()
        .map(|&at| format!("{rev}:{}\n", paths[at]))
        .collect();
    // A missing object echoes its input line, so the reply can outgrow the headers.
    let limit = input
        .len()
        .saturating_add(asked.len().saturating_mul(HEADER_BYTES + 20));
    let Ok(checked) = run_git_input(
        repo,
        &["cat-file", "--batch-check"],
        input.into_bytes(),
        limit,
    ) else {
        return found;
    };
    if !checked.status.success() {
        return found;
    }
    let lines: Vec<&[u8]> = checked
        .stdout
        .split(|byte| *byte == b'\n')
        .take(asked.len())
        .collect();
    if lines.len() != asked.len() {
        return found;
    }
    let per_file = usize::try_from(per_file).unwrap_or(usize::MAX);
    let mut budget = usize::try_from(total).unwrap_or(usize::MAX);
    let chunk_budget = MAX_CAPTURE_BYTES / 2;
    let mut chunks: Vec<Vec<(usize, String, usize)>> = vec![Vec::new()];
    let mut chunk_bytes = 0usize;
    for (&at, line) in asked.iter().zip(lines) {
        let Some((oid, size)) = blob_header(line) else {
            continue;
        };
        if size > per_file || size > budget || size + HEADER_BYTES > chunk_budget {
            continue;
        }
        budget -= size;
        if chunk_bytes + size + HEADER_BYTES > chunk_budget {
            chunks.push(Vec::new());
            chunk_bytes = 0;
        }
        chunk_bytes += size + HEADER_BYTES;
        if let Some(chunk) = chunks.last_mut() {
            chunk.push((at, oid, size));
        }
    }
    for chunk in chunks.into_iter().filter(|chunk| !chunk.is_empty()) {
        let input: String = chunk.iter().map(|(_, oid, _)| format!("{oid}\n")).collect();
        let limit = chunk.iter().map(|(_, _, size)| size + HEADER_BYTES).sum();
        let Ok(read) = run_git_input(repo, &["cat-file", "--batch"], input.into_bytes(), limit)
        else {
            continue;
        };
        if !read.status.success() {
            continue;
        }
        let mut rest = read.stdout.as_slice();
        for (at, _, size) in chunk {
            let Some(newline) = rest.iter().position(|byte| *byte == b'\n') else {
                break;
            };
            let (header, after) = rest.split_at(newline + 1);
            if blob_header(&header[..newline]).map(|(_, n)| n) != Some(size) || after.len() < size {
                break;
            }
            found[at] = Some(String::from_utf8_lossy(&after[..size]).into_owned());
            rest = after.get(size + 1..).unwrap_or_default();
        }
    }
    found
}

/// `<oid> blob <size>`; anything else (`missing`, a tree, a gitlink) is `None`.
fn blob_header(line: &[u8]) -> Option<(String, usize)> {
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split(' ');
    let (oid, kind, size) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || kind != "blob" || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some((oid.to_owned(), size.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn blobs_at_a_revision_are_read_in_order_and_the_rest_are_none() {
        let dir = tempfile::tempdir().expect("dir");
        let repo = dir.path();
        git(repo, &["init", "-q"]);
        std::fs::create_dir(repo.join("src")).expect("src");
        std::fs::write(repo.join("a.rs"), "fn a() {}\n").expect("a");
        std::fs::write(repo.join("src/b c.rs"), "fn b() {}\n").expect("b");
        std::fs::write(repo.join("big.rs"), "x".repeat(64)).expect("big");
        git(repo, &["add", "."]);
        git(
            repo,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=T",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "one",
            ],
        );
        std::fs::write(repo.join("a.rs"), "changed\n").expect("a");

        let read = read_blobs(
            repo,
            "HEAD",
            &[
                "src/b c.rs",
                "gone.rs",
                "big.rs",
                "src",
                "a.rs",
                "line\nbreak",
            ],
            32,
            1024,
        );
        assert_eq!(
            read,
            [
                Some("fn b() {}\n".to_owned()),
                None,
                None,
                None,
                Some("fn a() {}\n".to_owned()),
                None,
            ]
        );
        let capped = read_blobs(repo, "HEAD", &["a.rs", "src/b c.rs"], 32, 12);
        assert_eq!(capped, [Some("fn a() {}\n".to_owned()), None]);
        assert!(read_blobs(repo, "--help", &["a.rs"], 32, 1024)[0].is_none());
    }
}
