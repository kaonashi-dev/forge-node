//! The stopped sequencer: an in-progress rebase, merge, cherry-pick or revert,
//! the paths it left unmerged, and the three ways out (§14).
//!
//! Everything here is local — `rebase --continue` talks to the object store,
//! never to a socket — so it all runs on [`run_git`] and the ADR-008 budget.
//! The one non-obvious hazard is the editor: `--continue` opens `$EDITOR` for
//! the commit message it already has, and with stdin closed that editor would
//! hang until the timeout killed it. [`crate::command`] pins `GIT_EDITOR` and
//! `GIT_SEQUENCE_EDITOR` to `true` for exactly this reason.
//!
//! The state is **read**, never remembered: which paths are still unmerged is a
//! question only the index can answer, and a file the user (or an agent) just
//! staged has to stop being a conflict on the very next read.

use crate::command::{git_path, run_git, GitError};
use std::path::Path;

/// Cap on how many conflicted paths one read reports. A rebase that stops with
/// more than this is a rebase nobody resolves file by file, and the list is
/// flagged [`SequencerState::truncated`] rather than silently cut.
pub const MAX_CONFLICTS: usize = 200;

/// Ceiling on the small bookkeeping files git writes under `rebase-merge/`
/// (`msgnum`, `end`, `head-name`, `onto`). They hold one short line; anything
/// larger is not the file we think it is, and is not read at all.
const MAX_STATE_FILE_BYTES: u64 = 4 * 1024;

/// The porcelain-v1 status codes that mean "unmerged".
///
/// Both columns are meaningful here, unlike [`crate::diff::classify`]: `DU` and
/// `UD` are different questions for whoever resolves them.
const UNMERGED_CODES: [&str; 7] = ["DD", "AU", "UD", "UA", "DU", "AA", "UU"];

/// Which sequencer operation is stopped in the worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sequencer {
    /// `git rebase`, either backend (`rebase-merge/` or `rebase-apply/`).
    Rebase,
    /// `git merge` stopped on conflicts (`MERGE_HEAD`).
    Merge,
    /// `git cherry-pick` (`CHERRY_PICK_HEAD`).
    CherryPick,
    /// `git revert` (`REVERT_HEAD`).
    Revert,
}

impl Sequencer {
    /// The subcommand that continues or aborts this operation.
    #[must_use]
    pub fn subcommand(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }
}

/// One path git reports as unmerged, with the `XY` pair that says how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// Path relative to the worktree root, exactly as git spells it.
    pub path: String,
    /// The porcelain `XY` code (`UU`, `DU`, `AA`, …).
    pub code: String,
}

/// What the worktree's sequencer looks like right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SequencerState {
    /// The operation in progress, or `None` when the tree is not mid-anything.
    pub operation: Option<Sequencer>,
    /// Short object id of `HEAD` — detached during a rebase, which is exactly
    /// what the GUI header shows.
    pub head: Option<String>,
    /// The branch being replayed (`rebase-merge/head-name`), falling back to
    /// the checked-out branch when nothing is in progress.
    pub branch: Option<String>,
    /// Where the replay lands: the short id in `rebase-merge/onto`.
    pub onto: Option<String>,
    /// Which commit of the replay stopped, 1-based.
    pub step: Option<u32>,
    /// How many commits the replay has in total.
    pub total: Option<u32>,
    /// Unmerged paths, capped at [`MAX_CONFLICTS`].
    pub conflicts: Vec<Conflict>,
    /// Whether the conflict list was cut at the cap.
    pub truncated: bool,
}

impl SequencerState {
    /// Whether a rebase, merge, cherry-pick or revert is stopped here.
    #[must_use]
    pub fn in_progress(&self) -> bool {
        self.operation.is_some()
    }
}

/// What `--continue` did.
///
/// Both variants carry the state read back afterwards, because the caller has
/// to redraw either way and a second read would be a second `status` for an
/// answer this one already has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Continued {
    /// The operation ran to the end; nothing is in progress any more.
    Finished(SequencerState),
    /// It stopped again — on the next commit's conflicts, or because the ones
    /// on screen are still unstaged. Carries git's own words about why.
    Stopped {
        /// The state read back after the attempt.
        state: SequencerState,
        /// Git's message, trimmed. Empty when git said nothing useful.
        message: String,
    },
}

impl Continued {
    /// The state read back after the attempt, whichever way it went.
    #[must_use]
    pub fn state(&self) -> &SequencerState {
        match self {
            Self::Finished(state) | Self::Stopped { state, .. } => state,
        }
    }
}

/// Read the sequencer state of the worktree at `path`.
///
/// Cheap enough to answer a panel opening: one `rev-parse` per candidate git
/// path, one `status`, and a handful of small file reads. It never writes.
///
/// # Errors
/// [`GitError::CommandFailed`] when `git status` cannot describe the tree.
pub fn state(path: &Path) -> Result<SequencerState, GitError> {
    let operation = detect(path);
    let mut state = SequencerState {
        operation,
        head: short_head(path),
        branch: crate::repository::current_branch(path).unwrap_or(None),
        ..SequencerState::default()
    };

    if operation.is_none() {
        // Nothing is stopped: the index cannot hold an unmerged entry, so the
        // `status` scan below would be a subprocess spent proving it.
        return Ok(state);
    }

    let (conflicts, truncated) = unmerged(path)?;
    state.conflicts = conflicts;
    state.truncated = truncated;

    if operation == Some(Sequencer::Rebase) {
        read_rebase_progress(path, &mut state);
    }
    Ok(state)
}

/// Continue the stopped operation in the worktree at `path`.
///
/// Git refuses while anything is still unmerged, and that refusal is *not* an
/// error here: it is the answer the panel shows, so it comes back as
/// [`Continued::Stopped`] with git's message rather than as an `Err`.
///
/// # Errors
/// [`GitError::CommandFailed`] when nothing is in progress, or git could not
/// be run at all.
pub fn continue_sequencer(path: &Path) -> Result<Continued, GitError> {
    let operation = detect(path).ok_or_else(|| nothing_in_progress("--continue"))?;
    let args = [operation.subcommand(), "--continue"];
    let out = run_git(Some(path), &args)?;
    let after = state(path)?;

    if after.in_progress() {
        let message = if out.stderr.trim().is_empty() {
            out.stdout.trim().to_string()
        } else {
            out.stderr.trim().to_string()
        };
        tracing::debug!(target: "git", op = operation.subcommand(), "rebase.continue stopped");
        return Ok(Continued::Stopped {
            state: after,
            message,
        });
    }

    tracing::debug!(target: "git", op = operation.subcommand(), "rebase.continue finished");
    Ok(Continued::Finished(after))
}

/// Abort the stopped operation, restoring the branch git started from.
///
/// Destructive by nature — every resolution made since the operation stopped
/// goes with it — so the caller confirms before this is reached.
///
/// # Errors
/// [`GitError::CommandFailed`] when nothing is in progress or git refuses.
pub fn abort(path: &Path) -> Result<(), GitError> {
    let operation = detect(path).ok_or_else(|| nothing_in_progress("--abort"))?;
    let args = [operation.subcommand(), "--abort"];
    run_git(Some(path), &args)?.ok(&args)?;
    tracing::debug!(target: "git", op = operation.subcommand(), "rebase.abort");
    Ok(())
}

/// Stage `paths` as resolved (`git add --`), so `--continue` will accept them.
///
/// Git decides what "resolved" means: staging a file that still holds conflict
/// markers stages the markers, which is the same thing the command line does
/// and the same thing the next read reports back.
///
/// # Errors
/// [`GitError::CommandFailed`] when a path is not a plain worktree-relative
/// path, or when `git add` fails.
pub fn mark_resolved(path: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Err(GitError::CommandFailed {
            args: vec!["add".into()],
            status: -1,
            stderr: "no paths to stage".into(),
        });
    }
    for candidate in paths {
        validate_relative(candidate)?;
    }

    // `--` closes the option list; the validation above is what keeps a path
    // from climbing out of the checkout in the first place.
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    run_git(Some(path), &args)?.ok(&args)?;
    tracing::debug!(target: "git", count = paths.len(), "rebase.mark_resolved");
    Ok(())
}

/// Which operation, if any, is stopped in the worktree at `path`.
///
/// Order matters: a rebase that stops on a conflict also writes
/// `CHERRY_PICK_HEAD` under the merge backend, and reporting that as a
/// cherry-pick would offer the user `git cherry-pick --abort` for a rebase.
fn detect(path: &Path) -> Option<Sequencer> {
    if git_path(path, "rebase-merge").is_some() || git_path(path, "rebase-apply").is_some() {
        return Some(Sequencer::Rebase);
    }
    if git_path(path, "MERGE_HEAD").is_some() {
        return Some(Sequencer::Merge);
    }
    if git_path(path, "CHERRY_PICK_HEAD").is_some() {
        return Some(Sequencer::CherryPick);
    }
    if git_path(path, "REVERT_HEAD").is_some() {
        return Some(Sequencer::Revert);
    }
    None
}

fn short_head(path: &Path) -> Option<String> {
    let out = run_git(Some(path), &["rev-parse", "--short", "HEAD"]).ok()?;
    if !out.success() {
        return None;
    }
    let head = out.stdout_trimmed();
    (!head.is_empty()).then(|| head.to_string())
}

/// Unmerged paths from `status --porcelain=v1 -z`, capped at [`MAX_CONFLICTS`].
///
/// `-z` for the same reason as [`crate::diff`]: a path with a space, a quote or
/// a newline arrives NUL-delimited instead of shell-quoted, so nothing has to
/// be unescaped and no path is silently mangled.
fn unmerged(path: &Path) -> Result<(Vec<Conflict>, bool), GitError> {
    const ARGS: [&str; 6] = [
        "status",
        "--porcelain=v1",
        "--untracked-files=no",
        "--no-renames",
        "-z",
        "--",
    ];
    let out = run_git(Some(path), &ARGS)?.ok(&ARGS)?;

    let mut conflicts = Vec::new();
    let mut truncated = false;
    for record in out.stdout.split('\0') {
        // `XY <path>`: two status columns, a space, then the path.
        if record.len() < 4 {
            continue;
        }
        let (code, file) = record.split_at(3);
        let code = code[..2].to_string();
        if !UNMERGED_CODES.contains(&code.as_str()) {
            continue;
        }
        if conflicts.len() >= MAX_CONFLICTS {
            truncated = true;
            break;
        }
        conflicts.push(Conflict {
            path: file.to_string(),
            code,
        });
    }
    Ok((conflicts, truncated))
}

/// Fill in branch, onto and progress from the rebase's own state directory.
///
/// Best effort by design: the files differ between the merge and apply
/// backends and across git versions, and a missing counter is a missing
/// counter — never a reason to fail a read that already knows the conflicts.
fn read_rebase_progress(path: &Path, state: &mut SequencerState) {
    let Some(dir) = git_path(path, "rebase-merge").or_else(|| git_path(path, "rebase-apply"))
    else {
        return;
    };

    if let Some(name) = read_state_file(&dir.join("head-name")) {
        let branch = name
            .strip_prefix("refs/heads/")
            .unwrap_or(name.as_str())
            .to_string();
        if !branch.is_empty() && branch != "detached HEAD" {
            state.branch = Some(branch);
        }
    }
    if let Some(onto) = read_state_file(&dir.join("onto")) {
        // The file holds a full object id; the header shows it the way git's
        // own prompt does.
        state.onto = Some(onto.chars().take(7).collect());
    }
    // `msgnum`/`end` is the merge backend, `next`/`last` the apply one.
    state.step = read_state_file(&dir.join("msgnum"))
        .or_else(|| read_state_file(&dir.join("next")))
        .and_then(|text| text.parse().ok());
    state.total = read_state_file(&dir.join("end"))
        .or_else(|| read_state_file(&dir.join("last")))
        .and_then(|text| text.parse().ok());
}

/// Read one of git's one-line bookkeeping files, size-checked first.
///
/// `metadata()` before the read is the rule the whole repository follows: a
/// budget consulted after the allocation bounds the answer, not the peak.
fn read_state_file(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    if size == 0 || size > MAX_STATE_FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

fn nothing_in_progress(flag: &str) -> GitError {
    GitError::CommandFailed {
        args: vec![flag.to_string()],
        status: -1,
        stderr: "no rebase, merge, cherry-pick or revert is in progress".into(),
    }
}

/// Refuse anything that is not a plain path inside the checkout.
fn validate_relative(candidate: &str) -> Result<(), GitError> {
    let reason = if candidate.trim().is_empty() {
        Some("path is empty")
    } else if candidate.starts_with('-') {
        Some("path may not start with '-' (it would parse as a git option)")
    } else if Path::new(candidate).is_absolute() {
        Some("path must be relative to the worktree")
    } else if Path::new(candidate)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        Some("path may not climb out of the worktree")
    } else {
        None
    };

    match reason {
        None => Ok(()),
        Some(reason) => Err(GitError::CommandFailed {
            args: vec!["add".into(), candidate.to_string()],
            status: -1,
            stderr: format!("refusing to stage {candidate:?}: {reason}"),
        }),
    }
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

    fn git_allow_failure(repo: &Path, args: &[&str]) {
        let _ = Command::new("git").args(args).current_dir(repo).status();
    }

    /// A repository whose `topic` branch conflicts with `main` on `a.txt`.
    fn conflicting_repo(repo: &Path) {
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        git(repo, &["add", "a.txt"]);
        git(repo, &["commit", "-m", "base"]);

        git(repo, &["checkout", "-b", "topic"]);
        std::fs::write(repo.join("a.txt"), "topic\n").unwrap();
        git(repo, &["commit", "-am", "topic"]);

        git(repo, &["checkout", "main"]);
        std::fs::write(repo.join("a.txt"), "main\n").unwrap();
        git(repo, &["commit", "-am", "main"]);

        git(repo, &["checkout", "topic"]);
        // Conflicts, so it exits non-zero and leaves the rebase stopped.
        git_allow_failure(repo, &["rebase", "main"]);
    }

    #[test]
    fn a_clean_tree_has_no_operation() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(repo, &["add", "a.txt"]);
        git(repo, &["commit", "-m", "init"]);

        let state = state(repo).unwrap();
        assert!(!state.in_progress());
        assert!(state.conflicts.is_empty());
        assert_eq!(state.branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_stopped_rebase_reports_its_conflicts_and_progress() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        conflicting_repo(repo);

        let state = state(repo).unwrap();
        assert_eq!(state.operation, Some(Sequencer::Rebase));
        assert_eq!(state.conflicts.len(), 1);
        assert_eq!(state.conflicts[0].path, "a.txt");
        assert_eq!(state.conflicts[0].code, "UU");
        // The branch being replayed, not the detached HEAD it replays onto.
        assert_eq!(state.branch.as_deref(), Some("topic"));
        assert_eq!(state.step, Some(1));
        assert_eq!(state.total, Some(1));
        assert!(state.head.is_some());
    }

    #[test]
    fn continue_stops_again_while_paths_are_unmerged() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        conflicting_repo(repo);

        match continue_sequencer(repo).unwrap() {
            Continued::Stopped { state, message } => {
                assert_eq!(state.conflicts.len(), 1);
                assert!(!message.is_empty());
            }
            Continued::Finished(_) => panic!("continue must refuse an unmerged tree"),
        }
    }

    #[test]
    fn resolving_and_continuing_finishes_the_rebase() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        conflicting_repo(repo);

        std::fs::write(repo.join("a.txt"), "resolved\n").unwrap();
        mark_resolved(repo, &["a.txt".to_string()]).unwrap();

        // A staged file is no longer a conflict on the very next read.
        let staged = state(repo).unwrap();
        assert!(staged.in_progress());
        assert!(staged.conflicts.is_empty());

        let continued = continue_sequencer(repo).unwrap();
        assert!(matches!(continued, Continued::Finished(_)));
        assert!(!continued.state().in_progress());
        assert_eq!(continued.state().branch.as_deref(), Some("topic"));
    }

    #[test]
    fn abort_puts_the_branch_back() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        conflicting_repo(repo);

        abort(repo).unwrap();
        let after = state(repo).unwrap();
        assert!(!after.in_progress());
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "topic\n"
        );
    }

    #[test]
    fn continue_and_abort_refuse_when_nothing_is_in_progress() {
        let dir = tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(repo, &["add", "a.txt"]);
        git(repo, &["commit", "-m", "init"]);

        assert!(continue_sequencer(repo).is_err());
        assert!(abort(repo).is_err());
    }

    #[test]
    fn a_path_that_could_be_an_option_or_climb_out_is_refused() {
        let dir = tempdir().unwrap();
        assert!(mark_resolved(dir.path(), &["--force".to_string()]).is_err());
        assert!(mark_resolved(dir.path(), &["../outside.txt".to_string()]).is_err());
        assert!(mark_resolved(dir.path(), &["/etc/passwd".to_string()]).is_err());
        assert!(mark_resolved(dir.path(), &[]).is_err());
    }
}
