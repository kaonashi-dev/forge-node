//! Talking to a remote: `fetch`, and `push` only when a caller explicitly asks
//! (Juva's `CreatePullRequest` path). Local commits stay in [`crate::change`].
//!
//! Everything here goes through [`run_git_network`], not [`crate::run_git`]:
//! its own timeout, `ssh` in batch mode, askpass disabled. See the
//! [`crate::command`] module docs for why that is a deliberate deviation from
//! ADR-008 rather than an oversight.
//!
//! **Push is never automatic.** A background sweeper must not call it; only an
//! explicit user-confirmed request may.

use crate::command::{run_git, run_git_network, GitError, GIT_NETWORK_TIMEOUT};
use std::path::Path;
use std::time::Duration;

/// What a fetch actually did, for the message the GUI shows afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchOutcome {
    /// The remote that was fetched.
    pub remote: String,
    /// Refs git reported as updated (its stderr summary), verbatim and
    /// trimmed. Empty when the remote had nothing new.
    pub updated: Vec<String>,
}

/// Whether `repo` has any remote configured at all.
///
/// Cheap enough to call before offering a "Fetch" control: a project with no
/// remote should not be shown one.
///
/// # Errors
/// [`GitError::Io`] / [`GitError::Timeout`] if git cannot be run.
pub fn has_remote(repo: &Path) -> Result<bool, GitError> {
    const ARGS: [&str; 1] = ["remote"];
    let out = run_git(Some(repo), &ARGS)?;
    Ok(out.success() && !out.stdout_trimmed().is_empty())
}

/// The remote to fetch when the caller has no opinion.
///
/// `origin` when it exists, otherwise the first remote configured, otherwise
/// `None`. A repository with exactly one oddly-named remote is common enough
/// (`upstream`, a fork's `me`) that defaulting blindly to `origin` would make
/// the feature look broken there.
///
/// # Errors
/// [`GitError::CommandFailed`] if `git remote -v` fails.
pub fn default_remote(repo: &Path) -> Result<Option<String>, GitError> {
    let remotes = crate::repository::list_remotes(repo)?;
    if remotes.iter().any(|(name, _)| name == "origin") {
        return Ok(Some("origin".to_string()));
    }
    Ok(remotes.into_iter().next().map(|(name, _)| name))
}

/// Fetch `remote` into `repo`, pruning refs that no longer exist upstream.
///
/// `--prune` is on because the whole point of the fetch is to show the user an
/// accurate branch list; keeping a remote-tracking ref for a branch that was
/// merged and deleted last week is precisely the stale row the picker must not
/// offer. Pruning cannot lose work: it only ever removes `refs/remotes/…`, and
/// a local branch that tracked the deleted one stays exactly where it is.
///
/// `--no-tags` keeps a fetch from dragging in a release history nobody asked
/// for on repositories that publish thousands of tags.
///
/// # Errors
/// - [`GitError::InvalidBranchName`] if `remote` looks like a git option.
/// - [`GitError::CommandFailed`] if the fetch fails — the captured stderr is
///   the real message (`Permission denied (publickey)`, `Could not resolve
///   host`), which is what the GUI should show.
/// - [`GitError::Timeout`] if it outlives `timeout`.
pub fn fetch(
    repo: &Path,
    remote: &str,
    timeout: Option<Duration>,
) -> Result<FetchOutcome, GitError> {
    // `remote` reaches the CLI as a bare argument and never went through
    // `check-ref-format`; a value of `--upload-pack=…` would be an argument
    // injection into a command that opens a socket.
    if remote.starts_with('-') {
        return Err(GitError::InvalidBranchName {
            branch: remote.to_string(),
            reason: "remote name may not start with '-' (it would parse as a git option)"
                .to_string(),
        });
    }

    let args = ["fetch", "--prune", "--no-tags", "--", remote];
    let out =
        run_git_network(Some(repo), &args, timeout.unwrap_or(GIT_NETWORK_TIMEOUT))?.ok(&args)?;

    // git reports the ref updates on stderr, one indented line each; stdout is
    // empty for a plain fetch. Anything that is not a ref line ("From <url>")
    // is dropped so the caller gets a list it can count.
    let updated = out
        .stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with("From ") && !line.starts_with("remote:")
        })
        .map(String::from)
        .collect();

    tracing::debug!(target: "git", remote, "remote.fetch");
    Ok(FetchOutcome {
        remote: remote.to_string(),
        updated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_rejects_option_like_remote_names() {
        // Never reaches the network: the guard runs before the subprocess.
        let err = fetch(
            Path::new("/nonexistent"),
            "--upload-pack=touch /tmp/x",
            None,
        )
        .unwrap_err();
        assert!(matches!(err, GitError::InvalidBranchName { .. }));
    }
}
