//! Working-tree diffs and local commits for Juva (§14 adjacent).
//!
//! Read paths stay local (`run_git`). [`push`] opens a socket and therefore
//! goes through [`crate::run_git_network`] — same hardening as [`crate::fetch`].

use crate::command::{run_git, run_git_network, GitError, GIT_NETWORK_TIMEOUT};
use crate::repository::{current_branch, default_branch, status};
use std::path::Path;
use std::time::Duration;

/// Soft ceiling for patch text sent over IPC / into a Juva prompt.
pub const MAX_PATCH_BYTES: usize = 48 * 1024;

/// Soft ceiling for how many changed paths are listed.
pub const MAX_FILES: usize = 200;

/// A change snapshot ready for Juva or the GUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSnapshot {
    pub branch: Option<String>,
    pub default_branch: Option<String>,
    pub dirty: bool,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub files: Vec<(String, String)>,
    pub patch: String,
    pub truncated: bool,
}

/// Collect status + a bounded patch for `repo`.
///
/// # Errors
/// [`GitError::CommandFailed`] when git cannot describe the tree.
pub fn change_context(repo: &Path) -> Result<ChangeSnapshot, GitError> {
    let st = status(repo)?;
    let branch = st.branch.clone();
    let default = default_branch(repo).ok().flatten();
    let files = list_changed_files(repo)?;
    let (patch, truncated) = collect_patch(repo)?;

    Ok(ChangeSnapshot {
        branch,
        default_branch: default,
        dirty: st.dirty,
        ahead: st.ahead,
        behind: st.behind,
        files,
        patch,
        truncated,
    })
}

/// Stage everything under `repo` (tracked + untracked) and create a commit.
///
/// # Errors
/// - [`GitError::CommandFailed`] when there is nothing to commit or git fails.
/// - [`GitError::InvalidBranchName`] is unused here; reserved for callers that
///   validate messages themselves.
pub fn commit(repo: &Path, message: &str) -> Result<String, GitError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(GitError::CommandFailed {
            args: vec!["commit".into()],
            status: -1,
            stderr: "commit message is empty".into(),
        });
    }

    // Stage the whole tree. Juva's first cut is "commit what is dirty", not a
    // staging UI. `-A` includes deletions and untracked files.
    const ADD: [&str; 2] = ["add", "-A"];
    run_git(Some(repo), &ADD)?.ok(&ADD)?;

    let args = ["commit", "-m", message];
    let out = run_git(Some(repo), &args)?.ok(&args)?;
    // `git commit` prints the subject on stdout; fall back to HEAD.
    let sha = tip_sha(repo).unwrap_or_else(|_| out.stdout_trimmed().to_string());
    tracing::debug!(target: "git", %sha, "change.commit");
    Ok(sha)
}

/// Push the current branch to `remote`, setting upstream when missing.
///
/// # Errors
/// Same class as [`crate::fetch`]: network, auth, and timeout.
pub fn push(repo: &Path, remote: &str, timeout: Option<Duration>) -> Result<(), GitError> {
    if remote.starts_with('-') {
        return Err(GitError::InvalidBranchName {
            branch: remote.to_string(),
            reason: "remote name may not start with '-' (it would parse as a git option)"
                .to_string(),
        });
    }
    let branch = current_branch(repo)?.ok_or_else(|| GitError::CommandFailed {
        args: vec!["push".into()],
        status: -1,
        stderr: "detached HEAD: refuse to push without an explicit ref".into(),
    })?;

    let args = ["push", "-u", "--", remote, &branch];
    run_git_network(Some(repo), &args, timeout.unwrap_or(GIT_NETWORK_TIMEOUT))?.ok(&args)?;
    tracing::debug!(target: "git", remote, %branch, "change.push");
    Ok(())
}

fn tip_sha(repo: &Path) -> Result<String, GitError> {
    const ARGS: [&str; 2] = ["rev-parse", "HEAD"];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;
    Ok(out.stdout_trimmed().to_string())
}

fn list_changed_files(repo: &Path) -> Result<Vec<(String, String)>, GitError> {
    const ARGS: [&str; 2] = ["status", "--porcelain"];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;
    let mut files = Vec::new();
    for line in out.stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = line[..2].trim().to_string();
        let path = line[3..].trim().to_string();
        if path.is_empty() {
            continue;
        }
        files.push((path, status));
        if files.len() >= MAX_FILES {
            break;
        }
    }
    Ok(files)
}

fn collect_patch(repo: &Path) -> Result<(String, bool), GitError> {
    // Staged first, then unstaged — Juva should see both. Untracked content is
    // summarized by path only (diffing every untracked file is unbounded).
    let mut patch = String::new();
    let mut truncated = false;

    for args in [
        ["diff", "--cached", "--"].as_slice(),
        ["diff", "--"].as_slice(),
    ] {
        let out = run_git(Some(repo), args)?;
        if !out.success() {
            // An empty repo with no commits yet fails `diff --cached`; skip.
            continue;
        }
        if !out.stdout.is_empty() {
            if !patch.is_empty() {
                patch.push('\n');
            }
            patch.push_str(&out.stdout);
        }
        if patch.len() > MAX_PATCH_BYTES {
            patch.truncate(MAX_PATCH_BYTES);
            truncated = true;
            break;
        }
    }

    Ok((patch, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn change_context_sees_dirty_file() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(repo, &["add", "a.txt"]);
        git(repo, &["commit", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();

        let snap = change_context(repo).unwrap();
        assert!(snap.dirty);
        assert!(snap.files.iter().any(|(p, _)| p == "a.txt"));
        assert!(snap.patch.contains("two") || snap.patch.contains("a.txt"));
    }

    #[test]
    fn commit_stages_and_records_message() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(repo, &["add", "a.txt"]);
        git(repo, &["commit", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();

        let sha = commit(repo, "fix: update a").unwrap();
        assert!(!sha.is_empty());
        let snap = change_context(repo).unwrap();
        assert!(!snap.dirty);
    }
}
