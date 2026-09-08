//! Session baselines, the changes read, the review and the handoff capture.
//!
//! End to end through a real daemon + client, over a real git repository: the
//! whole point of a baseline is that `git status` alone cannot see it, and a
//! fake repository would not exercise that.

mod common;

use std::process::Command;

use protocol::{Request, Response};

fn git(repo: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .is_ok_and(|status| status.success());
    assert!(ok, "git {args:?}");
}

#[test]
fn a_session_reads_back_what_it_changed_and_the_checkout_reviews_it() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");
    let path = repo.path().to_path_buf();

    let daemon = harness.boot();
    let client = daemon.connect("session-changes");
    let workspace = common::add_main_workspace(&client, &path);

    let Response::SessionCreated { session_id, .. } = client
        .request(Request::CreateShellSession {
            workspace_id: workspace,
            parent: None,
            role: domain::SessionRole::Generic,
        })
        .expect("CreateShellSession")
    else {
        panic!("expected SessionCreated");
    };

    // Nothing yet: the session just started from HEAD.
    let Response::SessionChanges(before) = client
        .request(Request::GetSessionChanges { session_id })
        .expect("GetSessionChanges")
    else {
        panic!("expected SessionChanges");
    };
    assert_eq!(before.origin, domain::BaseOrigin::Recorded);
    assert!(before.summary.files.is_empty());
    assert_eq!(before.summary.commit_count, 0);

    // What the "session" did: a commit that leaves the tree clean, plus one
    // untracked file. A working-tree read would miss the first entirely.
    std::fs::write(path.join("committed.txt"), "one\n").expect("write");
    git(&path, &["add", "committed.txt"]);
    git(&path, &["commit", "-m", "add committed.txt"]);
    std::fs::write(path.join("dirty.txt"), "two\nthree\n").expect("write");

    let Response::SessionChanges(after) = client
        .request(Request::GetSessionChanges { session_id })
        .expect("GetSessionChanges")
    else {
        panic!("expected SessionChanges");
    };
    assert_eq!(after.summary.commit_count, 1);
    assert_eq!(after.summary.commits[0].subject, "add committed.txt");
    let mut paths: Vec<&str> = after
        .summary
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, ["committed.txt", "dirty.txt"]);
    // Untracked files are never in `numstat`; the capped read counts them.
    let untracked = after
        .summary
        .files
        .iter()
        .find(|file| file.path == "dirty.txt")
        .expect("dirty.txt");
    assert_eq!(untracked.additions, 2);
    assert_eq!(after.sharing_sessions, 0, "only one session here");

    // The review is the same story with the patches, plus who wrote it.
    let Response::WorkspaceReview(review) = client
        .request(Request::GetWorkspaceReview {
            workspace_id: workspace,
            context_lines: None,
        })
        .expect("GetWorkspaceReview")
    else {
        panic!("expected WorkspaceReview");
    };
    assert_eq!(review.origin, domain::BaseOrigin::Recorded);
    assert_eq!(review.sessions.len(), 1);
    assert_eq!(review.sessions[0].session_id, session_id);
    assert_eq!(review.sessions[0].commit_count, 1);
    assert_eq!(review.diff.files.len(), 2);
    assert!(review
        .diff
        .files
        .iter()
        .any(|file| file.path == "committed.txt" && file.patch.contains("+one")));
}

/// The capture is a fold over decoded cells, so nothing that reaches a handoff
/// prompt can carry an escape sequence — which is what makes it safe to fence
/// into an argv entry.
#[test]
fn a_handoff_capture_is_plain_text_within_its_budget() {
    let harness = common::Harness::new();
    let repo = test_support::init_repo().expect("git repo (git must be installed)");

    let daemon = harness.boot();
    let client = daemon.connect("handoff-capture");
    let workspace = common::add_main_workspace(&client, repo.path());

    let Response::SessionCreated {
        session_id,
        terminal_id,
    } = client
        .request(Request::CreateShellSession {
            workspace_id: workspace,
            parent: None,
            role: domain::SessionRole::Generic,
        })
        .expect("CreateShellSession")
    else {
        panic!("expected SessionCreated");
    };
    let events = client.events();
    common::attach(&client, terminal_id, 80, 24);
    // The line editor discards anything typed before the shell is up, so the
    // marker has to wait for a real prompt rather than for `Running`.
    common::wait_for_shell_prompt(&client, &events, terminal_id);

    // Quoted mid-word so the *echo* of the typed line cannot match it: only
    // the command's own output can.
    common::write_input(&client, terminal_id, "echo FORGE\"\"-MARKER\n");
    common::wait_for_row(&events, common::DEADLINE, |text| {
        text.contains("FORGE-MARKER")
    })
    .expect("the marker reaches the grid");

    let Response::SessionTranscript(transcript) = client
        .request(Request::GetSessionTranscript {
            session_id,
            max_lines: None,
            max_bytes: None,
        })
        .expect("GetSessionTranscript")
    else {
        panic!("expected SessionTranscript");
    };
    assert!(transcript.text.contains("FORGE-MARKER"));
    assert!(
        !transcript.text.contains('\x1b'),
        "the VT engine consumed every escape sequence: {:?}",
        transcript.text
    );

    // The budget bounds what is built, so an unclamped wire value cannot ask
    // the daemon to materialise the whole scrollback.
    let Response::SessionTranscript(tiny) = client
        .request(Request::GetSessionTranscript {
            session_id,
            max_lines: Some(u32::MAX),
            max_bytes: Some(24),
        })
        .expect("GetSessionTranscript")
    else {
        panic!("expected SessionTranscript");
    };
    assert!(tiny.text.len() <= 24);
    assert!(tiny.truncated);
}
