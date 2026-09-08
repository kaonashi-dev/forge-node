//! Real temporary git repositories for git-service and daemon integration tests
//! (§21 "Determinismo": integration tests use real PTYs and real git repos).
//!
//! Unlike the fakes in this crate, [`TempRepo`] shells out to the real `git`
//! binary in a fresh [`tempfile::TempDir`], so tests exercise the same git
//! plumbing the daemon uses. The repository is created with a deterministic
//! identity (`user.name`/`user.email`), commit signing disabled, and `main` as
//! the default branch, then seeded with one commit.
//!
//! If `git` is not installed, [`init_repo`] returns an [`io::Error`] whose
//! [`kind`](io::Error::kind) is [`io::ErrorKind::NotFound`] rather than
//! panicking, so a test can skip gracefully:
//!
//! ```
//! use test_support::temp_repo::init_repo;
//!
//! match init_repo() {
//!     Ok(repo) => assert!(repo.path().join(".git").exists()),
//!     Err(e) if e.kind() == std::io::ErrorKind::NotFound => { /* git missing: skip */ }
//!     Err(e) => panic!("unexpected error: {e}"),
//! }
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A temporary git repository. The [`TempDir`] is deleted when this is dropped,
/// so keep it alive for as long as the tests need the repo.
#[derive(Debug)]
pub struct TempRepo {
    /// The backing temp directory (deleted on drop).
    pub dir: TempDir,
    /// The repository's working-tree root (equal to `dir.path()`).
    pub path: PathBuf,
}

/// Run `git -C <path> <args...>`, mapping a non-zero exit to an [`io::Error`].
///
/// A missing `git` binary surfaces as the underlying spawn error (kind
/// [`io::ErrorKind::NotFound`]) via `?`.
fn run_git(path: &Path, args: &[&str]) -> io::Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

impl TempRepo {
    /// The repository's working-tree root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write `contents` to `name` at the repo root, stage it, and commit it.
    ///
    /// # Errors
    /// Returns an [`io::Error`] if the write or any `git` command fails.
    pub fn commit_file(&self, name: &str, contents: &str) -> io::Result<()> {
        fs::write(self.path.join(name), contents)?;
        run_git(&self.path, &["add", name])?;
        let message = format!("add {name}");
        run_git(&self.path, &["commit", "-m", &message])
    }

    /// Create and switch to a new branch `name`.
    ///
    /// # Errors
    /// Returns an [`io::Error`] if the `git` command fails.
    pub fn create_branch(&self, name: &str) -> io::Result<()> {
        run_git(&self.path, &["checkout", "-b", name])
    }
}

/// Initialize a real temporary git repository on branch `main`, with a fixed
/// identity and commit signing disabled, seeded with an initial README commit.
///
/// # Errors
/// Returns an [`io::Error`] if `git` is not installed (kind
/// [`io::ErrorKind::NotFound`]) or any `git` command or file write fails.
pub fn init_repo() -> io::Result<TempRepo> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().to_path_buf();

    // `git -C <path> -c init.defaultBranch=main init` — the config override keeps
    // the default branch deterministic on older git that lacks `init -b`.
    run_git(&path, &["-c", "init.defaultBranch=main", "init"])?;

    // Deterministic, self-contained identity so commits never depend on (or
    // touch) the developer's global git config.
    run_git(&path, &["config", "user.email", "forge-test@example.com"])?;
    run_git(&path, &["config", "user.name", "Forge Test"])?;
    run_git(&path, &["config", "commit.gpgsign", "false"])?;

    // Seed an initial commit so HEAD resolves.
    fs::write(path.join("README.md"), "# forge test repo\n")?;
    run_git(&path, &["add", "README.md"])?;
    run_git(&path, &["commit", "-m", "initial commit"])?;

    Ok(TempRepo { dir, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Return the repo, or `None` (after printing a skip note) if git is absent.
    fn init_or_skip() -> Option<TempRepo> {
        match init_repo() {
            Ok(repo) => Some(repo),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                eprintln!("skipping temp_repo test: git not installed ({e})");
                None
            }
            Err(e) => panic!("init_repo failed: {e}"),
        }
    }

    #[test]
    fn init_repo_creates_git_dir_and_head() {
        let Some(repo) = init_or_skip() else { return };
        assert!(repo.path().join(".git").exists(), ".git should exist");

        // HEAD resolves to the seed commit.
        let head = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("run git rev-parse");
        assert!(head.status.success(), "HEAD should resolve");
        assert!(!String::from_utf8_lossy(&head.stdout).trim().is_empty());
    }

    #[test]
    fn commit_file_and_create_branch() {
        let Some(repo) = init_or_skip() else { return };
        repo.commit_file("hello.txt", "hello\n").unwrap();
        repo.create_branch("feature/x").unwrap();

        let branch = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .expect("run git rev-parse");
        assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feature/x");
    }
}
