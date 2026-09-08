//! A rebase that stops on conflicts, resolved through the daemon's protocol.
//!
//! Covers the surface the Git panel drives — `GetRebaseState`,
//! `MarkConflictResolved`, `ContinueRebase`, `AbortRebase` — end to end through
//! a real daemon, a real client and real `git`, without the GUI.
//!
//! The `--continue` path is the one worth having a daemon test for: it opens
//! `$EDITOR` for the commit message it already has, and until `git-service`
//! pinned `GIT_EDITOR=true` it would have hung until the ADR-008 timeout.

mod common;

use std::path::Path;
use std::process::Command;

use domain::SequencerOp;
use protocol::{Request, Response};

/// Run git in `repo`, asserting it succeeded.
fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Leave `repo` with a rebase of `topic` onto `main` stopped on `a.txt`.
fn stop_a_rebase(repo: &Path) {
    std::fs::write(repo.join("a.txt"), "base\n").expect("write");
    git(repo, &["add", "a.txt"]);
    git(repo, &["commit", "-m", "base"]);

    git(repo, &["checkout", "-b", "topic"]);
    std::fs::write(repo.join("a.txt"), "topic\n").expect("write");
    git(repo, &["commit", "-am", "topic"]);

    git(repo, &["checkout", "main"]);
    std::fs::write(repo.join("a.txt"), "main\n").expect("write");
    git(repo, &["commit", "-am", "main"]);

    git(repo, &["checkout", "topic"]);
    // Conflicts on purpose: exits non-zero and leaves the rebase stopped.
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rebase", "main"])
        .output()
        .expect("run git rebase");
}

fn rebase_state(client: &client::Client, workspace: domain::WorkspaceId) -> domain::RebaseState {
    let Response::RebaseState(state) = client
        .request(Request::GetRebaseState {
            workspace_id: workspace,
        })
        .expect("GetRebaseState")
    else {
        panic!("expected RebaseState");
    };
    state
}

#[test]
fn a_stopped_rebase_is_resolved_and_continued_through_the_protocol() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    stop_a_rebase(repo.path());

    let daemon = harness.boot();
    let client = daemon.connect("scenario-rebase");
    let workspace = common::add_main_workspace(&client, repo.path());

    // --- the panel's read ---
    let stopped = rebase_state(&client, workspace);
    assert_eq!(stopped.operation, Some(SequencerOp::Rebase));
    assert_eq!(stopped.unresolved(), 1);
    assert_eq!(stopped.conflicts[0].path, "a.txt");
    assert_eq!(stopped.conflicts[0].label(), "both modified");
    assert_eq!(
        stopped.branch.as_deref(),
        Some("topic"),
        "the branch being replayed, not the detached HEAD"
    );
    assert!(!stopped.ready_to_continue());

    // --- continuing while a path is unmerged is an answer, not an error ---
    let Response::RebaseState(refused) = client
        .request(Request::ContinueRebase {
            workspace_id: workspace,
        })
        .expect("ContinueRebase answers even when git refuses")
    else {
        panic!("expected RebaseState");
    };
    assert_eq!(refused.unresolved(), 1, "still stopped on the same path");

    // --- resolve, stage, continue ---
    std::fs::write(repo.path().join("a.txt"), "resolved\n").expect("write");
    let Response::RebaseState(staged) = client
        .request(Request::MarkConflictResolved {
            workspace_id: workspace,
            paths: vec!["a.txt".to_string()],
        })
        .expect("MarkConflictResolved")
    else {
        panic!("expected RebaseState");
    };
    assert!(
        staged.ready_to_continue(),
        "a staged file leaves the conflict list on the very next read"
    );

    let Response::RebaseState(finished) = client
        .request(Request::ContinueRebase {
            workspace_id: workspace,
        })
        .expect("ContinueRebase")
    else {
        panic!("expected RebaseState");
    };
    assert!(!finished.in_progress(), "the replay ran to the end");
    assert_eq!(common::checked_out_branch(repo.path()), "topic");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).expect("read"),
        "resolved\n"
    );
}

#[test]
fn aborting_puts_the_branch_back() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    stop_a_rebase(repo.path());

    let daemon = harness.boot();
    let client = daemon.connect("scenario-rebase-abort");
    let workspace = common::add_main_workspace(&client, repo.path());
    assert!(rebase_state(&client, workspace).in_progress());

    client
        .request(Request::AbortRebase {
            workspace_id: workspace,
        })
        .expect("AbortRebase");

    assert!(!rebase_state(&client, workspace).in_progress());
    assert_eq!(common::checked_out_branch(repo.path()), "topic");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).expect("read"),
        "topic\n",
        "the branch is back where it was before the rebase"
    );
}

#[test]
fn a_path_that_could_parse_as_an_option_is_refused() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");
    stop_a_rebase(repo.path());

    let daemon = harness.boot();
    let client = daemon.connect("scenario-rebase-paths");
    let workspace = common::add_main_workspace(&client, repo.path());

    assert!(
        client
            .request(Request::MarkConflictResolved {
                workspace_id: workspace,
                paths: vec!["../outside.txt".to_string()],
            })
            .is_err(),
        "a path climbing out of the checkout is refused"
    );
    assert_eq!(
        rebase_state(&client, workspace).unresolved(),
        1,
        "and nothing was staged"
    );
}

#[test]
fn a_clean_checkout_reports_no_operation() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo");

    let daemon = harness.boot();
    let client = daemon.connect("scenario-rebase-clean");
    let workspace = common::add_main_workspace(&client, repo.path());

    let state = rebase_state(&client, workspace);
    assert!(!state.in_progress());
    assert!(state.conflicts.is_empty());
    assert_eq!(state.branch.as_deref(), Some("main"));
}
