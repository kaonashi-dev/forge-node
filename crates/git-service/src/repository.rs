//! Read-only repository queries built on the ADR-008 command list (§14.1).
//!
//! These functions return lightweight, crate-local result structs
//! ([`RepoStatus`], [`WorktreeEntry`]). They deliberately do **not** construct
//! `domain::Workspace` values — the daemon's `WorkspaceService` owns that
//! mapping (§9.3, §17); this crate only reports what git says.

use crate::command::{run_git, GitError};
use std::path::{Path, PathBuf};

/// One ref as reported by `for-each-ref` (§14.3).
///
/// Crate-local, like [`RepoStatus`] and [`WorktreeEntry`]: this crate reports
/// what git says and the daemon maps it into `domain::BranchRef`, which also
/// carries the "checked out in workspace X" answer that only the daemon knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefEntry {
    /// Short branch name with neither `refs/heads/` nor the remote prefix.
    pub name: String,
    /// `None` for a local branch, otherwise the remote's name.
    pub remote: Option<String>,
    /// The tracked upstream in `origin/main` form, when configured.
    pub upstream: Option<String>,
    /// Committer date of the tip as a Unix timestamp.
    pub committed_at: Option<i64>,
    /// Subject line of the tip commit.
    pub subject: Option<String>,
}

/// A parsed summary of `git status --porcelain=v2 --branch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// Current branch, or `None` when detached.
    pub branch: Option<String>,
    /// `true` if the working tree has any tracked change or untracked file.
    pub dirty: bool,
    /// The commit HEAD points at, or `None` when unborn or unknown.
    pub head: Option<String>,
    /// Commits ahead of upstream, when an upstream is configured.
    pub ahead: Option<u32>,
    /// Commits behind upstream, when an upstream is configured.
    pub behind: Option<u32>,
}

/// One entry from `git worktree list --porcelain` (§14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Absolute path of the worktree's working directory.
    pub path: PathBuf,
    /// Short branch name (the `refs/heads/` prefix is stripped), or `None`
    /// when the worktree is detached or bare.
    pub branch: Option<String>,
    /// The checked-out commit object id, when present.
    pub head: Option<String>,
    /// `true` for the repository's bare main entry.
    pub bare: bool,
    /// `true` when the worktree has a detached HEAD.
    pub detached: bool,
    /// `true` when the worktree is locked.
    pub locked: bool,
    /// `true` when git says the entry is prunable — its `gitdir` file points to
    /// a non-existent location, which is what a directory deleted by hand
    /// leaves behind. The entry still appears in `worktree list` until a
    /// `worktree prune`, so it must not be treated as a live checkout.
    pub prunable: bool,
}

impl WorktreeEntry {
    /// Whether this entry names no live checkout: git marked it `prunable`, or
    /// its directory is gone. A `locked` entry is not stale — locked says the
    /// user does not want it pruned, not that it is missing.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.prunable || !self.path.exists()
    }
}

/// Discover the top-level working directory containing `path` (§14.1).
///
/// Uses `rev-parse --show-toplevel`. If `path` is inside another repository's
/// worktree, git reports that worktree's toplevel and it is accepted as-is.
///
/// # Errors
/// [`GitError::NotAGitRepo`] if `path` is not inside a git repository.
pub fn discover_root(path: &Path) -> Result<PathBuf, GitError> {
    let out = run_git(Some(path), &["rev-parse", "--show-toplevel"])?;
    if out.success() {
        Ok(PathBuf::from(out.stdout_trimmed()))
    } else {
        Err(GitError::NotAGitRepo(path.to_path_buf()))
    }
}

/// The repository's **common** git directory, absolute (§14.2).
///
/// Every worktree of a repository shares one common dir: `--git-dir` from
/// inside a linked worktree answers `.git/worktrees/<name>`, while this answers
/// the main `.git` from anywhere. It is therefore the one location all the
/// workspaces of a project agree on, which is what makes it the home of the
/// shared-file store and of the managed `info/exclude` block.
///
/// # Errors
/// [`GitError::NotAGitRepo`] when `repo` is not inside a repository;
/// [`GitError::Io`] / [`GitError::Timeout`] if git cannot be run.
pub fn common_dir(repo: &Path) -> Result<PathBuf, GitError> {
    let out = run_git(
        Some(repo),
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if !out.success() {
        return Err(GitError::NotAGitRepo(repo.to_path_buf()));
    }
    let dir = out.stdout_trimmed();
    if dir.is_empty() {
        return Err(GitError::NotAGitRepo(repo.to_path_buf()));
    }
    Ok(PathBuf::from(dir))
}

/// Whether `path` is tracked by git in `repo` (§14.2).
///
/// A tracked file needs no sharing rule: git already puts it in every worktree,
/// and an injected copy on top of it would show as a modification.
///
/// # Errors
/// [`GitError::Io`] / [`GitError::Timeout`] if git cannot be run.
pub fn is_tracked(repo: &Path, relative: &str) -> Result<bool, GitError> {
    let out = run_git(Some(repo), &["ls-files", "--error-unmatch", "--", relative])?;
    Ok(out.success())
}

/// Paths git ignores in `repo`, as `git status` reports them (§14.2).
///
/// `--ignored=matching` names a directory that matches an ignore pattern
/// *as the directory* rather than walking into it, so `node_modules/` is one
/// entry and not forty thousand. Untracked-but-not-ignored paths come back
/// too: a file the user has not committed and has not ignored is exactly the
/// kind of local thing a worktree also needs.
///
/// # Errors
/// [`GitError::CommandFailed`] when git refuses the repository;
/// [`GitError::Io`] / [`GitError::Timeout`] if git cannot be run.
pub fn ignored_paths(repo: &Path) -> Result<Vec<IgnoredPath>, GitError> {
    let out = run_git(
        Some(repo),
        &["status", "--porcelain", "--ignored=matching", "-z"],
    )?;
    if !out.success() {
        return Err(GitError::CommandFailed {
            args: vec![
                "status".to_owned(),
                "--porcelain".to_owned(),
                "--ignored=matching".to_owned(),
            ],
            status: out.status,
            stderr: out.stderr.clone(),
        });
    }
    let mut found = Vec::new();
    for record in out.stdout.split('\0') {
        // `XY <path>`: `!!` is ignored, `??` untracked. Anything else is a
        // tracked change, which is git's business and not ours.
        let Some((code, path)) = record.split_at_checked(3) else {
            continue;
        };
        let ignored = match &code[..2] {
            "!!" => true,
            "??" => false,
            _ => continue,
        };
        let path = path.trim_end_matches('/');
        if path.is_empty() || path == ".git" {
            continue;
        }
        found.push(IgnoredPath {
            path: path.to_owned(),
            ignored,
        });
    }
    Ok(found)
}

/// Rewrite the block of ignore rules Forge manages in the repository's
/// **common** `info/exclude` (§14.2).
///
/// Everything outside the delimiters is preserved verbatim, and an empty
/// `patterns` removes the block. This is what keeps an injected `.env` or
/// `node_modules` symlink from making every provisioned worktree read as dirty
/// — without touching the user's committed `.gitignore`.
///
/// # Errors
/// [`GitError::NotAGitRepo`] when the common dir cannot be resolved;
/// [`GitError::Io`] when the file cannot be read or written.
pub fn set_local_excludes(repo: &Path, patterns: &[String]) -> Result<(), GitError> {
    let info = common_dir(repo)?.join("info");
    let file = info.join("exclude");
    let existing = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(GitError::Io(e)),
    };

    let mut kept = String::new();
    let mut inside = false;
    for line in existing.lines() {
        if line.trim() == EXCLUDE_BEGIN {
            inside = true;
            continue;
        }
        if line.trim() == EXCLUDE_END {
            inside = false;
            continue;
        }
        if !inside {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    while kept.ends_with("\n\n") {
        kept.pop();
    }

    let mut out = kept;
    if !patterns.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(EXCLUDE_BEGIN);
        out.push('\n');
        for pattern in patterns {
            out.push_str(pattern);
            out.push('\n');
        }
        out.push_str(EXCLUDE_END);
        out.push('\n');
    }

    std::fs::create_dir_all(&info).map_err(GitError::Io)?;
    std::fs::write(&file, out).map_err(GitError::Io)
}

/// Opening delimiter of the managed block in `info/exclude`.
const EXCLUDE_BEGIN: &str = "# >>> forge: shared files (managed, do not edit)";
/// Closing delimiter of the managed block in `info/exclude`.
const EXCLUDE_END: &str = "# <<< forge";

/// One path `git status --ignored` reported (§14.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoredPath {
    /// Relative to the repository root, with no trailing slash.
    pub path: String,
    /// `true` when an ignore rule matches it, `false` for merely untracked.
    pub ignored: bool,
}

/// The current branch of `repo`, or `None` when HEAD is detached or unborn.
///
/// Uses `rev-parse --abbrev-ref HEAD`, which prints `HEAD` when detached.
///
/// # Errors
/// [`GitError::Io`] / [`GitError::Timeout`] if git cannot be run.
pub fn current_branch(repo: &Path) -> Result<Option<String>, GitError> {
    let out = run_git(Some(repo), &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if !out.success() {
        // Unborn branch (no commits yet) or other transient state: best effort.
        return Ok(None);
    }
    let branch = out.stdout_trimmed();
    if branch.is_empty() || branch == "HEAD" {
        Ok(None)
    } else {
        Ok(Some(branch.to_string()))
    }
}

/// The full hash `HEAD` points at, or `None` when there are no commits yet.
///
/// Best effort by design: this is what a session records as its baseline, and
/// a folder workspace or an unborn branch simply has none. A failure here must
/// never stop a session from starting.
#[must_use]
pub fn head_commit(repo: &Path) -> Option<String> {
    const ARGS: [&str; 3] = ["rev-parse", "--verify", "HEAD"];
    let out = run_git(Some(repo), &ARGS).ok()?;
    if !out.success() {
        return None;
    }
    let id = out.stdout_trimmed();
    (!id.is_empty()).then(|| id.to_string())
}

/// Whether `commit` still resolves to an object in `repo`.
///
/// A recorded baseline can stop resolving: a rebase, an amend, a branch that
/// was rewritten under the checkout. The caller falls back to `HEAD` and says
/// so rather than diffing against a ref git no longer knows.
#[must_use]
pub fn commit_exists(repo: &Path, commit: &str) -> bool {
    let spec = format!("{commit}^{{commit}}");
    run_git(Some(repo), &["rev-parse", "--verify", "--quiet", &spec]).is_ok_and(|out| out.success())
}

/// The common ancestor of every commit in `commits`, or `None` if there is none.
///
/// One subprocess whatever the number of sessions: `merge-base --octopus` is
/// what answers "the point all of these started after". A single input is
/// returned as itself without asking git.
#[must_use]
pub fn merge_base(repo: &Path, commits: &[String]) -> Option<String> {
    match commits {
        [] => None,
        [only] => Some(only.clone()),
        many => {
            let mut args = vec!["merge-base", "--octopus"];
            args.extend(many.iter().map(String::as_str));
            let out = run_git(Some(repo), &args).ok()?;
            if !out.success() {
                return None;
            }
            let id = out.stdout_trimmed();
            (!id.is_empty()).then(|| id.to_string())
        }
    }
}

/// `commit` in the abbreviated form git itself would print, for display.
#[must_use]
pub fn short_commit(commit: &str) -> String {
    commit.chars().take(SHORT_COMMIT_LEN).collect()
}

/// How many characters of a hash a list shows. Git's own default abbreviation.
const SHORT_COMMIT_LEN: usize = 7;

/// The remote's default branch (`origin/HEAD` target), best effort (§14.1).
///
/// Uses `symbolic-ref refs/remotes/origin/HEAD` and strips the
/// `refs/remotes/origin/` prefix. Returns `None` if no such ref exists.
///
/// # Errors
/// [`GitError::Io`] / [`GitError::Timeout`] if git cannot be run.
pub fn default_branch(repo: &Path) -> Result<Option<String>, GitError> {
    let out = run_git(Some(repo), &["symbolic-ref", "refs/remotes/origin/HEAD"])?;
    if !out.success() {
        return Ok(None);
    }
    let full = out.stdout_trimmed();
    let short = full.strip_prefix("refs/remotes/origin/").unwrap_or(full);
    if short.is_empty() {
        Ok(None)
    } else {
        Ok(Some(short.to_string()))
    }
}

/// Parsed working-tree status of `repo` via `status --porcelain=v2 --branch`.
///
/// # Errors
/// [`GitError::CommandFailed`] if git reports an error (e.g. not a repo).
pub fn status(repo: &Path) -> Result<RepoStatus, GitError> {
    const ARGS: [&str; 3] = ["status", "--porcelain=v2", "--branch"];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;
    Ok(parse_status_v2(&out.stdout))
}

/// Local branch names, via `branch --list --format=%(refname:short)`.
///
/// # Errors
/// [`GitError::CommandFailed`] if git reports an error.
pub fn list_branches(repo: &Path) -> Result<Vec<String>, GitError> {
    const ARGS: [&str; 3] = ["branch", "--list", "--format=%(refname:short)"];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;
    Ok(out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Every local and remote-tracking branch, newest commit first (§14.3).
///
/// One `for-each-ref` over `refs/heads` **and** `refs/remotes` rather than two
/// `git branch` calls: it is a single subprocess, it sorts server-side, and it
/// is the only form that also yields the upstream and the commit date the
/// picker sorts by.
///
/// `refs/remotes/<remote>/HEAD` is dropped. It is a symbolic ref standing for
/// the remote's default branch, not a branch of its own, so listing it would
/// offer the user "origin/HEAD" as something to check out.
///
/// # Errors
/// [`GitError::CommandFailed`] if git reports an error.
pub fn list_refs(repo: &Path) -> Result<Vec<RefEntry>, GitError> {
    // Tab-separated because a branch name may contain almost anything else,
    // and `%(contents:subject)` goes last so a tab inside it cannot shift a
    // field. `LC_ALL=C` plus `:unix` keeps the date locale-independent.
    const FORMAT: &str =
        "--format=%(refname)\t%(upstream:short)\t%(committerdate:unix)\t%(contents:subject)";
    const ARGS: [&str; 5] = [
        "for-each-ref",
        "--sort=-committerdate",
        FORMAT,
        "refs/heads",
        "refs/remotes",
    ];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;
    Ok(parse_refs(&out.stdout))
}

/// The configured remotes and their fetch URLs, via `remote -v`.
///
/// Returns each remote once even though `remote -v` prints a fetch and a push
/// line per remote; the fetch URL is the one that matters here.
///
/// # Errors
/// [`GitError::CommandFailed`] if git reports an error.
pub fn list_remotes(repo: &Path) -> Result<Vec<(String, String)>, GitError> {
    const ARGS: [&str; 2] = ["remote", "-v"];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;
    let mut seen: Vec<(String, String)> = Vec::new();
    for line in out.stdout.lines() {
        // `origin\tgit@host:repo.git (fetch)`
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if kind != "(fetch)" {
            continue;
        }
        if !seen.iter().any(|(n, _)| n == name) {
            seen.push((name.to_string(), url.to_string()));
        }
    }
    Ok(seen)
}

/// All worktrees of `repo`, via `worktree list --porcelain`.
///
/// # Errors
/// [`GitError::CommandFailed`] if git reports an error.
pub fn list_worktrees(repo: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
    const ARGS: [&str; 3] = ["worktree", "list", "--porcelain"];
    let out = run_git(Some(repo), &ARGS)?.ok(&ARGS)?;
    Ok(parse_worktrees(&out.stdout))
}

/// Parse the porcelain v2 status output.
///
/// The `--porcelain=v2 --branch` format emits `# branch.*` header lines followed
/// by one line per changed/untracked/unmerged entry:
/// - `# branch.head <name>` (or `(detached)`),
/// - `# branch.oid <oid>` (or `(initial)` before the first commit),
/// - `# branch.ab +<ahead> -<behind>` (only when an upstream is configured),
/// - entry lines begin with `1`, `2`, `u` (tracked changes / renames / unmerged)
///   or `?` (untracked); any such line means the tree is dirty.
fn parse_status_v2(text: &str) -> RepoStatus {
    let mut st = RepoStatus::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            let head = rest.trim();
            st.branch = if head == "(detached)" {
                None
            } else {
                Some(head.to_string())
            };
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for token in rest.split_whitespace() {
                if let Some(a) = token.strip_prefix('+') {
                    st.ahead = a.parse().ok();
                } else if let Some(b) = token.strip_prefix('-') {
                    st.behind = b.parse().ok();
                }
            }
        } else if let Some(rest) = line.strip_prefix("# branch.oid ") {
            // `(initial)` is git's word for an unborn HEAD, not an object id.
            st.head = if rest.trim() == "(initial)" {
                None
            } else {
                Some(rest.trim().to_string())
            };
        } else if line.starts_with('#') {
            // Other header line (branch.upstream): ignored.
        } else if !line.trim().is_empty() {
            // Any non-header, non-blank line is a change/untracked entry.
            st.dirty = true;
        }
    }
    st
}

/// Parse the tab-separated `for-each-ref` output of [`list_refs`].
///
/// A line is `<refname>\t<upstream>\t<unix date>\t<subject>`; empty fields are
/// empty strings, and the subject is taken as the rest of the line so a tab
/// inside it cannot shift anything.
fn parse_refs(text: &str) -> Vec<RefEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.splitn(4, '\t');
        let Some(refname) = fields.next() else {
            continue;
        };
        let upstream = fields.next().unwrap_or("");
        let date = fields.next().unwrap_or("");
        let subject = fields.next().unwrap_or("");

        let (name, remote) = if let Some(rest) = refname.strip_prefix("refs/heads/") {
            (rest.to_string(), None)
        } else if let Some(rest) = refname.strip_prefix("refs/remotes/") {
            // `<remote>/<branch…>`; the branch part may itself contain slashes.
            let Some((remote, branch)) = rest.split_once('/') else {
                continue;
            };
            // A symbolic ref standing for the remote default, not a branch.
            if branch == "HEAD" {
                continue;
            }
            (branch.to_string(), Some(remote.to_string()))
        } else {
            continue;
        };
        if name.is_empty() {
            continue;
        }

        out.push(RefEntry {
            name,
            remote,
            upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
            committed_at: date.parse::<i64>().ok(),
            subject: (!subject.trim().is_empty()).then(|| subject.trim().to_string()),
        });
    }
    out
}

/// Parse `worktree list --porcelain`: records separated by blank lines, each a
/// `worktree <path>` line followed by attribute lines
/// (`HEAD`, `branch`, `bare`, `detached`, `locked`, `prunable`).
fn parse_worktrees(text: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    for line in text.lines() {
        if line.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                branch: None,
                head: None,
                bare: false,
                detached: false,
                locked: false,
                prunable: false,
            });
        } else if let Some(entry) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                entry.head = Some(head.to_string());
            } else if let Some(branch) = line.strip_prefix("branch ") {
                entry.branch = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_string(),
                );
            } else if line == "bare" {
                entry.bare = true;
            } else if line == "detached" {
                entry.detached = true;
            } else if line == "locked" || line.starts_with("locked ") {
                entry.locked = true;
            } else if line == "prunable" || line.starts_with("prunable ") {
                entry.prunable = true;
            }
            // Any future attribute is ignored, like `prunable`'s reason line.
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_status_on_main() {
        let text = "# branch.oid abc123\n# branch.head main\n";
        let st = parse_status_v2(text);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.head.as_deref(), Some("abc123"));
        assert!(!st.dirty);
        assert_eq!(st.ahead, None);
        assert_eq!(st.behind, None);
    }

    #[test]
    fn parses_an_unborn_head_as_no_oid() {
        // Git prints the literal `(initial)` before the first commit; taking
        // it as an oid would make every later read look like a movement.
        let text = "# branch.oid (initial)\n# branch.head main\n";
        let st = parse_status_v2(text);
        assert_eq!(st.head, None);
        assert_eq!(st.branch.as_deref(), Some("main"));
    }

    #[test]
    fn parses_a_status_without_an_oid_line_as_none() {
        let text = "# branch.head main\n";
        let st = parse_status_v2(text);
        assert_eq!(st.head, None);
    }

    #[test]
    fn parses_dirty_and_ahead_behind() {
        let text = "# branch.head main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 aaa bbb file.rs\n? new.txt\n";
        let st = parse_status_v2(text);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert!(st.dirty);
        assert_eq!(st.ahead, Some(2));
        assert_eq!(st.behind, Some(1));
    }

    #[test]
    fn parses_detached_status() {
        let text = "# branch.head (detached)\n";
        let st = parse_status_v2(text);
        assert_eq!(st.branch, None);
    }

    #[test]
    fn parses_local_and_remote_refs() {
        let text = "refs/heads/main\torigin/main\t1750000000\tInitial commit\n\
                    refs/heads/feature/auth\t\t1750000100\tWIP: auth\n\
                    refs/remotes/origin/main\t\t1750000000\tInitial commit\n\
                    refs/remotes/origin/feature/deep/nest\t\t1749000000\tOld work\n";
        let refs = parse_refs(text);
        assert_eq!(refs.len(), 4);

        assert_eq!(refs[0].name, "main");
        assert_eq!(refs[0].remote, None);
        assert_eq!(refs[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(refs[0].committed_at, Some(1_750_000_000));
        assert_eq!(refs[0].subject.as_deref(), Some("Initial commit"));

        // No upstream configured: the empty field is `None`, not `Some("")`.
        assert_eq!(refs[1].name, "feature/auth");
        assert_eq!(refs[1].upstream, None);

        assert_eq!(refs[2].name, "main");
        assert_eq!(refs[2].remote.as_deref(), Some("origin"));

        // Only the first slash separates the remote; the rest is the branch.
        assert_eq!(refs[3].name, "feature/deep/nest");
        assert_eq!(refs[3].remote.as_deref(), Some("origin"));
    }

    #[test]
    fn parse_refs_drops_the_remote_head_symref() {
        // `refs/remotes/origin/HEAD` stands for the remote's default branch.
        // Listing it would offer "origin/HEAD" as something to check out.
        let text = "refs/remotes/origin/HEAD\t\t1750000000\tInitial commit\n\
                    refs/remotes/origin/HEADroom\t\t1750000000\tNot the symref\n";
        let refs = parse_refs(text);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "HEADroom");
    }

    #[test]
    fn parse_refs_keeps_a_tab_inside_the_subject() {
        // The subject is the last field precisely so this cannot shift one.
        let text = "refs/heads/main\t\t1750000000\tfix:\tindent the thing\n";
        let refs = parse_refs(text);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].subject.as_deref(), Some("fix:\tindent the thing"));
    }

    #[test]
    fn parse_refs_ignores_refs_that_are_not_branches() {
        let text = "refs/tags/v1.0\t\t1750000000\tRelease\n\
                    refs/stash\t\t1750000000\tWIP\n";
        assert!(parse_refs(text).is_empty());
    }

    #[test]
    fn parses_worktree_list() {
        let text = "\
worktree /repo
HEAD f3bf41c
branch refs/heads/main

worktree /repo/wtA
HEAD f3bf41c
branch refs/heads/feature

worktree /repo/detached
HEAD abc
detached
locked needs review

worktree /repo/ghost
HEAD def
branch refs/heads/ghost
prunable gitdir file points to non-existent location
";
        let wts = parse_worktrees(text);
        assert_eq!(wts.len(), 4);
        assert_eq!(wts[0].path, PathBuf::from("/repo"));
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert_eq!(wts[0].head.as_deref(), Some("f3bf41c"));
        assert_eq!(wts[1].branch.as_deref(), Some("feature"));
        assert!(wts[2].detached);
        assert!(wts[2].locked);
        assert_eq!(wts[2].branch, None);
        assert!(wts[3].prunable);
        assert_eq!(wts[3].branch.as_deref(), Some("ghost"));
    }
}
