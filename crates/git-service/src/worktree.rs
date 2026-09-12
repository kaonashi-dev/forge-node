//! Managed worktree operations: slug generation, validation, create, remove
//! and removal pre-checks (§14.2, §14.3, §14.4).
//!
//! This crate never deletes a branch (§14.4) — [`remove`] only removes the
//! worktree working directory and prunes stale administrative entries.

use crate::command::{run_git, GitError};
use crate::repository::{list_branches, list_worktrees};
use std::collections::HashSet;
use std::path::Path;

/// Maximum slug length (§14.2).
const MAX_SLUG_LEN: usize = 64;

/// Removal pre-checks returned to the caller so the GUI can confirm (§14.4).
///
/// The daemon combines these with its own "running sessions in this workspace"
/// check before allowing a non-forced removal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemovePrechecks {
    /// `git status --porcelain` reported at least one change in the worktree.
    pub dirty: bool,
    /// A merge or rebase is currently in progress in the worktree.
    pub merge_or_rebase_in_progress: bool,
}

/// Slug used when the input contains nothing that can safely name a directory.
const FALLBACK_SLUG: &str = "worktree";

/// Turn a branch name into a filesystem-safe slug (§14.2).
///
/// Rules: `/` becomes `-`; any character outside `[A-Za-z0-9._-]` becomes `-`;
/// runs of `-` collapse to one; the result is capped at 64 characters. The
/// transformed string is always ASCII, so the length cap is also a char cap.
///
/// A slug is also a *path component*, so three results are rewritten to
/// [`FALLBACK_SLUG`] rather than returned as-is: the empty string, `.`, and
/// `..`. `.` and `-` are legal slug characters, so without this a caller-chosen
/// name of `".."` produced the slug `".."` and the worktree path
/// `<worktrees_root>/<project>/..` — which resolves to `<worktrees_root>`, i.e.
/// the directory holding every managed worktree of every project. A later
/// forced removal would then take all of them with it.
#[must_use]
pub fn slugify(branch: &str) -> String {
    let mut out = String::with_capacity(branch.len().min(MAX_SLUG_LEN));
    let mut prev_dash = false;
    for ch in branch.chars() {
        let mapped = if ch == '/' {
            '-'
        } else if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '-'
        };
        if mapped == '-' {
            if prev_dash {
                continue; // collapse repeated dashes
            }
            prev_dash = true;
        } else {
            prev_dash = false;
        }
        out.push(mapped);
        if out.len() >= MAX_SLUG_LEN {
            break;
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        return FALLBACK_SLUG.to_owned();
    }
    out
}

/// Produce a slug for `branch` that is not already present in `existing`.
///
/// The base slug is [`slugify`]; on collision a numeric suffix `-2`, `-3`, …
/// is appended until a free name is found (§14.2).
///
/// The 64-character cap of §14.2 applies to the *final* name, so the base is
/// shortened to make room for the suffix rather than pushed past the limit: a
/// 64-character branch slug that collides used to produce a 66-character
/// directory name.
#[must_use]
pub fn unique_slug(existing: &HashSet<String>, branch: &str) -> String {
    let base = slugify(branch);
    if !existing.contains(&base) {
        return base;
    }
    let mut n = 2u32;
    loop {
        let suffix = format!("-{n}");
        // `slugify` only ever emits ASCII, so truncating by bytes is safe.
        let keep = MAX_SLUG_LEN.saturating_sub(suffix.len());
        let mut stem = base[..base.len().min(keep)].to_owned();
        // Truncation can leave a trailing `-`, which would read as `foo--2`.
        while stem.ends_with('-') {
            stem.pop();
        }
        if stem.is_empty() {
            stem.push_str(FALLBACK_SLUG);
        }
        let candidate = format!("{stem}{suffix}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Reject a ref that git's option parser would read as a flag.
///
/// Every ref here reaches the CLI as an argument. `git worktree add` takes
/// options both before and after its positionals, so a caller-supplied value
/// like `--force` or `-B` would be consumed as an option rather than as the ref
/// it is meant to be. Callers pass `--` where git accepts it (see [`create`]),
/// and this guard covers the positions where it does not.
fn reject_option_like(what: &str, value: &str) -> Result<(), GitError> {
    if value.starts_with('-') {
        return Err(GitError::InvalidBranchName {
            branch: value.to_string(),
            reason: format!("{what} may not start with '-' (it would parse as a git option)"),
        });
    }
    Ok(())
}

/// Validate a branch name with `git check-ref-format --branch` (§14.3).
///
/// A leading `-` is rejected before git ever sees the value: `check-ref-format`
/// would parse `--help` as its own option and exit 0, letting an option-looking
/// "branch name" pass validation.
///
/// # Errors
/// [`GitError::InvalidBranchName`] if the name looks like an option or git
/// rejects it.
pub fn validate_branch_name(branch: &str) -> Result<(), GitError> {
    reject_option_like("branch name", branch)?;
    let out = run_git(None, &["check-ref-format", "--branch", branch])?;
    if out.success() {
        Ok(())
    } else {
        Err(GitError::InvalidBranchName {
            branch: branch.to_string(),
            reason: out.stderr.trim().to_string(),
        })
    }
}

/// Create a worktree at `path` for `branch` (§14.3).
///
/// Behaviour:
/// - branch does **not** exist ⇒ `worktree add <path> -b <branch> <base>`
///   (`base` defaults to `HEAD`);
/// - branch exists and is not checked out elsewhere ⇒ `worktree add <path> <branch>`;
/// - branch exists but is checked out in another worktree ⇒ [`GitError::Conflict`]
///   carrying that worktree's path.
///
/// The branch name is validated first. This never creates the parent directory
/// or sets permissions — the daemon owns location and `0700` mode (§14.2).
///
/// # Errors
/// - [`GitError::InvalidBranchName`] for an invalid branch name.
/// - [`GitError::Conflict`] if the branch is checked out elsewhere.
/// - [`GitError::CommandFailed`] if `git worktree add` fails.
/// - [`GitError::NonUtf8Path`] if `path` is not valid UTF-8.
pub fn create(repo: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<(), GitError> {
    validate_branch_name(branch)?;
    // `base` is caller-supplied and reaches git as a bare argument; unlike
    // `branch` it never went through `check-ref-format`.
    if let Some(base) = base {
        reject_option_like("base ref", base)?;
    }
    let path_str = path_to_str(path)?;

    let branch_exists = list_branches(repo)?.iter().any(|b| b == branch);

    if branch_exists {
        // Reject if the branch is checked out in a live worktree. A stale entry
        // (`prunable`, or a directory deleted by hand) is still listed by git
        // and still holds the branch, but the checkout it names is gone: prune
        // the dead bookkeeping, then add. Without this, a worktree deleted
        // outside the app could never be recreated on its branch.
        let entries = list_worktrees(repo)?;
        if let Some(existing) = entries
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch) && !w.is_stale())
        {
            return Err(GitError::Conflict {
                branch: branch.to_string(),
                path: existing.path.clone(),
            });
        }
        if entries.iter().any(|w| w.branch.as_deref() == Some(branch)) {
            prune(repo)?;
        }
        // `--` ends option parsing so the positionals cannot be read as flags.
        let args = ["worktree", "add", "--", path_str, branch];
        run_git(Some(repo), &args)?.ok(&args)?;
    } else {
        let base_ref = base.unwrap_or("HEAD");
        let args = ["worktree", "add", "-b", branch, "--", path_str, base_ref];
        run_git(Some(repo), &args)?.ok(&args)?;
    }

    tracing::debug!(target: "git", branch, "worktree.create");
    Ok(())
}

/// Drop administrative entries whose working directory is gone (§14.4).
///
/// Never touches a working directory; only `.git/worktrees/` bookkeeping is
/// removed, which is exactly what a hand-deleted worktree leaves behind.
///
/// # Errors
/// [`GitError::CommandFailed`] if `git worktree prune` fails.
pub fn prune(repo: &Path) -> Result<(), GitError> {
    const PRUNE: [&str; 2] = ["worktree", "prune"];
    run_git(Some(repo), &PRUNE)?.ok(&PRUNE)?;
    Ok(())
}

/// Remove the worktree at `path`, then prune stale administrative entries (§14.4).
///
/// Never deletes the branch. With `force = false` git itself refuses to remove a
/// dirty worktree; callers should consult [`precheck_remove`] first.
///
/// A `path` that no longer exists on disk is not an error: the removal reduces
/// to the prune, which is what clears git's leftover bookkeeping for it.
///
/// # Errors
/// - [`GitError::CommandFailed`] if `git worktree remove`/`prune` fails.
/// - [`GitError::NonUtf8Path`] if `path` is not valid UTF-8.
pub fn remove(repo: &Path, path: &Path, force: bool) -> Result<(), GitError> {
    let path_str = path_to_str(path)?;

    // `git worktree remove` refuses a path that is no longer on disk ("is not
    // a working tree"), so a directory deleted outside the app could never be
    // cleaned up through this call. Pruning alone is the whole repair in that
    // case: the administrative entry under `.git/worktrees/` is all that is
    // left of it, and that is exactly what `prune` drops.
    if path.exists() {
        let mut args: Vec<&str> = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push("--");
        args.push(path_str);
        run_git(Some(repo), &args)?.ok(&args)?;
    }

    prune(repo)?;

    tracing::debug!(target: "git", force, "worktree.remove");
    Ok(())
}

/// Compute removal pre-checks for the worktree at `path` (§14.4).
///
/// Reports whether the worktree is dirty (`status --porcelain`) and whether a
/// merge or rebase is in progress (presence of `MERGE_HEAD`, `rebase-merge/` or
/// `rebase-apply/` in the worktree's git dir). The daemon adds its own running-
/// session check on top of this.
///
/// A `path` that no longer exists reports neither: nothing blocks the removal
/// of a worktree that is already gone.
///
/// # Errors
/// [`GitError::CommandFailed`] if `git status` fails in the worktree.
pub fn precheck_remove(repo: &Path, path: &Path) -> Result<RemovePrechecks, GitError> {
    // A worktree directory deleted outside the app has nothing left to check:
    // there is no working tree to be dirty and no sequencer state to be
    // mid-replay. `git status` there fails with "cannot change to <path>",
    // which used to reach the GUI as a `GitError` and made the one action that
    // could still clean the workspace up — removing it — the one action that
    // could never succeed.
    if !path.exists() {
        return Ok(RemovePrechecks::default());
    }

    // Run status inside the worktree itself.
    const ARGS: [&str; 2] = ["status", "--porcelain"];
    let out = run_git(Some(path), &ARGS)?.ok(&ARGS)?;
    let dirty = out.stdout.lines().any(|l| !l.trim().is_empty());

    let merge_or_rebase_in_progress = git_path_exists(path, "MERGE_HEAD")
        || git_path_exists(path, "rebase-merge")
        || git_path_exists(path, "rebase-apply");

    // `repo` is part of the signature for symmetry with the other operations and
    // future use; the checks only need the worktree path.
    let _ = repo;

    Ok(RemovePrechecks {
        dirty,
        merge_or_rebase_in_progress,
    })
}

/// Whether the git-managed file/dir named `name` exists for the worktree at
/// `dir`, resolved via [`crate::command::git_path`].
fn git_path_exists(dir: &Path, name: &str) -> bool {
    crate::command::git_path(dir, name).is_some()
}

/// Convert a path to `&str` for the CLI, erroring on non-UTF-8 input.
fn path_to_str(path: &Path) -> Result<&str, GitError> {
    path.to_str()
        .ok_or_else(|| GitError::NonUtf8Path(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_replaces_slashes_and_specials() {
        assert_eq!(slugify("feature/auth"), "feature-auth");
        assert_eq!(slugify("feat/JIRA-123_fix.v2"), "feat-JIRA-123_fix.v2");
        assert_eq!(slugify("hello world!"), "hello-world-");
    }

    #[test]
    fn slugify_collapses_repeated_dashes() {
        assert_eq!(slugify("a///b"), "a-b");
        assert_eq!(slugify("a  --  b"), "a-b");
        assert_eq!(slugify("weird@@@name"), "weird-name");
    }

    #[test]
    fn slugify_maps_non_ascii_to_dash() {
        // 'é' is outside [A-Za-z0-9._-] and becomes a single dash.
        assert_eq!(slugify("caf\u{e9}"), "caf-");
        assert_eq!(slugify("\u{e9}\u{e9}\u{e9}"), "-");
    }

    #[test]
    fn slugify_caps_at_64_chars() {
        let long = "a".repeat(200);
        let slug = slugify(&long);
        assert_eq!(slug.len(), 64);
        assert!(slug.chars().all(|c| c == 'a'));
    }

    #[test]
    fn slugify_never_returns_a_path_traversing_component() {
        // `.` and `-` survive slugification, so these would otherwise pass
        // through verbatim and escape the worktrees root when joined as a path.
        assert_eq!(slugify(".."), FALLBACK_SLUG);
        assert_eq!(slugify("."), FALLBACK_SLUG);
        assert_eq!(slugify(""), FALLBACK_SLUG);
        // `/` still maps to `-`, so deeper traversal was never possible.
        assert_eq!(slugify("../.."), "..-..");
    }

    #[test]
    fn validate_branch_name_rejects_option_like_names() {
        let err = validate_branch_name("--help").unwrap_err();
        assert!(matches!(err, GitError::InvalidBranchName { .. }));
        // Without the guard `git check-ref-format --branch --help` exits 0.
        let err = validate_branch_name("-b").unwrap_err();
        assert!(matches!(err, GitError::InvalidBranchName { .. }));
    }

    #[test]
    fn unique_slug_returns_base_when_free() {
        let existing = HashSet::new();
        assert_eq!(unique_slug(&existing, "feature/auth"), "feature-auth");
    }

    #[test]
    fn unique_slug_appends_numeric_suffix_on_collision() {
        let mut existing = HashSet::new();
        existing.insert("feature-auth".to_string());
        assert_eq!(unique_slug(&existing, "feature/auth"), "feature-auth-2");

        existing.insert("feature-auth-2".to_string());
        existing.insert("feature-auth-3".to_string());
        assert_eq!(unique_slug(&existing, "feature/auth"), "feature-auth-4");
    }

    #[test]
    fn unique_slug_keeps_the_64_char_cap_when_adding_a_suffix() {
        // The cap of §14.2 is on the directory name that actually gets created,
        // so the suffix has to fit inside it, not extend past it.
        let base = slugify(&"a".repeat(200));
        assert_eq!(base.len(), MAX_SLUG_LEN);

        let mut existing = HashSet::new();
        existing.insert(base.clone());
        let next = unique_slug(&existing, &"a".repeat(200));
        assert_eq!(next.len(), MAX_SLUG_LEN);
        assert!(next.ends_with("-2"));
        assert_ne!(next, base);

        // A longer suffix eats one more character of the stem.
        for n in 2..=10 {
            existing.insert(format!("{}-{n}", &base[..MAX_SLUG_LEN - 2]));
        }
        let eleventh = unique_slug(&existing, &"a".repeat(200));
        assert!(eleventh.len() <= MAX_SLUG_LEN, "got {eleventh:?}");
    }

    #[test]
    fn unique_slug_does_not_produce_a_double_dash_after_truncation() {
        // A stem whose 62nd character is a `-` must not become `...--2`.
        let branch = format!("{}/{}", "b".repeat(61), "c".repeat(20));
        let base = slugify(&branch);
        assert_eq!(base.len(), MAX_SLUG_LEN);

        let mut existing = HashSet::new();
        existing.insert(base);
        let next = unique_slug(&existing, &branch);
        assert!(!next.contains("--"), "got {next:?}");
        assert!(next.len() <= MAX_SLUG_LEN);
    }
}
