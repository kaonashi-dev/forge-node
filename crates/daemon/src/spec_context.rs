//! What a saved spec claims about the checkout, checked against it: the `path:line` facts of its
//! code context and the programs its automatic validations start. Every miss is a warning for
//! G1, never a refusal. Disk only, so callers hold no lock.

use std::io::{BufRead as _, BufReader, Read as _};
use std::path::{Component, Path, PathBuf};

use domain::spec_run::draft::CodeRef;

/// Bytes of one cited file read to count its lines; a line past them is not checked.
const MAX_SCANNED_BYTES: u64 = 8 * 1024 * 1024;

#[must_use]
pub(crate) fn check_refs(root: &Path, refs: &[CodeRef]) -> Vec<String> {
    let Ok(root) = root.canonicalize() else {
        return vec!["the checkout could not be read; code context was not checked".to_owned()];
    };
    refs.iter()
        .filter_map(|cited| check_ref(&root, cited).err())
        .collect()
}

fn check_ref(root: &Path, cited: &CodeRef) -> Result<(), String> {
    let at = format!("{}:{}", cited.path, cited.line);
    let path = inside(root, &cited.path).map_err(|miss| {
        let why = match miss {
            Miss::Missing => "does not exist in the checkout",
            Miss::Outside => "is outside the checkout",
        };
        format!("code context {at}: {} {why}", cited.path)
    })?;
    if !path.is_file() {
        return Err(format!("code context {at}: {} is not a file", cited.path));
    }
    let found = has_line(&path, u64::from(cited.line), MAX_SCANNED_BYTES)
        .map_err(|e| format!("code context {at}: {e}"))?;
    match found {
        Lines::Reached => Ok(()),
        Lines::Short(lines) => Err(format!(
            "code context {at}: {} has {lines} lines",
            cited.path
        )),
        Lines::Unread => Err(format!(
            "code context {at}: the line is past the first {} MiB of {}, which Forge does not read",
            MAX_SCANNED_BYTES / (1024 * 1024),
            cited.path
        )),
    }
}

enum Lines {
    Reached,
    /// The whole file was read and has this many lines.
    Short(u64),
    /// The cap ended the read before the line.
    Unread,
}

enum Miss {
    Missing,
    Outside,
}

/// `cited` resolved under `root` (canonical) after symlinks. An absolute path counts when it
/// resolves inside; a relative one may not climb out with `..`.
fn inside(root: &Path, cited: &str) -> Result<PathBuf, Miss> {
    let cited = Path::new(cited);
    let joined = if cited.is_absolute() {
        cited.to_path_buf()
    } else if cited
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        root.join(cited)
    } else {
        return Err(Miss::Outside);
    };
    let path = joined.canonicalize().map_err(|_| Miss::Missing)?;
    if path.starts_with(root) {
        Ok(path)
    } else {
        Err(Miss::Outside)
    }
}

/// Reads until line `needed` exists, the file ends, or more than `cap` bytes are read.
fn has_line(path: &Path, needed: u64, cap: u64) -> std::io::Result<Lines> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file.take(cap + 1));
    let mut newlines = 0u64;
    let mut partial = false;
    let mut read = 0u64;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let room = usize::try_from(cap - read).unwrap_or(usize::MAX);
        let counted = &chunk[..chunk.len().min(room)];
        let Some(&last) = counted.last() else {
            // The one byte past `cap` only says that the file goes on.
            return Ok(Lines::Unread);
        };
        newlines += counted.iter().filter(|b| **b == b'\n').count() as u64;
        partial = last != b'\n';
        read += counted.len() as u64;
        let len = counted.len();
        reader.consume(len);
        if newlines + u64::from(partial) >= needed {
            return Ok(Lines::Reached);
        }
    }
    Ok(Lines::Short(newlines + u64::from(partial)))
}

/// A program with a slash is a file of the checkout; a bare one is looked up along the launch
/// environment's `PATH`, the one check sessions get.
#[must_use]
pub(crate) fn check_programs(
    root: &Path,
    programs: &[(String, String)],
    path_entries: &[PathBuf],
) -> Vec<String> {
    let root = root.canonicalize().ok();
    programs
        .iter()
        .filter_map(|(id, program)| {
            let (found, where_) = if Path::new(program).is_absolute() {
                (is_executable_file(Path::new(program)), "an executable file")
            } else if program.contains('/') {
                let found = root
                    .as_deref()
                    .and_then(|root| inside(root, program).ok())
                    .is_some_and(|path| is_executable_file(&path));
                (found, "an executable file in the checkout yet")
            } else {
                let found = path_entries
                    .iter()
                    .any(|dir| is_executable_file(&dir.join(program)));
                (found, "on the launch PATH")
            };
            (!found).then(|| format!("{id}'s validation runs {program}, which is not {where_}"))
        })
        .collect()
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn cited(path: &str, line: u32) -> CodeRef {
        CodeRef {
            path: path.into(),
            line,
        }
    }

    #[test]
    fn a_cited_line_must_exist_inside_the_checkout() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("src/lib.rs"), "a\nb\nc").unwrap();
        std::fs::write(outside.path().join("secret.rs"), "x\n".repeat(50)).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.rs"),
            repo.path().join("link.rs"),
        )
        .unwrap();

        let absolute = repo.path().join("src/lib.rs");
        let outside_file = outside.path().join("secret.rs");
        let warnings = check_refs(
            repo.path(),
            &[
                cited("src/lib.rs", 3),
                cited(&absolute.to_string_lossy(), 2),
                cited(&outside_file.to_string_lossy(), 2),
                cited("src/lib.rs", 4),
                cited("src/gone.rs", 1),
                cited("src", 1),
                cited("../escape.rs", 1),
                cited("link.rs", 2),
            ],
        );
        assert_eq!(
            warnings,
            [
                format!(
                    "code context {0}:2: {0} is outside the checkout",
                    outside_file.display()
                ),
                "code context src/lib.rs:4: src/lib.rs has 3 lines".to_owned(),
                "code context src/gone.rs:1: src/gone.rs does not exist in the checkout".to_owned(),
                "code context src:1: src is not a file".to_owned(),
                "code context ../escape.rs:1: ../escape.rs is outside the checkout".to_owned(),
                "code context link.rs:2: link.rs is outside the checkout".to_owned(),
            ]
        );
    }

    #[test]
    fn a_line_past_the_read_cap_is_reported_unread_not_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.rs");
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert!(matches!(has_line(&path, 2, 4).unwrap(), Lines::Reached));
        assert!(matches!(has_line(&path, 4, 4).unwrap(), Lines::Unread));
        assert!(matches!(has_line(&path, 3, 4).unwrap(), Lines::Unread));
        assert!(matches!(has_line(&path, 4, 100).unwrap(), Lines::Reached));
        assert!(matches!(has_line(&path, 5, 100).unwrap(), Lines::Short(4)));
    }

    #[test]
    fn a_validation_program_is_a_checkout_script_or_on_the_launch_path() {
        let repo = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join("scripts")).unwrap();
        for (dir, name, mode) in [
            (repo.path(), "scripts/dev", 0o755),
            (repo.path(), "scripts/plain", 0o644),
            (bin.path(), "cargo", 0o755),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let programs = [
            ("AC-1".to_owned(), "scripts/dev".to_owned()),
            ("AC-2".to_owned(), "cargo".to_owned()),
            ("AC-3".to_owned(), "scripts/plain".to_owned()),
            ("AC-4".to_owned(), "bun".to_owned()),
            ("AC-5".to_owned(), "/bin/sh".to_owned()),
            ("AC-6".to_owned(), "/nonexistent/tool".to_owned()),
        ];
        assert_eq!(
            check_programs(repo.path(), &programs, &[bin.path().to_path_buf()]),
            [
                "AC-3's validation runs scripts/plain, which is not an executable file in the checkout yet",
                "AC-4's validation runs bun, which is not on the launch PATH",
                "AC-6's validation runs /nonexistent/tool, which is not an executable file",
            ]
        );
    }
}
