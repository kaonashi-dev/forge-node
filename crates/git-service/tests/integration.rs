//! Integration tests against **real** temporary git repositories (§21: never
//! mock git). Every test that touches git skips gracefully when the `git`
//! binary is not available so the suite stays green in minimal environments.

use git_service::command::run_git;
use git_service::{
    create, current_branch, default_remote, discover_root, fetch, has_remote, list_branches,
    list_refs, list_remotes, list_worktrees, precheck_remove, remove, status, GitError,
};
use std::path::Path;
use tempfile::{tempdir, TempDir};

/// Whether a usable `git` is on PATH.
fn git_available() -> bool {
    run_git(None, &["--version"])
        .map(|o| o.success())
        .unwrap_or(false)
}

/// Skip the current test (returning early) when git is unavailable.
macro_rules! require_git {
    () => {
        if !git_available() {
            eprintln!("skipping: `git` not available in this environment");
            return;
        }
    };
}

/// Run a setup git command, panicking on any failure.
fn setup(repo: &Path, args: &[&str]) {
    run_git(Some(repo), args)
        .unwrap_or_else(|e| panic!("git {args:?} errored: {e}"))
        .ok(args)
        .unwrap_or_else(|e| panic!("git {args:?} failed: {e}"));
}

/// Create a temp repo on `main` with one commit and a configured local identity.
fn init_repo() -> TempDir {
    let dir = tempdir().unwrap();
    let p = dir.path();
    setup(p, &["-c", "init.defaultBranch=main", "init"]);
    setup(p, &["config", "user.email", "test@example.com"]);
    setup(p, &["config", "user.name", "Forge Test"]);
    std::fs::write(p.join("README.md"), "hello\n").unwrap();
    setup(p, &["add", "."]);
    setup(p, &["-c", "commit.gpgsign=false", "commit", "-m", "init"]);
    dir
}

fn canon(p: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap()
}

#[test]
fn discover_root_finds_toplevel_from_subdir() {
    require_git!();
    let repo = init_repo();
    let sub = repo.path().join("nested/dir");
    std::fs::create_dir_all(&sub).unwrap();

    let root = discover_root(&sub).unwrap();
    assert_eq!(canon(&root), canon(repo.path()));
}

#[test]
fn discover_root_errors_outside_a_repo() {
    require_git!();
    let plain = tempdir().unwrap();
    match discover_root(plain.path()) {
        Err(GitError::NotAGitRepo(_)) => {}
        other => panic!("expected NotAGitRepo, got {other:?}"),
    }
}

#[test]
fn current_branch_is_main_after_commit() {
    require_git!();
    let repo = init_repo();
    assert_eq!(
        current_branch(repo.path()).unwrap().as_deref(),
        Some("main")
    );
}

#[test]
fn status_is_clean_then_dirty_after_edit() {
    require_git!();
    let repo = init_repo();

    let clean = status(repo.path()).unwrap();
    assert_eq!(clean.branch.as_deref(), Some("main"));
    assert!(!clean.dirty, "fresh repo should be clean");

    std::fs::write(repo.path().join("README.md"), "changed\n").unwrap();
    let dirty = status(repo.path()).unwrap();
    assert!(dirty.dirty, "edited repo should be dirty");

    // An untracked file also counts as dirty.
    std::fs::write(repo.path().join("new.txt"), "x\n").unwrap();
    assert!(status(repo.path()).unwrap().dirty);
}

#[test]
fn list_branches_reflects_created_branches() {
    require_git!();
    let repo = init_repo();
    setup(repo.path(), &["branch", "feature"]);

    let branches = list_branches(repo.path()).unwrap();
    assert!(branches.iter().any(|b| b == "main"));
    assert!(branches.iter().any(|b| b == "feature"));
}

#[test]
fn create_new_branch_worktree_then_list_and_remove() {
    require_git!();
    let repo = init_repo();
    let wt_root = tempdir().unwrap();
    let wt_path = wt_root.path().join("wt-feature");

    // Branch does not exist yet -> `worktree add <path> -b feature-x HEAD`.
    create(repo.path(), &wt_path, "feature-x", None).unwrap();

    // The new branch exists and the worktree is listed with the right branch.
    // Capture the canonical path now, while the directory still exists.
    let wt_canon = canon(&wt_path);
    assert!(list_branches(repo.path())
        .unwrap()
        .iter()
        .any(|b| b == "feature-x"));
    let listed = list_worktrees(repo.path()).unwrap();
    let entry = listed
        .iter()
        .find(|w| canon(&w.path) == wt_canon)
        .expect("new worktree should be listed");
    assert_eq!(entry.branch.as_deref(), Some("feature-x"));
    assert!(entry.head.is_some());

    // Pre-checks: clean immediately after creation.
    let pre = precheck_remove(repo.path(), &wt_path).unwrap();
    assert!(!pre.dirty);
    assert!(!pre.merge_or_rebase_in_progress);

    // Make it dirty and confirm the pre-check flags it.
    std::fs::write(wt_path.join("scratch.txt"), "wip\n").unwrap();
    assert!(precheck_remove(repo.path(), &wt_path).unwrap().dirty);

    // Force-remove (it is dirty) and confirm it disappears from the listing.
    remove(repo.path(), &wt_path, true).unwrap();
    let after = list_worktrees(repo.path()).unwrap();
    assert!(after.iter().all(|w| canon(&w.path) != wt_canon));

    // Removing a worktree never deletes the branch (§14.4).
    assert!(list_branches(repo.path())
        .unwrap()
        .iter()
        .any(|b| b == "feature-x"));
}

/// A worktree directory deleted outside the app must still be removable.
///
/// `git status` cannot run in a directory that is gone and `git worktree
/// remove` refuses a path that is not a working tree, so before this both
/// halves of the removal failed and the workspace was stuck in the rail.
#[test]
fn remove_worktree_whose_directory_was_deleted_by_hand() {
    require_git!();
    let repo = init_repo();
    let wt_root = tempdir().unwrap();
    let wt_path = wt_root.path().join("wt-vanished");

    create(repo.path(), &wt_path, "vanished", None).unwrap();
    let wt_canon = canon(&wt_path);
    std::fs::remove_dir_all(&wt_path).unwrap();

    // Nothing is dirty and nothing is mid-replay in a directory that is gone.
    let pre = precheck_remove(repo.path(), &wt_path).unwrap();
    assert!(!pre.dirty);
    assert!(!pre.merge_or_rebase_in_progress);

    // The unforced removal is the one the GUI issues first, and it succeeds:
    // pruning is the whole repair once the directory is gone.
    remove(repo.path(), &wt_path, false).unwrap();
    let after = list_worktrees(repo.path()).unwrap();
    assert!(after.iter().all(|w| canon(&w.path) != wt_canon));

    // Still never the branch (§14.4).
    assert!(list_branches(repo.path())
        .unwrap()
        .iter()
        .any(|b| b == "vanished"));
}

#[test]
fn create_existing_branch_worktree() {
    require_git!();
    let repo = init_repo();
    setup(repo.path(), &["branch", "solo"]);

    let wt_root = tempdir().unwrap();
    let wt_path = wt_root.path().join("wt-solo");

    // Branch exists and is not checked out anywhere -> `worktree add <path> solo`.
    create(repo.path(), &wt_path, "solo", None).unwrap();
    let listed = list_worktrees(repo.path()).unwrap();
    assert!(listed
        .iter()
        .any(|w| w.branch.as_deref() == Some("solo") && canon(&w.path) == canon(&wt_path)));
}

#[test]
fn create_conflicts_when_branch_checked_out_elsewhere() {
    require_git!();
    let repo = init_repo();
    let wt_root = tempdir().unwrap();
    let wt_path = wt_root.path().join("wt-main");

    // `main` is already checked out at the repo root.
    match create(repo.path(), &wt_path, "main", None) {
        Err(GitError::Conflict { branch, path }) => {
            assert_eq!(branch, "main");
            assert_eq!(canon(&path), canon(repo.path()));
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[test]
fn validate_branch_name_rejects_bad_names() {
    require_git!();
    // Spaces and control-ish sequences are invalid ref names.
    match git_service::validate_branch_name("bad name") {
        Err(GitError::InvalidBranchName { .. }) => {}
        other => panic!("expected InvalidBranchName, got {other:?}"),
    }
    // A normal name validates.
    git_service::validate_branch_name("feature/ok").unwrap();
}

// ---- ADR-008 subprocess contract -------------------------------------------

/// Every invocation must reach `git` with a stable locale and interactive
/// prompting disabled. A `!` alias is the only way to observe the child's own
/// environment, so the assertion is made from inside the process git spawns.
#[test]
fn the_child_runs_with_a_stable_locale_and_no_terminal_prompt() {
    require_git!();
    let repo = init_repo();
    let out = run_git(
        Some(repo.path()),
        &[
            "-c",
            "alias.forgeenv=!printf '%s|%s\\n' \"$LC_ALL\" \"$GIT_TERMINAL_PROMPT\"",
            "forgeenv",
        ],
    )
    .unwrap();
    assert!(out.success(), "alias failed: {}", out.stderr);
    assert_eq!(
        out.stdout_trimmed(),
        "C|0",
        "ADR-008 requires LC_ALL=C and GIT_TERMINAL_PROMPT=0 in the child"
    );
}

/// A non-zero exit is a normal result, not an `Err`: `discover_root` and
/// friends read the status to decide what it means.
#[test]
fn a_failed_command_is_returned_as_output_not_an_error() {
    require_git!();
    let repo = init_repo();
    let args = ["rev-parse", "--verify", "refs/heads/no-such-branch"];
    let out = run_git(Some(repo.path()), &args).expect("a non-zero exit is still a result");

    assert!(!out.success());
    assert!(out.stdout_trimmed().is_empty());
    assert!(!out.stderr.trim().is_empty(), "git should explain itself");

    // Turning that into an error is the caller's choice.
    match out.ok(&args) {
        Err(GitError::CommandFailed { status, .. }) => assert_ne!(status, 0),
        other => panic!("expected CommandFailed, got {other:?}"),
    }
}

#[test]
fn stdout_and_stderr_are_captured_separately() {
    require_git!();
    let repo = init_repo();
    let out = run_git(
        Some(repo.path()),
        &[
            "-c",
            "alias.forgeboth=!printf to-stdout; printf to-stderr >&2",
            "forgeboth",
        ],
    )
    .unwrap();
    assert_eq!(out.stdout_trimmed(), "to-stdout");
    assert_eq!(out.stderr.trim(), "to-stderr");
    assert!(out.success());
}

/// `-C` is what makes one `run_git` helper usable across repositories: the same
/// arguments must answer for the directory they were given, not the daemon's.
#[test]
fn the_repo_argument_routes_the_command_into_that_working_tree() {
    require_git!();
    let repo = init_repo();
    let elsewhere = tempdir().unwrap();
    let args = ["rev-parse", "--is-inside-work-tree"];

    let inside = run_git(Some(repo.path()), &args).unwrap();
    assert!(inside.success());
    assert_eq!(inside.stdout_trimmed(), "true");

    let outside = run_git(Some(elsewhere.path()), &args).unwrap();
    assert!(
        !outside.success(),
        "a bare temp dir must not answer as a work tree"
    );
}

// ---------------------------------------------------------------------------
// Refs and remotes (§14.3, branches plan phases 3 and 4)
// ---------------------------------------------------------------------------

/// A second repository that `origin` can point at, so the fetch path is
/// exercised for real without a network. A local path is a first-class git
/// transport, so this is the genuine `git fetch` code path, not a stand-in.
fn init_origin_with(branches: &[&str]) -> TempDir {
    let dir = init_repo();
    for branch in branches {
        setup(dir.path(), &["checkout", "-b", branch]);
        std::fs::write(
            dir.path().join(format!("{}.txt", branch.replace('/', "-"))),
            "x\n",
        )
        .unwrap();
        setup(dir.path(), &["add", "."]);
        setup(
            dir.path(),
            &["-c", "commit.gpgsign=false", "commit", "-m", "work"],
        );
    }
    setup(dir.path(), &["checkout", "main"]);
    dir
}

#[test]
fn list_refs_reports_local_branches_with_upstream_and_date() {
    require_git!();
    let repo = init_repo();
    setup(repo.path(), &["branch", "feature/auth"]);

    let refs = list_refs(repo.path()).unwrap();
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"main"), "got {names:?}");
    assert!(names.contains(&"feature/auth"), "got {names:?}");

    // Everything is local here, so nothing carries a remote.
    assert!(refs.iter().all(|r| r.remote.is_none()));
    // A real commit always has a date; this is the sort key of the picker.
    assert!(refs.iter().all(|r| r.committed_at.is_some()));
    assert!(refs.iter().all(|r| r.subject.is_some()));
}

#[test]
fn list_refs_is_sorted_newest_commit_first() {
    require_git!();
    let repo = init_repo();
    // A second commit on a new branch is strictly newer than main's tip.
    setup(repo.path(), &["checkout", "-b", "later"]);
    std::fs::write(repo.path().join("later.txt"), "x\n").unwrap();
    setup(repo.path(), &["add", "."]);
    setup(
        repo.path(),
        &["-c", "commit.gpgsign=false", "commit", "-m", "later"],
    );

    let refs = list_refs(repo.path()).unwrap();
    assert_eq!(
        refs.first().map(|r| r.name.as_str()),
        Some("later"),
        "newest commit must sort first: {refs:?}"
    );
}

#[test]
fn a_repo_without_a_remote_reports_none() {
    require_git!();
    let repo = init_repo();
    assert!(!has_remote(repo.path()).unwrap());
    assert_eq!(default_remote(repo.path()).unwrap(), None);
    assert!(list_remotes(repo.path()).unwrap().is_empty());
}

#[test]
fn default_remote_prefers_origin_but_accepts_the_only_one() {
    require_git!();
    let origin = init_repo();
    let repo = init_repo();

    // A single, oddly-named remote is still the one to fetch.
    setup(
        repo.path(),
        &["remote", "add", "upstream", origin.path().to_str().unwrap()],
    );
    assert_eq!(
        default_remote(repo.path()).unwrap().as_deref(),
        Some("upstream")
    );

    // With `origin` present it wins regardless of configuration order.
    setup(
        repo.path(),
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );
    assert_eq!(
        default_remote(repo.path()).unwrap().as_deref(),
        Some("origin")
    );
    assert!(has_remote(repo.path()).unwrap());
}

#[test]
fn fetch_brings_down_remote_branches_that_list_refs_then_reports() {
    require_git!();
    let origin = init_origin_with(&["feature/remote-only"]);
    let repo = init_repo();
    setup(
        repo.path(),
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );

    // Before the fetch the branch simply does not exist here — this is exactly
    // why the picker cannot offer it without a fetch.
    let before = list_refs(repo.path()).unwrap();
    assert!(!before.iter().any(|r| r.name == "feature/remote-only"));

    let outcome = fetch(repo.path(), "origin", None).unwrap();
    assert_eq!(outcome.remote, "origin");

    let after = list_refs(repo.path()).unwrap();
    let remote_ref = after
        .iter()
        .find(|r| r.name == "feature/remote-only")
        .expect("the remote branch should be listed after a fetch");
    assert_eq!(remote_ref.remote.as_deref(), Some("origin"));
    // A remote-tracking branch is an upstream; it does not have one.
    assert_eq!(remote_ref.upstream, None);
}

#[test]
fn fetch_prunes_a_branch_deleted_upstream() {
    require_git!();
    let origin = init_origin_with(&["feature/doomed"]);
    let repo = init_repo();
    setup(
        repo.path(),
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );
    fetch(repo.path(), "origin", None).unwrap();
    assert!(list_refs(repo.path())
        .unwrap()
        .iter()
        .any(|r| r.name == "feature/doomed"));

    setup(origin.path(), &["branch", "-D", "feature/doomed"]);
    fetch(repo.path(), "origin", None).unwrap();

    // The stale row is gone: this is the whole reason `--prune` is on.
    assert!(!list_refs(repo.path())
        .unwrap()
        .iter()
        .any(|r| r.name == "feature/doomed"));
}

#[test]
fn fetch_reports_a_bad_remote_as_a_command_failure_with_stderr() {
    require_git!();
    let repo = init_repo();
    setup(
        repo.path(),
        &["remote", "add", "origin", "/definitely/not/a/repository"],
    );

    let err = fetch(
        repo.path(),
        "origin",
        Some(std::time::Duration::from_secs(30)),
    )
    .unwrap_err();
    match err {
        // The captured stderr is what the GUI shows; a timeout here would mean
        // the batch-mode hardening failed.
        GitError::CommandFailed { stderr, .. } => {
            assert!(!stderr.is_empty(), "stderr should carry git's own message");
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
}

#[test]
fn a_worktree_created_from_a_remote_ref_tracks_it() {
    // The load-bearing claim of the plan: `create` needs no `--track`, because
    // starting from a remote-tracking branch makes git configure the upstream
    // itself (`branch.autoSetupMerge`, on by default).
    require_git!();
    let origin = init_origin_with(&["feature/tracked"]);
    let repo = init_repo();
    setup(
        repo.path(),
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );
    fetch(repo.path(), "origin", None).unwrap();

    let wt = tempdir().unwrap();
    let path = wt.path().join("tracked");
    create(
        repo.path(),
        &path,
        "feature/tracked",
        Some("origin/feature/tracked"),
    )
    .unwrap();

    let upstream = run_git(
        Some(&path),
        &["rev-parse", "--abbrev-ref", "feature/tracked@{upstream}"],
    )
    .unwrap();
    assert!(
        upstream.success(),
        "no upstream configured: {}",
        upstream.stderr
    );
    assert_eq!(upstream.stdout_trimmed(), "origin/feature/tracked");

    // And it is listed as a local branch that tracks the remote one.
    let refs = list_refs(repo.path()).unwrap();
    let local = refs
        .iter()
        .find(|r| r.name == "feature/tracked" && r.remote.is_none())
        .expect("local branch should exist");
    assert_eq!(local.upstream.as_deref(), Some("origin/feature/tracked"));
}
